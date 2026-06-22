mod ab_loop;
mod gallery;
mod gif;
mod loading;
mod playback;
mod sprite_preview;
mod state;

pub use ab_loop::{handle_progress_click, toggle_ab_loop};
pub use gallery::{
    GalleryContext, GalleryThumbResult, apply_gallery_thumb, run_pending_gallery_reload,
    toggle_gallery,
};
pub use gif::tick_gif_animation;
pub use loading::{
    rebuild_playlist_model, sync_loading_ui, tick_playlist_rebuild, try_finish_import_session,
    try_start_pending_gallery_append, try_start_pending_gallery_reload,
};
pub use playback::{
    advance_on_video_eof, all_slideshow_wants_timer, clear_library, enqueue_paths,
    navigate_all_relative, navigate_image_relative, play_index, present_item, remove_item,
    reorder_item, set_mode, set_slideshow_duration, show_image_at, sync_active_view_ui,
    sync_all_view_ui, sync_image_viewer_ui, toggle_slideshow,
};
pub use sprite_preview::{
    apply_sprite_result, hide_list_sprite_preview, schedule_sprite_generation,
    schedule_sprite_generation_for_now_playing, show_list_sprite_preview,
};
pub use state::{AppState, Mode};

use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

thread_local! {
    static TOAST_DISMISS_TIMER: RefCell<Option<Rc<slint::Timer>>> = const { RefCell::new(None) };
}

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

pub(crate) fn format_file_size(bytes: u64) -> String {
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

pub fn show_toast(app: &crate::AppWindow, message: impl Into<slint::SharedString>, error: bool) {
    app.set_toast_message(message.into());
    app.set_toast_error(error);
    app.set_toast_visible(true);

    let app_weak = app.as_weak();
    TOAST_DISMISS_TIMER.with(|slot| {
        let timer = slot
            .borrow_mut()
            .get_or_insert_with(|| Rc::new(slint::Timer::default()))
            .clone();
        timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(2),
            move || {
                if let Some(app) = app_weak.upgrade() {
                    app.set_toast_visible(false);
                }
            },
        );
    });
}
