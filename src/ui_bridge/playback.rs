use super::gif::decode_gif;
use super::sprite_preview::{clear_sprite_preview, hide_list_sprite_preview, sync_sprite_preview};
use super::state::{AppState, Mode};
use super::{log_mpv_err, rebuild_playlist_model};
use crate::library::{MediaKind, media_kind};
use crate::playlist::RemoveOutcome;
use crate::{AppWindow, PlaylistItemData};
use libmpv2::Mpv;
use slint::VecModel;
use std::path::{Path, PathBuf};

fn sync_mode_ui(app: &AppWindow, state: &AppState) {
    app.set_view_mode(match state.mode {
        Mode::Video => 0,
        Mode::Image => 1,
        Mode::All => 2,
    });
}

/// Loads `index` from the video queue: updates state, issues the mpv loadfile
/// command, and syncs the UI.
pub fn play_index(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    index: usize,
) {
    if state.mode != Mode::Video {
        return;
    }
    if state.queue.item(index).is_none() {
        return;
    }
    state.queue.set_now_playing(Some(index));
    let path = state.queue.item(index).unwrap().path.clone();
    log_mpv_err(
        "loadfile",
        mpv.command("loadfile", &[&path.to_string_lossy(), "replace"]),
    );
    log_mpv_err("resume-on-switch", mpv.set_property("pause", false));
    app.set_playing(true);
    sync_sprite_preview(app, state);
    rebuild_playlist_model(state, model);
}

/// All-mode: present a video from `all_queue` via mpv.
fn play_all_video(mpv: &Mpv, app: &AppWindow, state: &mut AppState, path: &Path) {
    state.all_current_is_video = true;
    app.set_current_is_video(true);
    state.gif_animation = None;
    log_mpv_err(
        "loadfile",
        mpv.command("loadfile", &[&path.to_string_lossy(), "replace"]),
    );
    log_mpv_err("resume-on-switch", mpv.set_property("pause", false));
    app.set_playing(true);
    sync_sprite_preview(app, state);
}

/// All-mode: present an image from `all_queue` in the viewer overlay.
fn show_all_image(mpv: &Mpv, app: &AppWindow, state: &mut AppState, path: &Path) {
    state.all_current_is_video = false;
    app.set_current_is_video(false);
    if log_mpv_err("pause-for-all-image", mpv.set_property("pause", true)) {
        app.set_playing(false);
    }

    let is_gif = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    if is_gif {
        match decode_gif(path) {
            Ok(anim) => {
                app.set_current_image(anim.frames[0].0.clone());
                state.gif_animation = Some(anim);
                return;
            }
            Err(err) => eprintln!("failed to decode gif {}: {err}", path.display()),
        }
    }
    state.gif_animation = None;
    match slint::Image::load_from_path(path) {
        Ok(img) => app.set_current_image(img),
        Err(err) => eprintln!("failed to load image {}: {err}", path.display()),
    }
}

/// Presents `index` from `all_queue` — video via mpv or image via overlay.
pub fn present_item(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    index: usize,
) {
    if state.mode != Mode::All {
        return;
    }
    let Some(item) = state.all_queue.item(index) else {
        return;
    };
    let path = item.path.clone();
    state.all_queue.set_now_playing(Some(index));
    state.gallery_open = true;
    app.set_gallery_open(true);
    reset_image_view_transform(app);

    match media_kind(&path) {
        MediaKind::Video => play_all_video(mpv, app, state, &path),
        MediaKind::Image => show_all_image(mpv, app, state, &path),
    }
    sync_all_view_ui(app, state);
    rebuild_playlist_model(state, model);
}

/// Shows `index` from the image queue in the gallery view — image-mode only.
pub fn show_image_at(
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    index: usize,
) {
    if state.image_queue.item(index).is_none() {
        return;
    }
    state.image_queue.set_now_playing(Some(index));
    state.gallery_open = true;
    reset_image_view_transform(app);
    sync_image_viewer_ui(app, state);
    rebuild_playlist_model(state, model);
}

pub fn navigate_image_relative(
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    delta: i32,
) -> bool {
    let next = if delta < 0 {
        state
            .image_queue
            .playable_prev(&state.search_query, state.shuffle_on, state.loop_on)
    } else {
        state
            .image_queue
            .playable_next(&state.search_query, state.shuffle_on, state.loop_on)
    };
    if let Some(idx) = next {
        show_image_at(app, state, model, idx);
        true
    } else {
        false
    }
}

pub fn navigate_all_relative(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    delta: i32,
) -> bool {
    let next = if delta < 0 {
        state
            .all_queue
            .playable_prev(&state.search_query, state.shuffle_on, state.loop_on)
    } else {
        state
            .all_queue
            .playable_next(&state.search_query, state.shuffle_on, state.loop_on)
    };
    if let Some(idx) = next {
        present_item(mpv, app, state, model, idx);
        true
    } else {
        false
    }
}

