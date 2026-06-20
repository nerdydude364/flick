use super::state::{AppState, Mode};
use crate::AppWindow;
use crate::library::{MediaKind, media_kind};
use slint::{Model, VecModel};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

const CONCURRENCY: usize = 8;

pub type GalleryThumbResult = (u64, usize, String);

pub fn toggle_gallery(
    app: &AppWindow,
    state: &mut AppState,
    gallery_model: &VecModel<slint::Image>,
    gallery_video_flags: &VecModel<bool>,
    gallery_tx: &Sender<GalleryThumbResult>,
) {
    state.gallery_open = !state.gallery_open;
    if !state.gallery_open {
        if state.mode == Mode::Image || state.mode == Mode::All {
            state.slideshow_on = false;
            app.set_slideshow_on(false);
            if let Some(timer) = &state.slideshow_timer {
                timer.stop();
            }
        }
        open_gallery(state, gallery_model, gallery_video_flags, gallery_tx);
    }
    app.set_gallery_open(state.gallery_open);
}

fn open_gallery(
    state: &mut AppState,
    gallery_model: &VecModel<slint::Image>,
    gallery_video_flags: &VecModel<bool>,
    gallery_tx: &Sender<GalleryThumbResult>,
) {
    state.gallery_generation += 1;
    let generation = state.gallery_generation;

    let (order, paths): (Vec<usize>, Vec<PathBuf>) = match state.mode {
        Mode::Image => {
            let order = state
                .image_queue
                .current_order(&state.search_query, state.shuffle_on);
            let paths = order
                .iter()
                .filter_map(|&i| state.image_queue.item(i))
                .map(|item| item.path.clone())
                .collect();
            (order, paths)
        }
        Mode::All => {
            let order = state
                .all_queue
                .current_order(&state.search_query, state.shuffle_on);
            let paths = order
                .iter()
                .filter_map(|&i| state.all_queue.item(i))
                .map(|item| item.path.clone())
                .collect();
            (order, paths)
        }
        Mode::Video => return,
    };

    state.gallery_order = order;
    let is_video: Vec<bool> = paths
        .iter()
        .map(|p| media_kind(p) == MediaKind::Video)
        .collect();

    gallery_model.set_vec(vec![slint::Image::default(); paths.len()]);
    gallery_video_flags.set_vec(is_video.clone());

    let tx = gallery_tx.clone();
    std::thread::spawn(move || {
        for batch_start in (0..paths.len()).step_by(CONCURRENCY) {
            let batch_end = (batch_start + CONCURRENCY).min(paths.len());
            std::thread::scope(|scope| {
                for (offset, path) in paths[batch_start..batch_end].iter().enumerate() {
                    let tx = tx.clone();
                    let pos = batch_start + offset;
                    let is_vid = is_video[pos];
                    scope.spawn(move || {
                        let hash = if is_vid {
                            crate::thumbnails::ensure_video_poster_cached(path)
                        } else {
                            crate::thumbnails::ensure_poster_cached(path)
                        };
                        if let Some(hash) = hash {
                            let _ = tx.send((generation, pos, hash));
                        }
                    });
                }
            });
        }
    });
}

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
