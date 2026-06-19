mod ab_loop;
mod gallery;
mod gif;
mod playback;
mod sprite_preview;
mod state;

pub use ab_loop::{handle_progress_click, toggle_ab_loop};
pub use gallery::{GalleryThumbResult, apply_gallery_thumb, toggle_gallery};
pub use gif::tick_gif_animation;
pub use playback::{
    enqueue_paths, navigate_image_relative, play_index, remove_item, reorder_item, set_mode,
    set_slideshow_duration, show_image_at, sync_image_viewer_ui, toggle_slideshow,
};
pub use sprite_preview::{
    apply_sprite_result, hide_list_sprite_preview, schedule_sprite_generation,
    show_list_sprite_preview,
};
pub use state::{AppState, Mode, SpriteStatus};

use crate::PlaylistItemData;
use slint::VecModel;
use std::path::Path;

/// Logs `result`'s error (if any), prefixed with `label`, and reports whether
/// the call succeeded. Most callers fire-and-forget the return value; a few
/// (e.g. resuming playback only after a successful unpause) gate follow-up
/// state on it instead of duplicating the `if let Err(err) = ... { eprintln!
/// ... }` boilerplate at every mpv call site.
pub(crate) fn log_mpv_err<T>(label: &str, result: Result<T, libmpv2::Error>) -> bool {
    match result {
        Ok(_) => true,
        Err(err) => {
            eprintln!("{label} failed: {err}");
            false
        }
    }
}

/// Display name for a dialog-picked file: just the basename, matching
/// `toFileEntry(fp)` called with no `rootDir` in main.js. Folder scans use a
/// root-relative path instead — see `enqueue_paths`.
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

/// Builds the sidebar's row list from whichever queue is active for the
/// current mode — port of `renderList`'s `activeQ = mode === 'image' ?
/// imageQueue : queue`. Sprite status only ever applies to the video queue.
pub fn rebuild_playlist_model(state: &mut AppState, model: &VecModel<PlaylistItemData>) {
    let is_video = state.mode == Mode::Video;
    let (filtered, now_playing) = if is_video {
        (
            state.queue.filtered_indices(&state.search_query),
            state.queue.now_playing(),
        )
    } else {
        (
            state.image_queue.filtered_indices(&state.search_query),
            state.image_queue.now_playing(),
        )
    };
    let rows: Vec<PlaylistItemData> = filtered
        .into_iter()
        .map(|i| {
            let item = if is_video {
                state.queue.item(i)
            } else {
                state.image_queue.item(i)
            }
            .expect("valid index")
            .clone();
            let glyph = if is_video {
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

/// Port of `formatTime` (non-precise branch only — the hundredths-of-a-second
/// precise mode is a minor display toggle, deferred along with auto-hide chrome).
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
