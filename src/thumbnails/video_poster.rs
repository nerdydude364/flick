use super::cache;
use super::frame;
use super::hash::hash_video_file_cached;
use super::poster::POSTER_SIZE;
use image::codecs::jpeg::JpegEncoder;
use std::path::Path;

/// Single-frame video thumbnail for the All-mode gallery grid — same cache
/// directory and hash key as image posters, but sourced via headless mpv.
pub fn ensure_video_poster_cached(path: &Path) -> Option<String> {
    let hash = hash_video_file_cached(path).ok()?;
    if cache::is_poster_cached(&hash) {
        return Some(hash);
    }
    let frame = frame::extract_frame(path, 1.0, POSTER_SIZE, POSTER_SIZE).ok()?;
    let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)?;
    // `extract_frame` already letterboxes to POSTER_SIZE — skip a second resize.
    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut jpeg_bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg_bytes, 82)
        .encode_image(&rgb)
        .ok()?;
    cache::write_atomic(&cache::poster_file(&hash), &jpeg_bytes).ok()?;
    Some(hash)
}
