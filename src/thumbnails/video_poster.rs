use super::cache;
use super::frame;
use super::hash::hash_video_file_cached;
use super::poster::{self, POSTER_SIZE};
use image::codecs::jpeg::JpegEncoder;
use std::path::Path;

/// Single-frame video thumbnail for the All-mode gallery grid — same cache
/// directory and hash key as image posters, but sourced via headless mpv.
pub fn ensure_video_poster_cached(path: &Path) -> Option<String> {
    let hash = match hash_video_file_cached(path) {
        Ok(hash) => hash,
        Err(err) => {
            crate::flick_debug!("[video poster] hash failed {}: {err}", path.display());
            return None;
        }
    };
    if cache::is_poster_cached(&hash) {
        return Some(hash);
    }
    // Cheap validity gate before committing to a full frame extraction —
    // catches corrupt/truncated files fast, and (since the result is
    // persisted by content hash) only ever pays that cost once per file.
    if let Err(err) = frame::probe_duration_gated(path, &hash) {
        crate::flick_debug!("[video poster] probe failed {}: {err}", path.display());
        return None;
    }
    let frame = match frame::extract_frame(path, 1.0, POSTER_SIZE, POSTER_SIZE) {
        Ok(frame) => frame,
        Err(err) => {
            crate::flick_debug!(
                "[video poster] frame extract failed {}: {err}",
                path.display()
            );
            return None;
        }
    };
    let img = match image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba) {
        Some(img) => img,
        None => {
            crate::flick_debug!(
                "[video poster] rgba buffer size mismatch {} ({}x{})",
                path.display(),
                frame.width,
                frame.height
            );
            return None;
        }
    };
    // `extract_frame` already letterboxes to POSTER_SIZE — skip a second resize.
    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut jpeg_bytes = Vec::new();
    if let Err(err) = JpegEncoder::new_with_quality(&mut jpeg_bytes, 82).encode_image(&rgb) {
        crate::flick_debug!(
            "[video poster] jpeg encode failed {}: {err}",
            path.display()
        );
        return None;
    }
    if let Err(err) = cache::write_atomic(&cache::poster_file(&hash), &jpeg_bytes) {
        crate::flick_debug!(
            "[video poster] cache write failed {}: {err}",
            path.display()
        );
        return None;
    }
    if poster::ensure_poster_visible(&hash).is_none() {
        crate::flick_debug!(
            "[video poster] visibility timeout {} (hash {hash})",
            path.display()
        );
        return None;
    }
    Some(hash)
}
