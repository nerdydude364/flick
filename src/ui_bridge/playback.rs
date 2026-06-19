use super::gif::decode_gif;
use super::sprite_preview::{clear_sprite_preview, hide_list_sprite_preview, sync_sprite_preview};
use super::state::{AppState, Mode};
use super::{log_mpv_err, rebuild_playlist_model};
use crate::playlist::RemoveOutcome;
use crate::{AppWindow, PlaylistItemData};
use libmpv2::Mpv;
use slint::VecModel;
use std::path::PathBuf;

/// Loads `index` from the queue: updates state, issues the mpv loadfile
/// command, and syncs the UI. Port of the playback-triggering half of
/// `playAt` in app.js (sprite generation is Phase 3, not ported here).
pub fn play_index(mpv: &Mpv, app: &AppWindow, state: &mut AppState, model: &VecModel<PlaylistItemData>, index: usize) {
    // Hard guard: never start/touch video playback while in image mode.
    // Without this, e.g. loading a mixed video+image folder while browsing
    // images silently started a video playing in the background (caught via
    // manual testing) — autoplay-if-nothing-was-playing logic in
    // `enqueue_video_paths` doesn't know or care what mode is currently active.
    if state.mode != Mode::Video {
        return;
    }
    if state.queue.item(index).is_none() {
        return;
    }
    state.queue.set_now_playing(Some(index));
    let path = state.queue.item(index).unwrap().path.clone();
    log_mpv_err("loadfile", mpv.command("loadfile", &[&path.to_string_lossy(), "replace"]));
    // loadfile alone doesn't force a resume — if mpv was paused (e.g. parked at
    // EOF via keep-open=yes), it stays paused on the new file too, desyncing
    // the UI from actual playback state. Force it, matching the original's
    // unconditional `videoEl.play()` on every track switch.
    log_mpv_err("resume-on-switch", mpv.set_property("pause", false));
    app.set_playing(true);
    sync_sprite_preview(app, state);
    rebuild_playlist_model(state, model);
}

/// Shows `index` from the image queue in the gallery view — image-mode
/// equivalent of `play_index`, port of `showImageAt`.
pub fn show_image_at(app: &AppWindow, state: &mut AppState, model: &VecModel<PlaylistItemData>, index: usize) {
    if state.image_queue.item(index).is_none() {
        return;
    }
    state.image_queue.set_now_playing(Some(index));
    state.gallery_open = true;
    reset_image_view_transform(app);
    sync_image_viewer_ui(app, state);
    rebuild_playlist_model(state, model);
}

/// Gallery prev/next — port of `doNavigate`. Reuses `Queue::playable_next`/
/// `playable_prev`, the exact same filtered+shuffled-order logic the video
/// queue uses for prev/next, since `getActiveImageIndices` in the original
/// is the same "filtered, then reordered by shuffle" computation as
/// `currentOrderList` — just applied to the image queue.
pub fn navigate_image_relative(
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    delta: i32,
) -> bool {
    let next = if delta < 0 {
        state.image_queue.playable_prev(&state.search_query, state.shuffle_on, state.loop_on)
    } else {
        state.image_queue.playable_next(&state.search_query, state.shuffle_on, state.loop_on)
    };
    if let Some(idx) = next {
        show_image_at(app, state, model, idx);
        true
    } else {
        false
    }
}

/// Port of the slideshow toggle button's click handler. Doesn't itself
/// start/stop a timer (main.rs owns that) — just flips the flag and, if the
/// gallery wasn't open yet, jumps to the first image so there's something to
/// show, matching `slideshowBtn`'s `if (currentIdx < 0 ...) onNavigate(0)`.
pub fn toggle_slideshow(app: &AppWindow, state: &mut AppState, model: &VecModel<PlaylistItemData>) {
    // Only allow toggling slideshow while in Image mode.
    if state.mode != Mode::Image {
        return;
    }
    state.slideshow_on = !state.slideshow_on;
    app.set_slideshow_on(state.slideshow_on);
    if state.slideshow_on && state.image_queue.now_playing().is_none() && !state.image_queue.is_empty() {
        show_image_at(app, state, model, 0);
    }
}

