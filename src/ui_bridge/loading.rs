use super::state::{AppState, Mode, SpriteStatus};
use crate::AppWindow;
use crate::PlaylistItemData;
use crate::library::{MediaKind, media_kind};
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

/// Below this row count the sidebar model is rebuilt in one shot.
const PLAYLIST_SYNC_THRESHOLD: usize = 80;
/// Rows filled per UI timer tick when rebuilding a large library list.
const PLAYLIST_REBUILD_CHUNK: usize = 120;
/// Decoded sidebar row poster images — avoids re-reading JPEGs on every filter/search rebuild.
const POSTER_IMAGE_CACHE_MAX: usize = 512;

thread_local! {
    static POSTER_IMAGE_CACHE: RefCell<HashMap<String, slint::Image>> = RefCell::new(HashMap::new());
}

fn transparent_row_thumbnail() -> slint::Image {
    let pixel_buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&[0, 0, 0, 0], 1, 1);
    slint::Image::from_rgba8(pixel_buffer)
}

fn poster_image_for_hash(hash: &str) -> Option<slint::Image> {
    POSTER_IMAGE_CACHE.with(|cache| {
        if let Some(image) = cache.borrow().get(hash) {
            return Some(image.clone());
        }
        let image = crate::thumbnails::load_cached_poster(hash)?;
        let mut cache = cache.borrow_mut();
        if cache.len() >= POSTER_IMAGE_CACHE_MAX
            && let Some(oldest) = cache.keys().next().cloned()
        {
            cache.remove(&oldest);
        }
        cache.insert(hash.to_string(), image.clone());
        Some(image)
    })
}

fn playlist_row_thumbnail(path: &Path) -> slint::Image {
    let Some(hash) = crate::thumbnails::hash::hash_video_file_cached(path).ok() else {
        return transparent_row_thumbnail();
    };
    if !crate::thumbnails::cache::is_poster_cached(&hash) {
        return transparent_row_thumbnail();
    }
    poster_image_for_hash(&hash).unwrap_or_else(transparent_row_thumbnail)
}

pub struct PlaylistRebuildJob {
    pub filtered: Vec<usize>,
    pub now_playing: Option<usize>,
    pub show_sprite_status: bool,
    pub next_index: usize,
    /// First pass skips disk-heavy sprite lookups and poster decodes; a
    /// follow-up pass fills glyphs and row thumbnails.
    pub defer_sprite_status: bool,
    pub sprite_pass: bool,
}

fn playlist_busy(state: &AppState) -> bool {
    state.pending_playlist_rebuild.is_some()
}

fn gallery_busy(state: &AppState) -> bool {
    state.gallery_thumbs_pending > 0
        && state.gallery_thumbs_loaded + state.gallery_thumbs_failed < state.gallery_thumbs_pending
}

fn compose_loading_message(state: &AppState) -> String {
    let mut parts = Vec::new();
    if let Some(job) = state.pending_playlist_rebuild.as_ref() {
        let total = job.filtered.len();
        let done = job.next_index.min(total);
        let label = if job.sprite_pass { "previews" } else { "items" };
        parts.push(format!("{label} {done}/{total}"));
    }
    if state.gallery_thumbs_pending > 0 {
        let done = state.gallery_thumbs_loaded + state.gallery_thumbs_failed;
        let mut message = format!("thumbs {done}/{}", state.gallery_thumbs_pending);
        if state.gallery_thumbs_failed > 0 {
            message.push_str(&format!(" ({} failed)", state.gallery_thumbs_failed));
        }
        parts.push(message);
    }
    if parts.is_empty() {
        if state.library_loading_message.is_empty() {
            "Loading library…".to_string()
        } else {
            state.library_loading_message.clone()
        }
    } else {
        format!("Loading library… ({})", parts.join(", "))
    }
}

pub fn sync_loading_ui(app: &AppWindow, state: &mut AppState) {
    let playlist = playlist_busy(state);
    let gallery = gallery_busy(state);

    if playlist || gallery {
        state.library_loading = true;
    }

    let loading = state.library_loading || playlist || gallery;

    if !loading {
        state.library_loading = false;
        state.library_loading_message.clear();
    } else if state.library_loading_message.is_empty() {
        state.library_loading_message = "Loading library…".into();
    }

    let message = if loading {
        compose_loading_message(state)
    } else {
        String::new()
    };

    app.set_library_loading(loading);
    app.set_gallery_loading(loading && !state.gallery_open);
    app.set_library_loading_message(message.into());
}

