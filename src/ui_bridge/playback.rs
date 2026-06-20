use super::gallery::GalleryContext;
use super::gif::decode_gif;
use super::sprite_preview::{clear_sprite_preview, hide_list_sprite_preview, sync_sprite_preview};
use super::state::{AppState, Mode};
use super::loading::{rebuild_playlist_model, set_library_loading, sync_loading_ui};
use super::{log_mpv_err};
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

/// Image mode never drives mpv — stop decoding/audio so a prior video session
/// cannot continue underneath the image viewer.
pub fn stop_mpv_for_image_mode(mpv: &Mpv, app: &AppWindow) {
    log_mpv_err("stop-for-image-mode", mpv.command("stop", &[]));
    app.set_playing(false);
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
    state.gallery_open = true;
    app.set_gallery_open(true);
    let path = state.queue.item(index).unwrap().path.clone();
    log_mpv_err(
        "loadfile",
        mpv.command("loadfile", &[&path.to_string_lossy(), "replace"]),
    );
    log_mpv_err("resume-on-switch", mpv.set_property("pause", false));
    app.set_playing(true);
    sync_sprite_preview(app, state);
    sync_video_view_ui(app, state);
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
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    index: usize,
) {
    if state.mode != Mode::Image {
        return;
    }
    if state.image_queue.item(index).is_none() {
        return;
    }
    stop_mpv_for_image_mode(mpv, app);
    state.image_queue.set_now_playing(Some(index));
    state.gallery_open = true;
    reset_image_view_transform(app);
    sync_image_viewer_ui(app, state);
    rebuild_playlist_model(state, model);
}