pub fn set_slideshow_duration(app: &AppWindow, state: &mut AppState, seconds: f64) {
    state.slideshow_duration = seconds.clamp(2.0, 300.0);
    app.set_slideshow_duration_text(format!("{}s", state.slideshow_duration.round() as i64).into());
}

/// Pushes the current image-queue state to the AppWindow's display
/// properties: which image to show, its name, and the "N / total" counter.
/// Decodes and starts an animation if the image is a GIF.
pub fn sync_image_viewer_ui(app: &AppWindow, state: &mut AppState) {
    let order = state.image_queue.current_order(&state.search_query, state.shuffle_on);
    let now_playing = state.image_queue.now_playing();
    app.set_gallery_open(state.gallery_open);
    app.set_has_images(!state.image_queue.is_empty());

    let Some(index) = now_playing else {
        state.gif_animation = None;
        app.set_image_counter_text("".into());
        app.set_image_position(0);
        app.set_image_total(0);
        reset_image_view_transform(app);
        return;
    };
    let Some(item) = state.image_queue.item(index) else { return };
    if let Some(pos) = order.iter().position(|&i| i == index) {
        app.set_image_counter_text(format!("{} / {}", pos + 1, order.len()).into());
        app.set_image_position((pos + 1) as i32);
        app.set_image_total(order.len() as i32);
    }

    let is_gif = item.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    if is_gif {
        match decode_gif(&item.path) {
            Ok(anim) => {
                app.set_current_image(anim.frames[0].0.clone());
                state.gif_animation = Some(anim);
                return;
            }
            Err(err) => eprintln!("failed to decode gif {}: {err}", item.path.display()),
        }
    }
    state.gif_animation = None;
    match slint::Image::load_from_path(&item.path) {
        Ok(img) => app.set_current_image(img),
        Err(err) => eprintln!("failed to load image {}: {err}", item.path.display()),
    }
}

/// Resets zoom/rotation when switching images — transforms are per-image view state.
fn reset_image_view_transform(app: &AppWindow) {
    app.set_image_zoom(100.0);
    app.set_image_rotation_deg(0.0);
}

/// Adds `paths` (already name-resolved) to the video queue and runs the same
/// post-add orchestration `enqueue()` does in app.js: autoplay if nothing
/// was playing yet, or fold new items into the shuffle order (and jump to
/// the new first item) if shuffle was already on. Returns the index that was
/// just auto-played, if any, so the caller can schedule sprite generation
/// for it — that needs the `Rc<RefCell<AppState>>` this function doesn't
/// have (it only ever sees the already-borrowed `&mut AppState`), so it's
/// the caller's job, same as every other play_index call site.
fn enqueue_video_paths(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    named_paths: Vec<(String, PathBuf)>,
) -> Option<usize> {
    let had_items_before = !state.queue.is_empty();
    state.queue.enqueue(named_paths);

    if state.queue.now_playing().is_none() {
        if !state.queue.is_empty() {
            play_index(mpv, app, state, model, 0);
            return Some(0);
        }
    } else if state.shuffle_on && had_items_before {
        if let Some(idx) = state.queue.reshuffle_jump_to_first(&state.search_query) {
            play_index(mpv, app, state, model, idx);
            return Some(idx);
        }
    } else {
        rebuild_playlist_model(state, model);
    }
    None
}

/// Image-queue equivalent of `enqueue_video_paths` — port of `enqueue()`'s
/// image-handling half: show the first image if nothing was showing, or
/// `reshuffleImages()`'s keep-current-first behavior if shuffle was already on.
fn enqueue_image_paths(
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    named_paths: Vec<(String, PathBuf)>,
) {
    let had_items_before = !state.image_queue.is_empty();
    state.image_queue.enqueue(named_paths);

    if state.image_queue.now_playing().is_none() {
        if !state.image_queue.is_empty() {
            show_image_at(app, state, model, 0);
        }
    } else if state.shuffle_on && had_items_before {
        state.image_queue.reshuffle_keep_current_first(&state.search_query);
        sync_image_viewer_ui(app, state);
        rebuild_playlist_model(state, model);
    } else {
        rebuild_playlist_model(state, model);
    }
}

