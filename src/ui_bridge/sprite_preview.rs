use super::loading::patch_sprite_status_for_hash;
use super::state::{AppState, Mode, SpriteStatus};
use crate::thumbnails;
use crate::{AppWindow, PlaylistItemData};
use slint::VecModel;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// Caps how many *different* videos can be generating sprites at once.
/// Each `generate_sprite` call already saturates several cores on its own
/// (up to `thumbnails::sprite`'s internal `CONCURRENCY` headless mpv
/// instances extracting frames in parallel), so this bounds cross-video
/// fan-out rather than per-video work — without it, skimming through
/// several videos a beat apart (each past the 500ms debounce below) could
/// pile up many concurrent generations and overload the machine.
const MAX_PARALLEL_SPRITE_JOBS: usize = 2;

/// Runs `generate_sprite` on a worker thread and reports the result back
/// over `tx`. Once spawned, this always runs to completion — there's no
/// cancellation, by design: navigating away from the video mid-generation
/// must not waste the work already done or leave it to restart later.
fn spawn_sprite_worker(tx: std::sync::mpsc::Sender<(String, bool)>, hash: String, path: PathBuf) {
    std::thread::spawn(move || {
        // Caught rather than left to unwind: an escaping panic here would
        // kill this thread before `tx.send` runs, leaking one of only
        // `MAX_PARALLEL_SPRITE_JOBS` concurrent slots forever (since
        // `apply_sprite_result`, which decrements `sprite_active`, would
        // never see this job complete).
        let ok = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            thumbnails::generate_sprite(&path)
        })) {
            Ok(Ok(())) => true,
            Ok(Err(err)) => {
                crate::flick_debug!("[sprite] generate failed {}: {err}", path.display());
                false
            }
            Err(_) => {
                crate::flick_debug!("[sprite] generate panicked {}", path.display());
                false
            }
        };
        if tx.send((hash, ok)).is_err() {
            crate::flick_debug!(
                "[sprite] result channel closed for {} (ok={ok})",
                path.display()
            );
        }
    });
}

pub fn current_video_path(state: &AppState) -> Option<std::path::PathBuf> {
    match state.mode {
        Mode::Video => state
            .queue
            .now_playing()
            .and_then(|index| state.queue.item(index).map(|item| item.path.clone())),
        Mode::All if state.all_current_is_video => state
            .all_queue
            .now_playing()
            .and_then(|index| state.all_queue.item(index).map(|item| item.path.clone())),
        _ => None,
    }
}

/// Clears progress-bar scrub preview state (no sprite sheet loaded).
pub fn clear_sprite_preview(app: &AppWindow) {
    app.set_sprite_ready(false);
    app.set_sprite_loading(false);
    app.set_sprite_frame_count(0);
}