pub fn navigate_image_relative(
    mpv: &Mpv,
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
        show_image_at(mpv, app, state, model, idx);
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

/// Outcome of auto-advancing when mpv reports natural end-of-file.
pub struct VideoEofAdvance {
    /// Queue index when the new item is a video (for sprite scheduling).
    pub video_index: Option<usize>,
    /// Restart the image slideshow timer (All mode landed on an image).
    pub restart_slideshow_timer: bool,
}

/// Advance to the next playable item after a video finishes. Uses the video
/// queue in Video mode and `all_queue` in All mode (respecting search/shuffle/loop).
pub fn advance_on_video_eof(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
) -> VideoEofAdvance {
    match state.mode {
        Mode::Video => {
            let Some(idx) = state.queue.playable_next(
                &state.search_query,
                state.shuffle_on,
                state.loop_on,
            ) else {
                return VideoEofAdvance {
                    video_index: None,
                    restart_slideshow_timer: false,
                };
            };
            play_index(mpv, app, state, model, idx);
            VideoEofAdvance {
                video_index: Some(idx),
                restart_slideshow_timer: false,
            }
        }
        Mode::All if state.all_current_is_video => {
            let slideshow_on = state.slideshow_on;
            let advanced = navigate_all_relative(mpv, app, state, model, 1);
            VideoEofAdvance {
                video_index: if state.all_current_is_video {
                    state.all_queue.now_playing()
                } else {
                    None
                },
                restart_slideshow_timer: advanced && slideshow_on && all_slideshow_wants_timer(state),
            }
        }
        _ => VideoEofAdvance {
            video_index: None,
            restart_slideshow_timer: false,
        },
    }
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
                show_image_at(mpv, app, state, model, 0);
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

pub fn sync_active_view_ui(app: &AppWindow, state: &mut AppState) {
    match state.mode {
        Mode::Video => sync_video_view_ui(app, state),
        Mode::Image => sync_image_viewer_ui(app, state),
        Mode::All => sync_all_view_ui(app, state),
    }
}

pub fn sync_video_view_ui(app: &AppWindow, state: &AppState) {
    let order = state
        .queue
        .current_order(&state.search_query, state.shuffle_on);
    let now_playing = state.queue.now_playing();
    app.set_gallery_open(state.gallery_open);
    app.set_has_videos(!state.queue.is_empty());

    let Some(index) = now_playing else {
        app.set_video_counter_text("".into());
        app.set_video_position(0);
        app.set_video_total(0);
        return;
    };
    if let Some(pos) = order.iter().position(|&i| i == index) {
        app.set_video_counter_text(format!("{} / {}", pos + 1, order.len()).into());
        app.set_video_position((pos + 1) as i32);
        app.set_video_total(order.len() as i32);
    }
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
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    named_paths: Vec<(String, PathBuf, Option<u64>)>,
) {
    state.queue.enqueue(named_paths);
    if state.mode == Mode::Video {
        rebuild_playlist_model(state, model);
    }
}

fn enqueue_image_paths(
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    named_paths: Vec<(String, PathBuf, Option<u64>)>,
) {
    state.image_queue.enqueue(named_paths);
    if state.mode == Mode::Image {
        rebuild_playlist_model(state, model);
    }
}

fn finish_import_gallery(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    gallery: &GalleryContext<'_>,
) {
    if state.all_queue.is_empty() {
        return;
    }
    super::gallery::open_gallery_grid(mpv, app, state, gallery, super::gallery::GalleryReload::Force);
    sync_active_view_ui(app, state);
    super::loading::schedule_playlist_rebuild(state, model);
    sync_loading_ui(app, state);
}

pub fn enqueue_paths(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    named_paths: Vec<(String, PathBuf, Option<u64>)>,
    gallery: &GalleryContext<'_>,
) {
    let count = named_paths.len();
    if count > 0 {
        state.library_loading = true;
        set_library_loading(app, true, &format!("Adding {count} items…"));
    }

    state.all_queue.enqueue(
        named_paths
            .iter()
            .map(|(n, p, s)| (n.clone(), p.clone(), *s)),
    );

    let (videos, images): (Vec<_>, Vec<_>) = named_paths
        .into_iter()
        .partition(|(_, p, _)| crate::library::is_video_file(p));

    enqueue_video_paths(state, model, videos);
    enqueue_image_paths(state, model, images);

    if state.mode != Mode::All {
        set_mode(mpv, app, state, model, Mode::All, None);
    }
    finish_import_gallery(mpv, app, state, model, gallery);

    state.library_loading = false;
    sync_loading_ui(app, state);
}

pub fn set_mode(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    mode: Mode,
    gallery: Option<&GalleryContext<'_>>,
) {
    if state.mode == mode {
        return;
    }
    let leaving_video = state.mode == Mode::Video && mode != Mode::Video;
    if mode == Mode::Image {
        stop_mpv_for_image_mode(mpv, app);
    } else if leaving_video
        && log_mpv_err("auto-pause on mode switch", mpv.set_property("pause", true))
    {
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

    clear_sprite_preview(app);
    hide_list_sprite_preview(app);
    enter_mode_view(mpv, app, state, model, gallery, mode);
    rebuild_playlist_model(state, model);
}

fn enter_mode_view(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    gallery: Option<&GalleryContext<'_>>,
    mode: Mode,
) {
    match mode {
        Mode::Image => {
            if let Some(idx) = state.image_queue.now_playing() {
                show_image_at(mpv, app, state, model, idx);
            } else if !state.image_queue.is_empty() {
                show_gallery_grid(mpv, app, state, gallery);
                sync_image_viewer_ui(app, state);
            } else {
                sync_image_viewer_ui(app, state);
            }
        }
        Mode::Video => {
            if let Some(idx) = state.queue.now_playing() {
                play_index(mpv, app, state, model, idx);
            } else if !state.queue.is_empty() {
                show_gallery_grid(mpv, app, state, gallery);
                sync_video_view_ui(app, state);
            } else {
                sync_video_view_ui(app, state);
            }
        }
        Mode::All => {
            if let Some(idx) = state.all_queue.now_playing() {
                present_item(mpv, app, state, model, idx);
            } else if !state.all_queue.is_empty() {
                show_gallery_grid(mpv, app, state, gallery);
                sync_all_view_ui(app, state);
            } else {
                sync_all_view_ui(app, state);
            }
        }
    }
}

fn show_gallery_grid(
    mpv: &Mpv,
    app: &AppWindow,
    state: &mut AppState,
    gallery: Option<&GalleryContext<'_>>,
) {
    if let Some(gallery) = gallery {
        super::gallery::open_gallery_grid(mpv, app, state, gallery, super::gallery::GalleryReload::Force);
    } else {
        state.gallery_open = false;
        app.set_gallery_open(false);
    }
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
                state.gallery_open = false;
                sync_video_view_ui(app, state);
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
                show_image_at(mpv, app, state, model, new_index)
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