/// Splits `paths` by media kind and routes each to its queue — port of
/// `enqueue()`'s dispatch loop plus its auto-switch-mode logic ("if all
/// newly-added files are one type, switch to that mode"). Returns the video
/// queue index that was just auto-played, if any — see
/// `enqueue_video_paths`'s doc comment for why the caller has to be the one
/// to schedule sprite generation for it.
pub fn enqueue_paths(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    named_paths: Vec<(String, PathBuf)>,
) -> Option<usize> {
    let (videos, images): (Vec<_>, Vec<_>) =
        named_paths.into_iter().partition(|(_, p)| crate::library::is_video_file(p));

    if !images.is_empty() && videos.is_empty() {
        set_mode(mpv, app, state, model, Mode::Image);
    } else if !videos.is_empty() && images.is_empty() {
        set_mode(mpv, app, state, model, Mode::Video);
    }

    let played_index = if !videos.is_empty() { enqueue_video_paths(mpv, app, state, model, videos) } else { None };
    if !images.is_empty() {
        enqueue_image_paths(app, state, model, images);
    }
    played_index
}

/// Port of `switchMode` (minus the title-bar update, which this app doesn't
/// have yet) — plus auto-pausing video when leaving video mode, since
/// nothing shows it anymore and it would otherwise keep decoding/playing
/// audio in the background (confirmed via manual testing this was confusing,
/// not present in the original since the original never explicitly asked for
/// it either way, so this is a deliberate native-app improvement).
pub fn set_mode(mpv: &Mpv, app: &AppWindow, state: &mut AppState, model: &VecModel<PlaylistItemData>, mode: Mode) {
    if state.mode == mode {
        return;
    }
    if state.mode == Mode::Video
        && mode == Mode::Image
        && log_mpv_err("auto-pause on mode switch", mpv.set_property("pause", true))
    {
        app.set_playing(false);
    }
    state.mode = mode;
    // Enforce that slideshow is always off in video mode.
    if state.mode == Mode::Video {
        if state.slideshow_on {
            state.slideshow_on = false;
            app.set_slideshow_on(false);
        }
        if let Some(timer) = &state.slideshow_timer {
            timer.stop();
        }
    }
    app.set_image_mode(mode == Mode::Image);
    if mode == Mode::Image {
        clear_sprite_preview(app);
        hide_list_sprite_preview(app);
    }
    sync_image_viewer_ui(app, state);
    rebuild_playlist_model(state, model);
}

/// Removes `index` from whichever queue is active — port of the
/// orchestration half of `removeVideoAt`/`removeImageAt` (the
/// data-structure half, shared by both, is `Queue::remove_at`).
pub fn remove_item(mpv: &Mpv, app: &AppWindow, state: &mut AppState, model: &VecModel<PlaylistItemData>, index: usize) {
    if state.mode == Mode::Video {
        match state.queue.remove_at(index) {
            RemoveOutcome::QueueEmpty => {
                let _ = mpv.command("stop", &[]);
                app.set_playing(false);
                rebuild_playlist_model(state, model);
            }
            RemoveOutcome::NowPlayingChanged(new_index) => play_index(mpv, app, state, model, new_index),
            RemoveOutcome::NoPlaybackChange => rebuild_playlist_model(state, model),
        }
    } else {
        match state.image_queue.remove_at(index) {
            RemoveOutcome::QueueEmpty => {
                state.gallery_open = false;
                sync_image_viewer_ui(app, state);
                rebuild_playlist_model(state, model);
            }
            RemoveOutcome::NowPlayingChanged(new_index) => show_image_at(app, state, model, new_index),
            RemoveOutcome::NoPlaybackChange => {
                sync_image_viewer_ui(app, state);
                rebuild_playlist_model(state, model);
            }
        }
    }
}

/// Reorders whichever queue is active after a drag gesture. `dst` is the
/// *visual* row index the item was dropped on (not yet adjusted for
/// removal) — port of the drop handler's `effectiveDst = dst > src ? dst -
/// 1 : dst` adjustment, which `Queue::move_item` itself expects already applied.
pub fn reorder_item(state: &mut AppState, model: &VecModel<PlaylistItemData>, src: usize, dst: usize) {
    if src == dst {
        return;
    }
    let effective_dst = if dst > src { dst - 1 } else { dst };
    if state.mode == Mode::Video {
        state.queue.move_item(src, effective_dst);
    } else {
        state.image_queue.move_item(src, effective_dst);
    }
    rebuild_playlist_model(state, model);
}
