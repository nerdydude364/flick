use super::state::{AppState, Mode};
use crate::AppWindow;
use crate::library::{MediaKind, media_kind};
use libmpv2::Mpv;
use slint::{Model, VecModel};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

const CONCURRENCY: usize = 8;

pub type GalleryThumbResult = (u64, usize, Option<String>);

/// Thumbnail models + channel used to populate the shared gallery grid.
pub struct GalleryContext<'a> {
    pub thumbnails: &'a VecModel<slint::Image>,
    pub video_flags: &'a VecModel<bool>,
    pub tx: &'a Sender<GalleryThumbResult>,
}

/// Whether opening the grid should always rebuild thumbnails or only when
/// the filtered queue order drifted since the grid was last built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GalleryReload {
    IfStale,
    Force,
}

pub fn return_to_gallery(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    gallery: &GalleryContext<'_>,
) -> bool {
    if !state.gallery_open {
        return false;
    }
    open_gallery_grid(mpv, app, state, gallery, GalleryReload::IfStale)
}

/// Back-compat alias — the UI only invokes this to leave single-item view.
pub fn toggle_gallery(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    gallery: &GalleryContext<'_>,
) -> bool {
    return_to_gallery(mpv, app, state, gallery)
}

/// Show the thumbnail grid (no single-item view). Used after import and when
/// switching modes with nothing selected yet.
pub fn open_gallery_grid(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    gallery: &GalleryContext<'_>,
    reload: GalleryReload,
) -> bool {
    prepare_gallery_view(mpv, app, state);

    let needs_reload = reload == GalleryReload::Force
        || !gallery_grid_is_current(state, gallery.thumbnails.row_count());

    if !needs_reload {
        sync_loading_ui(app, state);
        return false;
    }

    prime_gallery_loading_state(state);
    sync_loading_ui(app, state);
    state.pending_gallery_reload = true;
    true
}

/// Runs a deferred thumbnail rebuild queued by [`open_gallery_grid`].
pub fn run_pending_gallery_reload(
    state: &mut AppState,
    app: &AppWindow,
    gallery: &GalleryContext<'_>,
) {
    if !state.pending_gallery_reload {
        return;
    }
    state.pending_gallery_reload = false;
    load_gallery_thumbnails(state, gallery);
    sync_loading_ui(app, state);
}

fn prepare_gallery_view(mpv: &Mpv, app: &AppWindow, state: &mut AppState) {
    state.gallery_open = false;
    if state.slideshow_on && (state.mode == Mode::Image || state.mode == Mode::All) {
        state.slideshow_on = false;
        app.set_slideshow_on(false);
        if let Some(timer) = &state.slideshow_timer {
            timer.stop();
        }
    }
    if state.mode == Mode::Video || (state.mode == Mode::All && state.all_current_is_video) {
        let _ = mpv.command("stop", &[]);
        app.set_playing(false);
    } else if state.mode == Mode::Image {
        let _ = mpv.command("stop", &[]);
        app.set_playing(false);
    }
    app.set_gallery_open(false);
}

fn current_gallery_order(state: &AppState) -> Vec<usize> {
    match state.mode {
        Mode::Video => state
            .queue
            .current_order(&state.search_query, state.shuffle_on),
        Mode::Image => state
            .image_queue
            .current_order(&state.search_query, state.shuffle_on),
        Mode::All => state
            .all_queue
            .current_order(&state.search_query, state.shuffle_on),
    }
}

fn gallery_grid_is_current(state: &AppState, thumb_count: usize) -> bool {
    if state.gallery_order.is_empty() || thumb_count == 0 {
        return false;
    }
    if thumb_count != state.gallery_order.len() {
        return false;
    }
    current_gallery_order(state) == state.gallery_order
}

fn prime_gallery_loading_state(state: &mut AppState) {
    let Some((order, paths, _)) = gallery_source(state) else {
        state.gallery_order.clear();
        state.gallery_thumbs_pending = 0;
        state.gallery_thumbs_loaded = 0;
        return;
    };
    state.gallery_order = order;
    state.gallery_thumbs_pending = paths.len();
    state.gallery_thumbs_loaded = 0;
}

fn sync_loading_ui(app: &AppWindow, state: &AppState) {
    super::loading::sync_loading_ui(app, state);
}

fn gallery_source(state: &AppState) -> Option<(Vec<usize>, Vec<PathBuf>, Vec<bool>)> {
    let (order, paths, is_video): (Vec<usize>, Vec<PathBuf>, Vec<bool>) = match state.mode {
        Mode::Video => {
            let order = state
                .queue
                .current_order(&state.search_query, state.shuffle_on);
            let paths: Vec<PathBuf> = order
                .iter()
                .filter_map(|&i| state.queue.item(i))
                .map(|item| item.path.clone())
                .collect();
            let is_video = vec![true; paths.len()];
            (order, paths, is_video)
        }
        Mode::Image => {
            let order = state
                .image_queue
                .current_order(&state.search_query, state.shuffle_on);
            let paths: Vec<PathBuf> = order
                .iter()
                .filter_map(|&i| state.image_queue.item(i))
                .map(|item| item.path.clone())
                .collect();
            let is_video = vec![false; paths.len()];
            (order, paths, is_video)
        }
        Mode::All => {
            let order = state
                .all_queue
                .current_order(&state.search_query, state.shuffle_on);
            let paths: Vec<PathBuf> = order
                .iter()
                .filter_map(|&i| state.all_queue.item(i))
                .map(|item| item.path.clone())
                .collect();
            let is_video: Vec<bool> = paths
                .iter()
                .map(|p| media_kind(p) == MediaKind::Video)
                .collect();
            (order, paths, is_video)
        }
    };
    if paths.is_empty() {
        None
    } else {
        Some((order, paths, is_video))
    }
}

fn load_gallery_thumbnails(state: &mut AppState, gallery: &GalleryContext<'_>) {
    let Some((order, paths, is_video)) = gallery_source(state) else {
        state.gallery_order.clear();
        state.gallery_thumbs_pending = 0;
        state.gallery_thumbs_loaded = 0;
        gallery.thumbnails.set_vec(Vec::new());
        gallery.video_flags.set_vec(Vec::new());
        return;
    };

    state.gallery_generation += 1;
    let generation = state.gallery_generation;
    state.gallery_order = order;
    state.gallery_thumbs_pending = paths.len();
    state.gallery_thumbs_loaded = 0;

    if gallery.thumbnails.row_count() != paths.len() {
        gallery
            .thumbnails
            .set_vec(vec![slint::Image::default(); paths.len()]);
    }
    gallery.video_flags.set_vec(is_video.clone());

    let tx = gallery.tx.clone();
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
                        let _ = tx.send((generation, pos, hash));
                    });
                }
            });
        }
    });
}

pub fn apply_gallery_thumb(
    state: &mut AppState,
    app: &AppWindow,
    gallery_model: &VecModel<slint::Image>,
    result: GalleryThumbResult,
) {
    let (generation, pos, hash) = result;
    if generation != state.gallery_generation || pos >= gallery_model.row_count() {
        return;
    }
    if let Some(hash) = hash {
        if let Some(image) = crate::thumbnails::load_cached_poster(&hash) {
            gallery_model.set_row_data(pos, image);
        }
    }
    if state.gallery_thumbs_loaded < state.gallery_thumbs_pending {
        state.gallery_thumbs_loaded += 1;
    }
    super::loading::sync_loading_ui(app, state);
}
