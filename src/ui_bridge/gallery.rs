use super::state::AppState;
use crate::AppWindow;
use slint::{Model, VecModel};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// Bounded worker count for background poster-thumbnail generation — mirrors
/// the video sprite pipeline's own `CONCURRENCY`, just applied across
/// independent images instead of one video's frames.
const CONCURRENCY: usize = 8;

/// One ready-to-load poster thumbnail's cache key, tagged with the grid
/// generation it was requested for (see `AppState::gallery_generation`) and
/// its position in that generation's `gallery_order`/`gallery_thumbnails`.
/// Carries the content hash rather than a loaded `slint::Image` — `Image`
/// isn't `Send`, so actually loading it from the cache has to happen back on
/// the UI thread (see `apply_gallery_thumb`).
pub type GalleryThumbResult = (u64, usize, String);

/// Toggles between the single fullscreen image view and the grid overview —
/// flips `gallery_open` and, when opening the grid, turns off any running
/// slideshow (its timer would otherwise keep calling `show_image_at`, which
/// forces `gallery_open` back to true every tick) and (re)populates the grid.
pub fn toggle_gallery(
    app: &AppWindow,
    state: &mut AppState,
    gallery_model: &VecModel<slint::Image>,
    gallery_tx: &Sender<GalleryThumbResult>,
) {
    state.gallery_open = !state.gallery_open;
    if !state.gallery_open {
        state.slideshow_on = false;
        app.set_slideshow_on(false);
        if let Some(timer) = &state.slideshow_timer {
            timer.stop();
        }
        open_gallery(state, gallery_model, gallery_tx);
    }
    app.set_gallery_open(state.gallery_open);
}

/// Populates `gallery_order` and a same-length placeholder thumbnail model
/// immediately (cheap, no I/O — every cell just shows its background color
/// until filled in), then hands the actual file paths to a background
/// thread for decoding. Ready thumbnails stream back individually over
/// `gallery_tx` and get applied as they finish (see `apply_gallery_thumb`),
/// so 1000+ multi-megapixel originals never block the UI thread or get held
/// in memory at full resolution all at once — see `ensure_poster_cached`'s
/// doc comment for the same reasoning applied per-image.
fn open_gallery(
    state: &mut AppState,
    gallery_model: &VecModel<slint::Image>,
    gallery_tx: &Sender<GalleryThumbResult>,
) {
    state.gallery_generation += 1;
    let generation = state.gallery_generation;

    let order = state
        .image_queue
        .current_order(&state.search_query, state.shuffle_on);
    let paths: Vec<PathBuf> = order
        .iter()
        .filter_map(|&i| state.image_queue.item(i))
        .map(|item| item.path.clone())
        .collect();
    state.gallery_order = order;

    gallery_model.set_vec(vec![slint::Image::default(); paths.len()]);

    let tx = gallery_tx.clone();
    std::thread::spawn(move || {
        for batch_start in (0..paths.len()).step_by(CONCURRENCY) {
            let batch_end = (batch_start + CONCURRENCY).min(paths.len());
            std::thread::scope(|scope| {
                for (offset, path) in paths[batch_start..batch_end].iter().enumerate() {
                    let tx = tx.clone();
                    let pos = batch_start + offset;
                    scope.spawn(move || {
                        if let Some(hash) = crate::thumbnails::ensure_poster_cached(path) {
                            let _ = tx.send((generation, pos, hash));
                        }
                    });
                }
            });
        }
    });
}

/// Applies one ready poster thumbnail (from the channel `open_gallery`'s
/// worker thread sends to) by loading it from the cache right here on the UI
/// thread — drops it if the grid has since moved on to a newer generation
/// (closed/reopened, search/shuffle changed) rather than writing into what's
/// now the wrong row.
pub fn apply_gallery_thumb(
    state: &AppState,
    gallery_model: &VecModel<slint::Image>,
    result: GalleryThumbResult,
) {
    let (generation, pos, hash) = result;
    if generation != state.gallery_generation || pos >= gallery_model.row_count() {
        return;
    }
    if let Some(image) = crate::thumbnails::load_cached_poster(&hash) {
        gallery_model.set_row_data(pos, image);
    }
}