pub fn schedule_playlist_rebuild(state: &mut AppState, model: &VecModel<PlaylistItemData>) {
    let (filtered, now_playing, show_sprite_status) = playlist_view(state);
    if filtered.len() <= PLAYLIST_SYNC_THRESHOLD {
        let rows: Vec<PlaylistItemData> = filtered
            .iter()
            .map(|&i| build_playlist_row(state, i, now_playing, show_sprite_status, false, false))
            .collect();
        model.set_vec(rows);
        state.pending_playlist_rebuild = None;
        return;
    }

    let placeholders: Vec<PlaylistItemData> = filtered
        .iter()
        .map(|&i| build_playlist_row(state, i, now_playing, show_sprite_status, true, true))
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
    for (display_index, &queue_index) in filtered[next_index..end].iter().enumerate() {
        let display_index = next_index + display_index;
        let row = build_playlist_row(
            state,
            queue_index,
            now_playing,
            show_sprite_status,
            defer_sprite_status && !sprite_pass,
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

    if defer_sprite_status && !sprite_pass {
        // Second pass: fill row thumbnails (and sprite-status glyphs when
        // applicable). Pass 1 only installs placeholders for responsiveness.
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

/// Updates the sprite-status glyph for the row matching `hash` without rebuilding
/// the whole sidebar model — used when a background sprite job finishes.
pub(crate) fn patch_sprite_status_for_hash(
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    hash: &str,
    status: SpriteStatus,
) {
    let (filtered, _, show_sprite_status) = playlist_view(state);
    let glyph = sprite_status_glyph(status);
    for (display_index, &queue_index) in filtered.iter().enumerate() {
        let item = match state.mode {
            Mode::Video => state.queue.item(queue_index),
            Mode::Image => state.image_queue.item(queue_index),
            Mode::All => state.all_queue.item(queue_index),
        };
        let Some(item) = item else {
            continue;
        };
        let path = item.path.clone();
        let is_video = media_kind(&path) == MediaKind::Video;
        if !(show_sprite_status || state.mode == Mode::All && is_video) {
            continue;
        }
        if state.sprite_hash_for(&path).as_deref() != Some(hash) {
            continue;
        }
        let Some(mut row) = model.row_data(display_index) else {
            continue;
        };
        row.sprite_status = glyph.into();
        model.set_row_data(display_index, row);
    }
}

/// Updates sidebar row thumbnail(s) once a poster lands in the disk cache —
/// e.g. after the gallery grid finishes generating a thumb the list can reuse.
pub(crate) fn patch_playlist_thumbnail_for_hash(
    state: &mut AppState,
    model: &VecModel<PlaylistItemData>,
    hash: &str,
) {
    if !crate::thumbnails::cache::is_poster_cached(hash) {
        return;
    }
    let Some(image) = poster_image_for_hash(hash) else {
        return;
    };
    let (filtered, _, _) = playlist_view(state);
    for (display_index, &queue_index) in filtered.iter().enumerate() {
        let item = match state.mode {
            Mode::Video => state.queue.item(queue_index),
            Mode::Image => state.image_queue.item(queue_index),
            Mode::All => state.all_queue.item(queue_index),
        };
        let Some(item) = item else {
            continue;
        };
        let path = item.path.clone();
        if state.sprite_hash_for(&path).as_deref() != Some(hash) {
            continue;
        }
        let Some(mut row) = model.row_data(display_index) else {
            continue;
        };
        row.thumbnail = image.clone();
        model.set_row_data(display_index, row);
    }
}

/// Starts a deferred gallery rebuild once the current one (if any) finishes.
pub fn try_start_pending_gallery_reload(
    state: &mut AppState,
    app: &AppWindow,
    gallery: &super::gallery::GalleryContext<'_>,
) {
    if state.pending_gallery_reload && !gallery_busy(state) {
        super::gallery::run_pending_gallery_reload(state, app, gallery);
    }
}

/// Ends the import loading session once sidebar and gallery work are idle.
pub fn try_finish_import_session(state: &mut AppState, app: &AppWindow) {
    if !state.library_loading {
        return;
    }
    if playlist_busy(state) || gallery_busy(state) {
        return;
    }
    state.library_loading = false;
    state.library_loading_message.clear();
    sync_loading_ui(app, state);
}

pub(crate) fn sprite_status_glyph(status: SpriteStatus) -> &'static str {
    match status {
        SpriteStatus::NotStarted => "-",
        SpriteStatus::InProgress => "⏳",
        SpriteStatus::Done => "✓",
    }
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
    defer_thumbnail: bool,
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
        thumbnail: if defer_thumbnail {
            transparent_row_thumbnail()
        } else {
            playlist_row_thumbnail(&item.path)
        },
    }
}
