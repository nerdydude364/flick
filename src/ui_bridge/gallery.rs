use super::loading::{gallery_busy, patch_playlist_thumbnail_for_hash};
use super::state::{AppState, Mode};
use crate::AppWindow;
use crate::library::{MediaKind, media_kind};
use libmpv2::Mpv;
use slint::{Model, VecModel};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::sync::{Condvar, Mutex};

/// Keep headless mpv poster extraction bounded — high concurrency is a common
/// source of flaky frame grabs on Linux when many files load at once.
const CONCURRENCY: usize = 4;
const GENERATION_RETRIES: usize = 2;
const UI_LOAD_ATTEMPTS: usize = 3;
const MAX_GALLERY_RETRY_PASSES: u8 = 3;

pub type GalleryThumbResult = (u64, usize, Option<String>);

/// Thumbnail models + channel used to populate the shared gallery grid.
pub struct GalleryContext<'a> {
    pub thumbnails: &'a VecModel<slint::Image>,
    pub video_flags: &'a VecModel<bool>,
    pub failed_flags: &'a VecModel<bool>,
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
    if state.mode == Mode::Video
        || state.mode == Mode::Image
        || (state.mode == Mode::All && state.all_current_is_video)
    {
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
        state.gallery_thumbs_failed = 0;
        return;
    };
    state.gallery_order = order;
    state.gallery_thumbs_pending = paths.len();
    state.gallery_thumbs_loaded = 0;
    state.gallery_thumbs_failed = 0;
}

fn sync_loading_ui(app: &AppWindow, state: &mut AppState) {
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

fn thumb_focus_index(state: &AppState) -> usize {
    let now_playing = match state.mode {
        Mode::Video => state.queue.now_playing(),
        Mode::Image => state.image_queue.now_playing(),
        Mode::All => state.all_queue.now_playing(),
    };
    now_playing
        .and_then(|idx| state.gallery_order.iter().position(|&i| i == idx))
        .unwrap_or(0)
}

fn thumb_work_order(len: usize, focus: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    order.sort_by_key(|&i| i.abs_diff(focus));
    order
}

struct ConcurrencyGate {
    slots: Mutex<usize>,
    available: Condvar,
}

impl ConcurrencyGate {
    fn new(limit: usize) -> Self {
        Self {
            slots: Mutex::new(limit),
            available: Condvar::new(),
        }
    }

    fn acquire(&self) {
        let mut slots = self.slots.lock().unwrap();
        while *slots == 0 {
            slots = self.available.wait(slots).unwrap();
        }
        *slots -= 1;
    }

    fn release(&self) {
        let mut slots = self.slots.lock().unwrap();
        *slots += 1;
        self.available.notify_one();
    }
}

struct GateGuard<'a>(&'a ConcurrencyGate);

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

