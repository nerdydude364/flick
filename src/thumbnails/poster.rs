use super::cache;
use super::hash::hash_video_file_cached;
use image::imageops::FilterType;
use std::path::Path;
use std::time::{Duration, Instant};

/// Square thumbnail size for the gallery grid — matches the ~120px cell size
/// (with a little headroom for retina); keeps decode/encode/cache I/O small.
pub const POSTER_SIZE: u32 = 128;

/// Decodes+downscales+caches a `POSTER_SIZE`x`POSTER_SIZE` thumbnail for
/// `path` if one isn't already cached, and returns its content hash either
/// way. Deliberately doesn't construct a `slint::Image` — `slint::Image`
/// isn't `Send`, so this is the half of poster loading that's safe to call
/// from a background thread; pair with `load_cached_poster` (UI-thread only)
/// to actually get pixels into the UI. The only shared state this touches is
/// the cache directory, and writes there go through `cache::write_atomic`.
pub fn ensure_poster_cached(path: &Path) -> Option<String> {
    // Generic content hash (size + head/tail chunks) — named for its
    // original video-sprite use but not video-specific, so it doubles as
    // the poster cache key here.
    let hash = hash_video_file_cached(path).ok()?;
    if cache::is_poster_cached(&hash) {
        return Some(hash);
    }
    {
        let img = image::open(path).ok()?;
        // JPEG has no alpha channel — drop it (transparent source images
        // just get an implicit black background in the thumbnail).
        let thumb = img
            .resize_to_fill(POSTER_SIZE, POSTER_SIZE, FilterType::Triangle)
            .to_rgb8();
        let mut jpeg_bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 82)
            .encode_image(&thumb)
            .ok()?;
        cache::write_atomic(&cache::poster_file(&hash), &jpeg_bytes).ok()?;
        wait_for_poster_ready(&hash, Duration::from_millis(200))?;
    }
    Some(hash)
}

/// Blocks until the poster JPEG is visible with non-zero size, or times out.
pub fn ensure_poster_visible(hash: &str) -> Option<()> {
    wait_for_poster_ready(hash, Duration::from_millis(200))
}

fn wait_for_poster_ready(hash: &str, timeout: Duration) -> Option<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if cache::poster_is_ready(hash) {
            return Some(());
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Loads a previously cached poster thumbnail by content hash. Must run on
/// the UI thread — see `ensure_poster_cached`'s doc comment.
pub fn load_cached_poster(hash: &str) -> Option<slint::Image> {
    slint::Image::load_from_path(&cache::poster_file(hash)).ok()
}

/// UI-thread decode with short retries — workers already block until the JPEG
/// is visible; this covers the brief window before Slint can decode it.
pub fn load_cached_poster_with_retry(hash: &str, attempts: usize) -> Option<slint::Image> {
    let attempts = attempts.clamp(1, 4);
    for attempt in 0..attempts {
        if cache::poster_is_ready(hash)
            && let Some(image) = load_cached_poster(hash)
        {
            return Some(image);
        }
        if attempt + 1 < attempts {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    None
}
