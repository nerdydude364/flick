use super::cache;
use super::frame::{ExtractError, extract_frame, probe_duration_gated};
use super::hash::hash_video_file_cached;
use image::{ImageBuffer, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

pub const THUMB_W: u32 = 160;
pub const THUMB_H: u32 = 90;
pub const COLUMNS: u32 = 10;
pub const MAX_FRAMES: u32 = 600;
pub const MIN_INTERVAL_SEC: f64 = 0.25;
const CONCURRENCY: usize = 8;
/// Decoded sprite sheets kept in memory so sidebar hover / scrub preview don't
/// re-read and JPEG-decode the same multi-megabyte sheet on every interaction.
const SPRITE_MEMORY_CACHE_MAX: usize = 6;

struct SpriteMemoryCache {
    entries: HashMap<String, (slint::Image, SpriteMeta)>,
    order: Vec<String>,
}

impl SpriteMemoryCache {
    fn get(&mut self, hash: &str) -> Option<(slint::Image, SpriteMeta)> {
        if !self.entries.contains_key(hash) {
            return None;
        }
        self.order.retain(|h| h != hash);
        self.order.push(hash.to_string());
        self.entries
            .get(hash)
            .map(|(image, meta)| (image.clone(), meta.clone()))
    }

    fn insert(&mut self, hash: String, image: slint::Image, meta: SpriteMeta) {
        if self.entries.contains_key(&hash) {
            self.order.retain(|h| h != &hash);
        } else if self.entries.len() >= SPRITE_MEMORY_CACHE_MAX
            && let Some(oldest) = self.order.first().cloned()
        {
            self.order.remove(0);
            self.entries.remove(&oldest);
        }
        self.order.push(hash.clone());
        self.entries.insert(hash, (image, meta));
    }
}

thread_local! {
    static SPRITE_MEMORY_CACHE: RefCell<SpriteMemoryCache> = RefCell::new(SpriteMemoryCache {
        entries: HashMap::new(),
        order: Vec::new(),
    });
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpriteMeta {
    pub interval_sec: f64,
    pub frame_count: u32,
    pub columns: u32,
    pub rows: u32,
    pub thumb_width: u32,
    pub thumb_height: u32,
}

#[derive(Debug)]
pub enum SpriteError {
    Extract(ExtractError),
    Io(std::io::Error),
    Image(image::ImageError),
    Json(String),
}

impl std::fmt::Display for SpriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Extract(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Image(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "{e}"),
        }
    }
}