fn generate_poster_hash(path: &std::path::Path, is_video: bool) -> Option<String> {
    for attempt in 0..GENERATION_RETRIES {
        let hash = if is_video {
            crate::thumbnails::ensure_video_poster_cached(path)
        } else {
            crate::thumbnails::ensure_poster_cached(path)
        };
        if let Some(ref h) = hash
            && crate::thumbnails::cache::poster_is_ready(h)
        {
            return hash;
        }
        if attempt + 1 < GENERATION_RETRIES {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
    None
}

fn spawn_gallery_thumb_workers(
    generation: u64,
    paths: Vec<PathBuf>,
    is_video: Vec<bool>,
    work_order: Vec<usize>,
    tx: Sender<GalleryThumbResult>,
) {
    std::thread::spawn(move || {
        let gate = Arc::new(ConcurrencyGate::new(CONCURRENCY));
        std::thread::scope(|scope| {
            for pos in work_order {
                let path = paths[pos].clone();
                let is_vid = is_video[pos];
                let tx = tx.clone();
                let gate = Arc::clone(&gate);
                scope.spawn(move || {
                    gate.acquire();
                    let _release = GateGuard(&gate);
                    let hash = generate_poster_hash(&path, is_vid);
                    let _ = tx.send((generation, pos, hash));
                });
            }
        });
    });
}

fn load_gallery_thumbnails(state: &mut AppState, gallery: &GalleryContext<'_>) {
    let Some((order, paths, is_video)) = gallery_source(state) else {
        state.gallery_order.clear();
        state.gallery_thumbs_pending = 0;
        state.gallery_thumbs_loaded = 0;
        state.gallery_thumbs_failed = 0;
        state.gallery_thumb_retry_pass = 0;
        gallery.thumbnails.set_vec(Vec::new());
        gallery.video_flags.set_vec(Vec::new());
        gallery.failed_flags.set_vec(Vec::new());
        return;
    };

    state.gallery_generation += 1;
    let generation = state.gallery_generation;
    state.gallery_order = order;
    state.gallery_thumbs_pending = paths.len();
    state.gallery_thumbs_loaded = 0;
    state.gallery_thumbs_failed = 0;
    state.gallery_thumb_retry_pass = 0;

    gallery
        .thumbnails
        .set_vec(vec![slint::Image::default(); paths.len()]);
    gallery.video_flags.set_vec(is_video.clone());
    gallery.failed_flags.set_vec(vec![false; paths.len()]);

    let focus = thumb_focus_index(state);
    let work_order = thumb_work_order(paths.len(), focus);
    spawn_gallery_thumb_workers(generation, paths, is_video, work_order, gallery.tx.clone());
}

/// Extends the grid when a merge import appends to an unchanged order prefix,
/// generating thumbnails only for the new tail — avoids redoing work already
/// on screen. Returns false when a full rebuild is required instead.
pub fn try_append_gallery_thumbnails(state: &mut AppState, gallery: &GalleryContext<'_>) -> bool {
    if state.gallery_open || gallery_busy(state) {
        return false;
    }
    let Some((order, paths, is_video)) = gallery_source(state) else {
        return false;
    };
    let old = &state.gallery_order;
    let thumb_count = gallery.thumbnails.row_count();
    if old.is_empty() || order.len() <= old.len() || thumb_count != old.len() {
        return false;
    }
    if order[..old.len()] != old[..] {
        return false;
    }

    let append_from = old.len();
    let new_count = paths.len() - append_from;
    state.gallery_generation += 1;
    let generation = state.gallery_generation;
    state.gallery_order = order;
    state.gallery_thumb_retry_pass = 0;
    state.gallery_thumbs_pending = state.gallery_thumbs_pending.saturating_add(new_count);

    let mut thumbs: Vec<slint::Image> = (0..thumb_count)
        .filter_map(|i| gallery.thumbnails.row_data(i))
        .collect();
    thumbs.resize(paths.len(), slint::Image::default());
    gallery.thumbnails.set_vec(thumbs);

    let mut flags: Vec<bool> = (0..gallery.video_flags.row_count())
        .filter_map(|i| gallery.video_flags.row_data(i))
        .collect();
    flags.extend_from_slice(&is_video[append_from..]);
    gallery.video_flags.set_vec(flags);

    let mut failed: Vec<bool> = (0..gallery.failed_flags.row_count())
        .filter_map(|i| gallery.failed_flags.row_data(i))
        .collect();
    failed.resize(paths.len(), false);
    gallery.failed_flags.set_vec(failed);

    let focus = thumb_focus_index(state);
    let mut work_order: Vec<usize> = (append_from..paths.len()).collect();
    work_order.sort_by_key(|&i| i.abs_diff(focus));

    spawn_gallery_thumb_workers(generation, paths, is_video, work_order, gallery.tx.clone());
    true
}

fn retry_failed_gallery_thumbnails(
    state: &mut AppState,
    gallery: &GalleryContext<'_>,
    failed_positions: Vec<usize>,
) {
    let Some((_, paths, is_video)) = gallery_source(state) else {
        return;
    };
    if failed_positions.is_empty() {
        return;
    }

    state.gallery_generation += 1;
    let generation = state.gallery_generation;
    state.gallery_thumb_retry_pass = state.gallery_thumb_retry_pass.saturating_add(1);
    state.gallery_thumbs_pending = failed_positions.len();
    state.gallery_thumbs_loaded = 0;
    state.gallery_thumbs_failed = 0;

    for pos in &failed_positions {
        gallery.failed_flags.set_row_data(*pos, false);
    }

    let work_paths: Vec<PathBuf> = failed_positions.iter().map(|&i| paths[i].clone()).collect();
    let work_is_video: Vec<bool> = failed_positions.iter().map(|&i| is_video[i]).collect();
    let work_order: Vec<usize> = (0..failed_positions.len()).collect();

    let tx = gallery.tx.clone();
    std::thread::spawn(move || {
        let gate = Arc::new(ConcurrencyGate::new(CONCURRENCY));
        std::thread::scope(|scope| {
            for local_pos in work_order {
                let path = work_paths[local_pos].clone();
                let is_vid = work_is_video[local_pos];
                let grid_pos = failed_positions[local_pos];
                let tx = tx.clone();
                let gate = Arc::clone(&gate);
                scope.spawn(move || {
                    gate.acquire();
                    let _release = GateGuard(&gate);
                    let hash = generate_poster_hash(&path, is_vid);
                    let _ = tx.send((generation, grid_pos, hash));
                });
            }
        });
    });
}

fn failed_gallery_positions(failed_flags: &VecModel<bool>) -> Vec<usize> {
    (0..failed_flags.row_count())
        .filter(|&i| failed_flags.row_data(i).unwrap_or(false))
        .collect()
}

fn finish_gallery_batch(
    state: &mut AppState,
    app: &AppWindow,
    gallery: &GalleryContext<'_>,
    _playlist_model: &VecModel<crate::PlaylistItemData>,
) {
    let failed = failed_gallery_positions(gallery.failed_flags);
    if !failed.is_empty() && state.gallery_thumb_retry_pass < MAX_GALLERY_RETRY_PASSES {
        retry_failed_gallery_thumbnails(state, gallery, failed);
        sync_loading_ui(app, state);
        return;
    }

    state.gallery_thumb_retry_pass = 0;
    sync_loading_ui(app, state);
}

pub fn apply_gallery_thumb(
    state: &mut AppState,
    app: &AppWindow,
    gallery: &GalleryContext<'_>,
    playlist_model: &VecModel<crate::PlaylistItemData>,
    result: GalleryThumbResult,
) {
    let (generation, pos, hash) = result;
    if generation != state.gallery_generation || pos >= gallery.thumbnails.row_count() {
        return;
    }
    let success = if let Some(ref hash) = hash {
        if let Some(image) =
            crate::thumbnails::load_cached_poster_with_retry(hash, UI_LOAD_ATTEMPTS)
        {
            gallery.thumbnails.set_row_data(pos, image);
            patch_playlist_thumbnail_for_hash(state, playlist_model, hash);
            true
        } else {
            gallery.failed_flags.set_row_data(pos, true);
            false
        }
    } else {
        gallery.failed_flags.set_row_data(pos, true);
        false
    };
    if success {
        state.gallery_thumbs_loaded = state.gallery_thumbs_loaded.saturating_add(1);
    } else {
        state.gallery_thumbs_failed = state.gallery_thumbs_failed.saturating_add(1);
    }
    let done = state.gallery_thumbs_loaded + state.gallery_thumbs_failed;
    let pending = state.gallery_thumbs_pending;
    if done == pending {
        finish_gallery_batch(state, app, gallery, playlist_model);
    } else if done.is_multiple_of(4) {
        sync_loading_ui(app, state);
    }
}