/// Returns true when the slideshow timer should be running (image on screen).
pub fn all_slideshow_wants_timer(state: &AppState) -> bool {
    state.mode == Mode::All
        && state.slideshow_on
        && !state.all_current_is_video
        && state.gallery_open
}

pub fn toggle_slideshow(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
) {
    if state.mode != Mode::Image && state.mode != Mode::All {
        return;
    }
    state.slideshow_on = !state.slideshow_on;
    app.set_slideshow_on(state.slideshow_on);
    if !state.slideshow_on {
        return;
    }
    match state.mode {
        Mode::Image => {
            if state.image_queue.now_playing().is_none() && !state.image_queue.is_empty() {
                show_image_at(app, state, model, 0);
            }
        }
        Mode::All => {
            if state.all_queue.now_playing().is_none() && !state.all_queue.is_empty() {
                present_item(mpv, app, state, model, 0);
            }
        }
        Mode::Video => {}
    }
}

pub fn set_slideshow_duration(app: &AppWindow, state: &mut AppState, seconds: f64) {
    state.slideshow_duration = seconds.clamp(2.0, 300.0);
    app.set_slideshow_duration_text(format!("{}s", state.slideshow_duration.round() as i64).into());
}

pub fn sync_all_view_ui(app: &AppWindow, state: &mut AppState) {
    let order = state
        .all_queue
        .current_order(&state.search_query, state.shuffle_on);
    let now_playing = state.all_queue.now_playing();
    app.set_gallery_open(state.gallery_open);
    app.set_has_media(!state.all_queue.is_empty());
    app.set_current_is_video(state.all_current_is_video);

    let Some(index) = now_playing else {
        state.gif_animation = None;
        app.set_media_counter_text("".into());
        app.set_media_position(0);
        app.set_media_total(0);
        return;
    };
    if let Some(pos) = order.iter().position(|&i| i == index) {
        app.set_media_counter_text(format!("{} / {}", pos + 1, order.len()).into());
        app.set_media_position((pos + 1) as i32);
        app.set_media_total(order.len() as i32);
    }
}

pub fn sync_image_viewer_ui(app: &AppWindow, state: &mut AppState) {
    let order = state
        .image_queue
        .current_order(&state.search_query, state.shuffle_on);
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
    let Some(item) = state.image_queue.item(index) else {
        return;
    };
    if let Some(pos) = order.iter().position(|&i| i == index) {
        app.set_image_counter_text(format!("{} / {}", pos + 1, order.len()).into());
        app.set_image_position((pos + 1) as i32);
        app.set_image_total(order.len() as i32);
    }

    let is_gif = item
        .path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"));
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

fn reset_image_view_transform(app: &AppWindow) {
    app.set_image_zoom(100.0);
    app.set_image_rotation_deg(0.0);
}

fn remove_from_typed_queues(state: &mut AppState, path: &PathBuf) {
    let _ = state.queue.remove_by_path(path);
    let _ = state.image_queue.remove_by_path(path);
}

fn enqueue_video_paths(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    named_paths: Vec<(String, PathBuf, Option<u64>)>,
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
    } else if state.mode == Mode::Video {
        rebuild_playlist_model(state, model);
    }
    None
}

fn enqueue_image_paths(
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    named_paths: Vec<(String, PathBuf, Option<u64>)>,
) {
    let had_items_before = !state.image_queue.is_empty();
    state.image_queue.enqueue(named_paths);

    if state.image_queue.now_playing().is_none() {
        if !state.image_queue.is_empty() {
            show_image_at(app, state, model, 0);
        }
    } else if state.shuffle_on && had_items_before {
        state
            .image_queue
            .reshuffle_keep_current_first(&state.search_query);
        sync_image_viewer_ui(app, state);
        if state.mode == Mode::Image {
            rebuild_playlist_model(state, model);
        }
    } else if state.mode == Mode::Image {
        rebuild_playlist_model(state, model);
    }
}

fn finish_all_enqueue(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    had_items_before: bool,
) {
    if state.mode != Mode::All {
        return;
    }

    if state.all_queue.now_playing().is_none() {
        if !state.all_queue.is_empty() {
            present_item(mpv, app, state, model, 0);
        }
    } else if state.shuffle_on && had_items_before {
        if let Some(idx) = state.all_queue.reshuffle_jump_to_first(&state.search_query) {
            present_item(mpv, app, state, model, idx);
        }
    } else {
        sync_all_view_ui(app, state);
        rebuild_playlist_model(state, model);
    }
}

