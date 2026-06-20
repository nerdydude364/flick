mod ab_loop;
mod gallery;
mod gif;
mod playback;
mod sprite_preview;
mod state;

pub use ab_loop::{handle_progress_click, toggle_ab_loop};
pub use gallery::{apply_gallery_thumb, toggle_gallery, GalleryContext, GalleryThumbResult};
pub use gif::tick_gif_animation;
pub use playback::{
    advance_on_video_eof, all_slideshow_wants_timer, enqueue_paths, navigate_all_relative,
    navigate_image_relative, play_index, present_item, remove_item, reorder_item, set_mode,
    set_slideshow_duration, show_image_at, sync_active_view_ui, sync_all_view_ui,
    sync_image_viewer_ui, sync_video_view_ui, toggle_slideshow,
};
pub use sprite_preview::{
    apply_sprite_result, hide_list_sprite_preview, schedule_sprite_generation,
    show_list_sprite_preview,
};
pub use state::{AppState, Mode, SpriteStatus};

use crate::PlaylistItemData;
use crate::library::{MediaKind, media_kind};
use slint::VecModel;
use std::path::Path;

pub(crate) fn log_mpv_err<T>(label: &str, result: Result<T, libmpv2::Error>) -> bool {
    match result {
        Ok(_) => true,
        Err(err) => {
            eprintln!("{label} failed: {err}");
            false
        }
    }
}

pub fn basename(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn sprite_status_glyph(status: SpriteStatus) -> &'static str {
    match status {
        SpriteStatus::NotStarted => "-",
        SpriteStatus::InProgress => "⏳",
        SpriteStatus::Done => "✓",
    }
}

pub fn rebuild_playlist_model(state: &mut AppState, model: &VecModel<PlaylistItemData>) {
    let (filtered, now_playing, show_sprite_status) = match state.mode {
        Mode::Video => (
            state.queue.filtered_indices(&state.search_query),
            state.queue.now_playing(),
            true,
        ),
        Mode::Image => (
            state.image_queue.filtered_indices(&state.search_query),
            state.image_queue.now_playing(),
            false,
        ),
        Mode::All => (
            state.all_queue.filtered_indices(&state.search_query),
            state.all_queue.now_playing(),
            false,
        ),
    };

    let rows: Vec<PlaylistItemData> = filtered
        .into_iter()
        .map(|i| {
            let item = match state.mode {
                Mode::Video => state.queue.item(i),
                Mode::Image => state.image_queue.item(i),
                Mode::All => state.all_queue.item(i),
            }
            .expect("valid index")
            .clone();
            let is_video = media_kind(&item.path) == MediaKind::Video;
            let glyph = if show_sprite_status || (state.mode == Mode::All && is_video) {
                sprite_status_glyph(state.sprite_status_for(&item.path))
            } else {
                ""
            };
            let size_text = item.size_bytes.map(format_file_size).unwrap_or_default();
            PlaylistItemData {
                queue_index: i as i32,
                name: item.name.into(),
                playing: now_playing == Some(i),
                sprite_status: glyph.into(),
                file_size_text: size_text.into(),
                is_video,
            }
        })
        .collect();
    model.set_vec(rows);
}

fn format_file_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

pub fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".to_string();
    }
    let total = seconds.floor() as u64;
    let s = total % 60;
    let m = (total / 60) % 60;
    let h = total / 3600;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}