/// Pushes the cached sprite sheet for the currently playing video (if any)
/// into the AppWindow properties that drive the progress-bar hover preview.
pub fn sync_sprite_preview(app: &AppWindow, state: &mut AppState) {
    let Some(path) = current_video_path(state) else {
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
                if let Some(path) = current_video_path(state) {
                    crate::flick_debug!(
                        "[sprite] preview load failed {} hash {hash}",
                        path.display()
                    );
                } else {
                    crate::flick_debug!("[sprite] preview load failed hash {hash}");
                }
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

fn sprite_meta_from_app(app: &AppWindow) -> thumbnails::SpriteMeta {
    thumbnails::SpriteMeta {
        interval_sec: app.get_sprite_interval_sec() as f64,
        frame_count: app.get_sprite_frame_count() as u32,
        columns: app.get_sprite_columns() as u32,
        rows: app.get_sprite_rows() as u32,
        thumb_width: app.get_sprite_thumb_w() as u32,
        thumb_height: app.get_sprite_thumb_h() as u32,
    }
}

fn now_playing_index(state: &AppState) -> Option<usize> {
    match state.mode {
        Mode::Video => state.queue.now_playing(),
        Mode::All => state.all_queue.now_playing(),
        Mode::Image => None,
    }
}

fn load_sprite_for_list_preview(
    app: &AppWindow,
    state: &AppState,
    queue_index: usize,
    hash: &str,
) -> Option<(slint::Image, thumbnails::SpriteMeta)> {
    if now_playing_index(state) == Some(queue_index) && app.get_sprite_ready() {
        return Some((app.get_sprite_image(), sprite_meta_from_app(app)));
    }
    thumbnails::load_cached_sprite(hash)
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

fn apply_list_preview_meta_to_ui(
    app: &AppWindow,
    meta: &thumbnails::SpriteMeta,
    image: slint::Image,
    col: u32,
    row: u32,
) {
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
    if state.mode != Mode::Video && state.mode != Mode::All {
        hide_list_sprite_preview(app);
        return;
    }
    let path = match state.mode {
        Mode::Video => state.queue.item(queue_index).map(|item| item.path.clone()),
        Mode::All => state
            .all_queue
            .item(queue_index)
            .map(|item| item.path.clone()),
        Mode::Image => None,
    };
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
    let Some((image, meta)) = load_sprite_for_list_preview(app, state, queue_index, &hash) else {
        crate::flick_debug!(
            "[sprite] list preview load failed {} hash {hash}",
            path.display()
        );
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
    let state_ref = state.borrow();
    let (now_playing, path) = match state_ref.mode {
        Mode::Video => (
            state_ref.queue.now_playing(),
            state_ref.queue.item(index).map(|it| it.path.clone()),
        ),
        Mode::All => (
            state_ref.all_queue.now_playing(),
            state_ref.all_queue.item(index).map(|it| it.path.clone()),
        ),
        Mode::Image => (None, None),
    };
    drop(state_ref);
    let Some(path) = path else {
        return;
    };
    // Folder scans can put images in this queue too (Phase 5 will give them
    // their own queue/mode) — sprites are a video-only concept, so skip them
    // rather than running mpv's video encode pipeline against a still image.
    if !crate::library::is_video_file(&path) {
        return;
    }
    let state = Rc::clone(state);
    let model = Rc::clone(model);
    sprite_timer.start(
        slint::TimerMode::SingleShot,
        std::time::Duration::from_millis(500),
        move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut state_ref = state.borrow_mut();
            // Bail if the user has already switched to something else since this
            // was scheduled — matches the original's `if (nowPlaying !== capturedIndex) return`.
            if now_playing != Some(index) {
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
            let Some(hash) = state_ref.sprite_hash_for(&path) else {
                return;
            };
            state_ref
                .sprite_status
                .insert(hash.clone(), SpriteStatus::InProgress);
            app.set_sprite_ready(false);
            app.set_sprite_loading(true);
            patch_sprite_status_for_hash(&mut state_ref, &model, &hash, SpriteStatus::InProgress);

            // Marked InProgress either way, so a video re-triggering
            // generation while this is still queued is a no-op above
            // rather than a duplicate enqueue.
            if state_ref.sprite_active < MAX_PARALLEL_SPRITE_JOBS {
                state_ref.sprite_active += 1;
                drop(state_ref);
                spawn_sprite_worker(sprite_tx.clone(), hash, path.clone());
            } else {
                state_ref.sprite_queue.push_back((hash, path.clone()));
            }
        },
    );
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
    sprite_tx: &std::sync::mpsc::Sender<(String, bool)>,
    hash: String,
    ok: bool,
) {
    let status = if ok {
        SpriteStatus::Done
    } else {
        SpriteStatus::NotStarted
    };
    state.sprite_status.insert(hash.clone(), status);
    if ok {
        let path = current_video_path(state);
        if path.is_some()
            && state.sprite_hash_for(path.as_ref().unwrap()).as_deref() == Some(hash.as_str())
        {
            sync_sprite_preview(app, state);
        }
    } else {
        app.set_sprite_loading(false);
    }
    patch_sprite_status_for_hash(state, model, &hash, status);

    // A slot just freed up — start whatever's been waiting longest. Loops
    // rather than starting just one, in case `MAX_PARALLEL_SPRITE_JOBS`
    // ever grows beyond 1 more than what was already running.
    state.sprite_active = state.sprite_active.saturating_sub(1);
    while state.sprite_active < MAX_PARALLEL_SPRITE_JOBS {
        let Some((next_hash, next_path)) = state.sprite_queue.pop_front() else {
            break;
        };
        state.sprite_active += 1;
        spawn_sprite_worker(sprite_tx.clone(), next_hash, next_path);
    }
}

/// Convenience wrapper for call sites that just resumed/advanced playback
/// (mode switch, remove-item, slideshow, shuffle) and want sprite
/// generation to begin the same as any other video-play event, without
/// each one having to dig the right index out of the right queue: resolves
/// whatever the active queue currently has as `now_playing` and forwards
/// to `schedule_sprite_generation`, which already no-ops for image mode or
/// a non-video path. Does nothing if nothing is playing.
pub fn schedule_sprite_generation_for_now_playing(
    app_weak: slint::Weak<AppWindow>,
    state: &Rc<RefCell<AppState>>,
    model: &Rc<VecModel<PlaylistItemData>>,
    sprite_timer: &Rc<slint::Timer>,
    sprite_tx: std::sync::mpsc::Sender<(String, bool)>,
) {
    let index = {
        let state_ref = state.borrow();
        match state_ref.mode {
            Mode::Video => state_ref.queue.now_playing(),
            Mode::All => state_ref.all_queue.now_playing(),
            Mode::Image => None,
        }
    };
    if let Some(index) = index {
        schedule_sprite_generation(app_weak, state, model, sprite_timer, sprite_tx, index);
    }
}