/// Generates (or returns the cached) sprite sheet for `video_path` — port of
/// `sprite:generate` in main.js, minus the ffmpeg subprocess plumbing: frame
/// extraction goes through `super::frame::extract_frame` (libmpv encode mode,
/// no external dependency), and tiling goes through the `image` crate instead
/// of ffmpeg's `tile` filter.
pub fn generate_sprite(video_path: &Path) -> Result<(), SpriteError> {
    let hash = hash_video_file_cached(video_path).map_err(SpriteError::Io)?;
    let sprite_path = cache::sprite_file(&hash);
    let meta_path = cache::meta_file(&hash);

    if cache::is_cached(&hash) {
        return Ok(());
    }

    // Cheap validity gate before committing to the full per-frame
    // extraction pass below — catches corrupt/truncated files fast, and
    // (since the result is persisted by content hash) only ever pays that
    // cost once per file, including across app restarts.
    let duration = probe_duration_gated(video_path, &hash).map_err(SpriteError::Extract)?;

    // Dynamic interval: ceil(duration/MAX_FRAMES) gives 1s for short videos,
    // scales up automatically for longer ones so frame count stays bounded.
    let raw_interval = (duration / MAX_FRAMES as f64).ceil().max(1.0);
    let interval_sec = raw_interval.max(MIN_INTERVAL_SEC);
    let frame_count = ((duration / interval_sec).floor() as u32).clamp(1, MAX_FRAMES);
    let rows = frame_count.div_ceil(COLUMNS);

    let mut sheet: RgbaImage = ImageBuffer::new(COLUMNS * THUMB_W, rows * THUMB_H);

    // Extract frames with bounded concurrency, same batch shape as the
    // original's `CONCURRENCY = 8` ffmpeg-process pool, just spawning OS
    // threads that each drive their own headless mpv instance instead.
    for batch_start in (0..frame_count).step_by(CONCURRENCY) {
        let batch_end = (batch_start + CONCURRENCY as u32).min(frame_count);
        let results: Vec<(u32, Result<super::frame::RawFrame, ExtractError>)> =
            std::thread::scope(|scope| {
                let handles: Vec<_> = (batch_start..batch_end)
                    .map(|i| {
                        scope.spawn(move || {
                            let t = i as f64 * interval_sec;
                            (i, extract_frame(video_path, t, THUMB_W, THUMB_H))
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("extraction thread panicked"))
                    .collect()
            });

        for (i, result) in results {
            let raw = result.map_err(SpriteError::Extract)?;
            let tile = ImageBuffer::<Rgba<u8>, _>::from_raw(raw.width, raw.height, raw.rgba)
                .expect("extract_frame buffer always matches its declared dimensions");
            let col = i % COLUMNS;
            let row = i / COLUMNS;
            image::imageops::replace(
                &mut sheet,
                &tile,
                (col * THUMB_W) as i64,
                (row * THUMB_H) as i64,
            );
        }
    }

    let mut jpeg_bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 80)
        .encode_image(&sheet)
        .map_err(SpriteError::Image)?;
    cache::write_atomic(&sprite_path, &jpeg_bytes).map_err(SpriteError::Io)?;

    let meta = SpriteMeta {
        interval_sec,
        frame_count,
        columns: COLUMNS,
        rows,
        thumb_width: THUMB_W,
        thumb_height: THUMB_H,
    };
    let meta_json = serde_json::to_string(&meta).map_err(|e| SpriteError::Json(e.to_string()))?;
    cache::write_atomic(&meta_path, meta_json.as_bytes()).map_err(SpriteError::Io)?;

    Ok(())
}

/// Loads a previously cached sprite sheet + metadata for UI preview (progress-bar
/// hover thumbnails, sidebar list hover). Returns `None` if the cache entry is
/// missing or corrupt. Decoded sheets are retained in a small in-memory LRU so
/// repeated hovers don't hit disk.
pub fn load_cached_sprite(hash: &str) -> Option<(slint::Image, SpriteMeta)> {
    if let Some(hit) = SPRITE_MEMORY_CACHE.with(|cache| cache.borrow_mut().get(hash)) {
        return Some(hit);
    }
    if !cache::is_cached(hash) {
        crate::flick_debug!("[sprite] cache entry missing hash {hash}");
        return None;
    }
    let meta_json = match std::fs::read_to_string(cache::meta_file(hash)) {
        Ok(json) => json,
        Err(err) => {
            crate::flick_debug!("[sprite] meta read failed hash {hash}: {err}");
            return None;
        }
    };
    let meta: SpriteMeta = match serde_json::from_str(&meta_json) {
        Ok(meta) => meta,
        Err(err) => {
            crate::flick_debug!("[sprite] meta parse failed hash {hash}: {err}");
            return None;
        }
    };
    let image = match slint::Image::load_from_path(&cache::sprite_file(hash)) {
        Ok(image) => image,
        Err(err) => {
            crate::flick_debug!(
                "[sprite] image decode failed hash {hash} ({}): {err}",
                cache::sprite_file(hash).display()
            );
            return None;
        }
    };
    SPRITE_MEMORY_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .insert(hash.to_string(), image.clone(), meta.clone());
    });
    Some((image, meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spike test (Phase 3): full pipeline end-to-end against the synthetic
    /// test video — bounded-concurrency extraction, tiling, caching. Ignored
    /// by default (environment-dependent fixture), run explicitly to verify.
    #[test]
    #[ignore]
    fn spike_generate_sprite_full_pipeline() {
        let input = Path::new("/tmp/flick-test-media/test.mp4");
        assert!(input.exists(), "test fixture missing: {}", input.display());

        let hash = hash_video_file_cached(input).unwrap();
        let _ = std::fs::remove_file(cache::sprite_file(&hash));
        let _ = std::fs::remove_file(cache::meta_file(&hash));

        generate_sprite(input).expect("generate_sprite failed");
        let (_, meta) = load_cached_sprite(&hash).expect("cached sprite missing after generate");
        eprintln!("sprite meta: {:?}", meta);
        assert!(cache::sprite_file(&hash).exists());
        // 30s test video, MIN_INTERVAL_SEC floor means 1 frame/sec -> ~30 frames, 3 rows of 10.
        assert_eq!(meta.columns, COLUMNS);
        assert!(meta.frame_count >= 1);

        std::fs::copy(cache::sprite_file(&hash), "/tmp/flick-sprite-spike.jpg").unwrap();
        eprintln!("wrote /tmp/flick-sprite-spike.jpg");

        // Second call should hit the cache and return instantly with the same meta.
        generate_sprite(input).expect("cached generate_sprite failed");
        let (_, cached_meta) = load_cached_sprite(&hash).expect("cached sprite missing");
        assert_eq!(cached_meta.frame_count, meta.frame_count);
    }
}
