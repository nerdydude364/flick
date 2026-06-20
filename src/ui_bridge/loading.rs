use super::state::{AppState, Mode, SpriteStatus};
use crate::AppWindow;
use crate::PlaylistItemData;
use crate::library::{MediaKind, media_kind};
use slint::{Model, VecModel};

/// Below this row count the sidebar model is rebuilt in one shot.
const PLAYLIST_SYNC_THRESHOLD: usize = 80;
/// Rows filled per UI timer tick when rebuilding a large library list.
const PLAYLIST_REBUILD_CHUNK: usize = 120;

pub struct PlaylistRebuildJob {
    pub filtered: Vec<usize>,
    pub now_playing: Option<usize>,
    pub show_sprite_status: bool,
    pub next_index: usize,
    /// First pass skips disk-heavy sprite lookups; a follow-up pass fills glyphs.
    pub defer_sprite_status: bool,
    pub sprite_pass: bool,
}

pub fn set_library_loading(app: &AppWindow, loading: bool, message: &str) {
    app.set_library_loading(loading);
    app.set_library_loading_message(message.into());
}

pub fn sync_loading_ui(app: &AppWindow, state: &AppState) {
    let playlist_busy = state.pending_playlist_rebuild.is_some();
    let gallery_busy = state.gallery_thumbs_pending > 0
        && state.gallery_thumbs_loaded < state.gallery_thumbs_pending;
    let loading = playlist_busy || gallery_busy || state.library_loading;
    app.set_library_loading(loading);
    app.set_gallery_loading(gallery_busy);

    let message = if playlist_busy {
        let job = state.pending_playlist_rebuild.as_ref().expect("job");
        let total = job.filtered.len();
        let done = job.next_index.min(total);
        if job.sprite_pass {
            format!("Updating previews… {done}/{total}")
        } else {
            format!("Loading library… {done}/{total}")
        }
    } else if gallery_busy {
        format!(
            "Loading thumbnails… {}/{}",
            state.gallery_thumbs_loaded, state.gallery_thumbs_pending
        )
    } else if state.library_loading {
        state.library_loading_message.clone()
    } else {
        String::new()
    };
    app.set_library_loading_message(message.into());
}

fn playlist_view(state: &AppState) -> (Vec<usize>, Option<usize>, bool) {
    match state.mode {
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
    }
}

fn build_playlist_row(
    state: &mut AppState,
    queue_index: usize,
    now_playing: Option<usize>,
    show_sprite_status: bool,
    defer_sprite_status: bool,
) -> PlaylistItemData {
    let item = match state.mode {
        Mode::Video => state.queue.item(queue_index),
        Mode::Image => state.image_queue.item(queue_index),
        Mode::All => state.all_queue.item(queue_index),
    }
    .expect("valid index")
    .clone();
    let is_video = media_kind(&item.path) == MediaKind::Video;
    let glyph = if defer_sprite_status {
        "-"
    } else if show_sprite_status || (state.mode == Mode::All && is_video) {
        sprite_status_glyph(state.sprite_status_for(&item.path))
    } else {
        ""
    };
    let size_text = item
        .size_bytes
        .map(super::format_file_size)
        .unwrap_or_default();
    PlaylistItemData {
        queue_index: queue_index as i32,
        name: item.name.into(),
        playing: now_playing == Some(queue_index),
        sprite_status: glyph.into(),
        file_size_text: size_text.into(),
        is_video,
    }
}

pub fn schedule_playlist_rebuild(state: &mut AppState, model: &VecModel<PlaylistItemData>) {
    let (filtered, now_playing, show_sprite_status) = playlist_view(state);
    if filtered.len() <= PLAYLIST_SYNC_THRESHOLD {
        let rows: Vec<PlaylistItemData> = filtered
            .iter()
            .map(|&i| build_playlist_row(state, i, now_playing, show_sprite_status, false))
            .collect();
        model.set_vec(rows);
        state.pending_playlist_rebuild = None;
        return;
    }

    let placeholders: Vec<PlaylistItemData> = filtered
        .iter()
        .map(|&i| build_playlist_row(state, i, now_playing, show_sprite_status, true))
        .collect();
    model.set_vec(placeholders);

    state.pending_playlist_rebuild = Some(PlaylistRebuildJob {
        filtered,
        now_playing,
        show_sprite_status,
        next_index: 0,
        defer_sprite_status: true,
        sprite_pass: false,
    });
}

/// Advances an in-progress sidebar rebuild. Returns `true` when idle.
pub fn tick_playlist_rebuild(
    app: &AppWindow,
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
) {
    let job_snapshot = state.pending_playlist_rebuild.as_ref().map(|job| {
        (
            job.next_index,
            job.filtered.len(),
            job.now_playing,
            job.show_sprite_status,
            job.defer_sprite_status,
            job.sprite_pass,
            job.filtered.clone(),
        )
    });
    let Some((
        mut next_index,
        total,
        now_playing,
        show_sprite_status,
        defer_sprite_status,
        sprite_pass,
        filtered,
    )) = job_snapshot
    else {
        return;
    };

    let end = (next_index + PLAYLIST_REBUILD_CHUNK).min(total);
    for display_index in next_index..end {
        let queue_index = filtered[display_index];
        let row = build_playlist_row(
            state,
            queue_index,
            now_playing,
            show_sprite_status,
            defer_sprite_status && !sprite_pass,
        );
        model.set_row_data(display_index, row);
    }
    next_index = end;

    if next_index < total {
        if let Some(job) = state.pending_playlist_rebuild.as_mut() {
            job.next_index = next_index;
        }
        sync_loading_ui(app, state);
        return;
    }

    if defer_sprite_status
        && show_sprite_status
        && !sprite_pass
        && filtered.iter().any(|&i| {
            let path = match state.mode {
                Mode::Video => state.queue.item(i),
                Mode::Image => state.image_queue.item(i),
                Mode::All => state.all_queue.item(i),
            };
            path.is_some_and(|item| media_kind(&item.path) == MediaKind::Video)
        })
    {
        state.pending_playlist_rebuild = Some(PlaylistRebuildJob {
            filtered,
            now_playing,
            show_sprite_status,
            next_index: 0,
            defer_sprite_status,
            sprite_pass: true,
        });
        sync_loading_ui(app, state);
        return;
    }

    state.pending_playlist_rebuild = None;
    sync_loading_ui(app, state);
}

pub fn rebuild_playlist_model(state: &mut AppState, model: &VecModel<PlaylistItemData>) {
    schedule_playlist_rebuild(state, model);
}

pub(crate) fn sprite_status_glyph(status: SpriteStatus) -> &'static str {
    match status {
        SpriteStatus::NotStarted => "-",
        SpriteStatus::InProgress => "⏳",
        SpriteStatus::Done => "✓",
    }
}
