use super::rebuild_playlist_model;
use super::state::{AppState, Mode, SpriteStatus};
use crate::thumbnails;
use crate::{AppWindow, PlaylistItemData};
use slint::VecModel;
use std::cell::RefCell;
use std::rc::Rc;

/// Clears progress-bar scrub preview state (no sprite sheet loaded).
pub fn clear_sprite_preview(app: &AppWindow) {
    app.set_sprite_ready(false);
    app.set_sprite_loading(false);
    app.set_sprite_frame_count(0);
}

/// Pushes the cached sprite sheet for the currently playing video (if any)
/// into the AppWindow properties that drive the progress-bar hover preview.
pub fn sync_sprite_preview(app: &AppWindow, state: &mut AppState) {
    let path = state.queue.now_playing().and_then(|index| {
        state.queue.item(index).map(|item| item.path.clone())
    });
    let Some(path) = path else {
        clear_sprite_preview(app);
        return;
    };
    if !crate::library::is_video_file(&path) {
        clear_sprite_preview(app);
        return;
    }

    let status = state.sprite_status_for(&path);
    match status {
        SpriteStatus::Done => {
            let Some(hash) = state.sprite_hash_for(&path) else {
                clear_sprite_preview(app);
                return;
            };
            let Some((image, meta)) = thumbnails::load_cached_sprite(&hash) else {
                clear_sprite_preview(app);
                return;
            };
            apply_sprite_meta_to_ui(app, &meta, image);
            app.set_sprite_loading(false);
            app.set_sprite_ready(true);
        }
        SpriteStatus::InProgress => {
            app.set_sprite_ready(false);
            app.set_sprite_loading(true);
            app.set_sprite_frame_count(0);
        }
        SpriteStatus::NotStarted => {
            clear_sprite_preview(app);
        }
    }
}

fn apply_sprite_meta_to_ui(app: &AppWindow, meta: &thumbnails::SpriteMeta, image: slint::Image) {
    app.set_sprite_image(image);
    app.set_sprite_interval_sec(meta.interval_sec as f32);
    app.set_sprite_frame_count(meta.frame_count as i32);
    app.set_sprite_columns(meta.columns as i32);
    app.set_sprite_rows(meta.rows as i32);
    app.set_sprite_thumb_w(meta.thumb_width as i32);
    app.set_sprite_thumb_h(meta.thumb_height as i32);
}

fn apply_list_preview_meta_to_ui(app: &AppWindow, meta: &thumbnails::SpriteMeta, image: slint::Image, col: u32, row: u32) {
    app.set_list_preview_image(image);
    app.set_list_preview_col(col as i32);
    app.set_list_preview_row(row as i32);
    app.set_list_preview_columns(meta.columns as i32);
    app.set_list_preview_rows(meta.rows as i32);
    app.set_list_preview_thumb_w(meta.thumb_width as i32);
    app.set_list_preview_thumb_h(meta.thumb_height as i32);
    app.set_list_preview_visible(true);
}

/// Hides the sidebar list hover preview.
pub fn hide_list_sprite_preview(app: &AppWindow) {
    app.set_list_preview_visible(false);
}

/// Shows a random frame from the cached sprite for `queue_index` — port of
/// `renderSpritePreview` in renderer/app.js (`Math.random() * frame_count`).
pub fn show_list_sprite_preview(app: &AppWindow, state: &mut AppState, queue_index: usize) {
    if state.mode != Mode::Video {
        hide_list_sprite_preview(app);
        return;
    }
    let path = state.queue.item(queue_index).map(|item| item.path.clone());
    let Some(path) = path else {
        hide_list_sprite_preview(app);
        return;
    };
    if !crate::library::is_video_file(&path) {
        hide_list_sprite_preview(app);
        return;
    }
    if state.sprite_status_for(&path) != SpriteStatus::Done {
        hide_list_sprite_preview(app);
        return;
    }
    let Some(hash) = state.sprite_hash_for(&path) else {
        hide_list_sprite_preview(app);
        return;
    };
    let Some((image, meta)) = thumbnails::load_cached_sprite(&hash) else {
        hide_list_sprite_preview(app);
        return;
    };

    use rand::RngExt;
    let frame = if meta.frame_count > 0 {
        rand::rng().random_range(0..meta.frame_count)
    } else {
        0
    };
    let col = frame % meta.columns;
    let row = frame / meta.columns;
    apply_list_preview_meta_to_ui(app, &meta, image, col, row);
}

/// Schedules background sprite generation for `index`, debounced 500ms so
/// rapidly skimming through the queue doesn't kick off a generation job (each
/// one spins up several headless mpv instances) for every item skimmed past
/// — port of the `spriteGenTimer`/`capturedIndex` debounce in `playAt`.
pub fn schedule_sprite_generation(
    app_weak: slint::Weak<AppWindow>,
    state: &Rc<RefCell<AppState>>,
    model: &Rc<VecModel<PlaylistItemData>>,
    sprite_timer: &Rc<slint::Timer>,
    sprite_tx: std::sync::mpsc::Sender<(String, bool)>,
    index: usize,
) {
    let Some(path) = state.borrow().queue.item(index).map(|it| it.path.clone()) else { return };
    // Folder scans can put images in this queue too (Phase 5 will give them
    // their own queue/mode) — sprites are a video-only concept, so skip them
    // rather than running mpv's video encode pipeline against a still image.
    if !crate::library::is_video_file(&path) {
        return;
    }
    let state = Rc::clone(state);
    let model = Rc::clone(model);
    sprite_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(500), move || {
        let Some(app) = app_weak.upgrade() else { return };
        let mut state_ref = state.borrow_mut();
        // Bail if the user has already switched to something else since this
        // was scheduled — matches the original's `if (nowPlaying !== capturedIndex) return`.
        if state_ref.queue.now_playing() != Some(index) {
            return;
        }
        let status = state_ref.sprite_status_for(&path);
        if status == SpriteStatus::Done {
            sync_sprite_preview(&app, &mut state_ref);
            return;
        }
        if status == SpriteStatus::InProgress {
            app.set_sprite_ready(false);
            app.set_sprite_loading(true);
            return;
        }
        let Some(hash) = state_ref.sprite_hash_for(&path) else { return };
        state_ref.sprite_status.insert(hash.clone(), SpriteStatus::InProgress);
        app.set_sprite_ready(false);
        app.set_sprite_loading(true);
        rebuild_playlist_model(&mut state_ref, &model);
        drop(state_ref);

        let tx = sprite_tx.clone();
        let path = path.clone();
        std::thread::spawn(move || {
            let ok = thumbnails::generate_sprite(&path).is_ok();
            let _ = tx.send((hash, ok));
        });
    });
}

/// Applies a background sprite-generation result (from the channel
/// `schedule_sprite_generation`'s worker thread sends to) and refreshes the
/// status icon. On failure, leaves the status at `NotStarted` rather than
/// caching a permanent failure, so a transient error (e.g. a momentarily
/// locked file) can be retried on the next play-through.
pub fn apply_sprite_result(
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    hash: String,
    ok: bool,
) {
    state.sprite_status.insert(hash.clone(), if ok { SpriteStatus::Done } else { SpriteStatus::NotStarted });
    if ok {
        let path = state.queue.now_playing().and_then(|index| {
            state.queue.item(index).map(|item| item.path.clone())
        });
        if let Some(path) = path
            && state.sprite_hash_for(&path).as_deref() == Some(hash.as_str())
        {
            sync_sprite_preview(app, state);
        }
    } else {
        app.set_sprite_loading(false);
    }
    rebuild_playlist_model(state, model);
}