pub fn enqueue_paths(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    named_paths: Vec<(String, PathBuf, Option<u64>)>,
) -> Option<usize> {
    let has_video = named_paths
        .iter()
        .any(|(_, p, _)| crate::library::is_video_file(p));
    let has_image = named_paths
        .iter()
        .any(|(_, p, _)| !crate::library::is_video_file(p));

    let had_all_before = !state.all_queue.is_empty();
    state.all_queue.enqueue(
        named_paths
            .iter()
            .map(|(n, p, s)| (n.clone(), p.clone(), *s)),
    );

    let (videos, images): (Vec<_>, Vec<_>) = named_paths
        .into_iter()
        .partition(|(_, p, _)| crate::library::is_video_file(p));

    if has_video && has_image {
        set_mode(mpv, app, state, model, Mode::All);
    } else if has_image && !has_video {
        set_mode(mpv, app, state, model, Mode::Image);
    } else if has_video && !has_image {
        set_mode(mpv, app, state, model, Mode::Video);
    }

    let played_index = if !videos.is_empty() {
        enqueue_video_paths(mpv, app, state, model, videos)
    } else {
        None
    };
    if !images.is_empty() {
        enqueue_image_paths(app, state, model, images);
    }
    finish_all_enqueue(mpv, app, state, model, had_all_before);
    played_index
}

pub fn set_mode(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    mode: Mode,
) {
    if state.mode == mode {
        return;
    }
    let leaving_video = state.mode == Mode::Video && mode != Mode::Video;
    if leaving_video && log_mpv_err("auto-pause on mode switch", mpv.set_property("pause", true)) {
        app.set_playing(false);
    }
    if mode == Mode::Video {
        if state.slideshow_on {
            state.slideshow_on = false;
            app.set_slideshow_on(false);
        }
        if let Some(timer) = &state.slideshow_timer {
            timer.stop();
        }
    }
    state.mode = mode;
    sync_mode_ui(app, state);

    match mode {
        Mode::Image => {
            clear_sprite_preview(app);
            hide_list_sprite_preview(app);
            sync_image_viewer_ui(app, state);
        }
        Mode::Video => {
            clear_sprite_preview(app);
            hide_list_sprite_preview(app);
        }
        Mode::All => {
            clear_sprite_preview(app);
            hide_list_sprite_preview(app);
            if state.all_queue.now_playing().is_none() && !state.all_queue.is_empty() {
                present_item(mpv, app, state, model, 0);
            } else if let Some(idx) = state.all_queue.now_playing() {
                present_item(mpv, app, state, model, idx);
            } else {
                sync_all_view_ui(app, state);
            }
        }
    }
    rebuild_playlist_model(state, model);
}

pub fn remove_item(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    index: usize,
) {
    match state.mode {
        Mode::Video => match state.queue.remove_at(index) {
            RemoveOutcome::QueueEmpty => {
                let _ = mpv.command("stop", &[]);
                app.set_playing(false);
                rebuild_playlist_model(state, model);
            }
            RemoveOutcome::NowPlayingChanged(new_index) => {
                play_index(mpv, app, state, model, new_index)
            }
            RemoveOutcome::NoPlaybackChange => rebuild_playlist_model(state, model),
        },
        Mode::Image => match state.image_queue.remove_at(index) {
            RemoveOutcome::QueueEmpty => {
                state.gallery_open = false;
                sync_image_viewer_ui(app, state);
                rebuild_playlist_model(state, model);
            }
            RemoveOutcome::NowPlayingChanged(new_index) => {
                show_image_at(app, state, model, new_index)
            }
            RemoveOutcome::NoPlaybackChange => {
                sync_image_viewer_ui(app, state);
                rebuild_playlist_model(state, model);
            }
        },
        Mode::All => {
            let path = state.all_queue.item(index).map(|item| item.path.clone());
            let Some(path) = path else {
                return;
            };
            match state.all_queue.remove_at(index) {
                RemoveOutcome::QueueEmpty => {
                    remove_from_typed_queues(state, &path);
                    let _ = mpv.command("stop", &[]);
                    app.set_playing(false);
                    state.gallery_open = false;
                    sync_all_view_ui(app, state);
                    rebuild_playlist_model(state, model);
                }
                RemoveOutcome::NowPlayingChanged(new_index) => {
                    remove_from_typed_queues(state, &path);
                    present_item(mpv, app, state, model, new_index);
                }
                RemoveOutcome::NoPlaybackChange => {
                    remove_from_typed_queues(state, &path);
                    sync_all_view_ui(app, state);
                    rebuild_playlist_model(state, model);
                }
            }
        }
    }
}

pub fn reorder_item(
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    src: usize,
    dst: usize,
) {
    if src == dst {
        return;
    }
    let effective_dst = if dst > src { dst - 1 } else { dst };
    match state.mode {
        Mode::Video => state.queue.move_item(src, effective_dst),
        Mode::Image => state.image_queue.move_item(src, effective_dst),
        Mode::All => state.all_queue.move_item(src, effective_dst),
    }
    rebuild_playlist_model(state, model);
}
