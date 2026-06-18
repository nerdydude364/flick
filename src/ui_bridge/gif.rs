use super::state::AppState;
use crate::AppWindow;
use std::path::Path;
use std::time::{Duration, Instant};

/// Decoded animated-GIF frames + playback position. Slint's `Image` element
/// only shows a GIF's first frame natively, so full parity with the
/// original (where `<img>` tags animate GIFs for free) means decoding and
/// driving frames ourselves.
pub struct GifAnimation {
    pub(crate) frames: Vec<(slint::Image, Duration)>,
    current: usize,
    frame_shown_at: Instant,
}

/// Decodes every frame of an animated GIF up front — fine for typically-small
/// GIFs, but a real cost for very large ones (this runs synchronously on the
/// UI thread when the image is shown, same eager-load tradeoff as the rest
/// of the image viewer for v1).
pub(crate) fn decode_gif(path: &Path) -> Result<GifAnimation, String> {
    use image::AnimationDecoder;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|e| e.to_string())?;
        let delay: Duration = frame.delay().into();
        let buffer = frame.into_buffer();
        let (width, height) = buffer.dimensions();
        let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(buffer.as_raw(), width, height);
        frames.push((slint::Image::from_rgba8(pixels), delay));
    }
    if frames.is_empty() {
        return Err("GIF had no frames".to_string());
    }
    Ok(GifAnimation { frames, current: 0, frame_shown_at: Instant::now() })
}

/// Advances any in-progress GIF animation whose current frame's delay has
/// elapsed. Called from a fast (33ms) polling timer in main.rs — a polling
/// approach (rather than a precise self-rescheduling timer chain) so that
/// showing a new image doesn't need to thread a `Timer` handle through every
/// `ui_bridge` function that can change which image is displayed.
pub fn tick_gif_animation(app: &AppWindow, state: &mut AppState) {
    let Some(gif) = state.gif_animation.as_mut() else { return };
    let (_, delay) = &gif.frames[gif.current];
    if gif.frame_shown_at.elapsed() < *delay {
        return;
    }
    gif.current = (gif.current + 1) % gif.frames.len();
    gif.frame_shown_at = Instant::now();
    app.set_current_image(gif.frames[gif.current].0.clone());
}
