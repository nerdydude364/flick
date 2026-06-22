use super::gif::GifAnimation;
use crate::playlist::Queue;
use crate::thumbnails;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteStatus {
    NotStarted,
    InProgress,
    Done,
}

/// State machine for the A-B segment loop button. mpv enforces the actual
/// looping natively once both `ab-loop-a`/`ab-loop-b` properties are set —
/// this just tracks which point we're picking next, since mpv's own
/// properties don't distinguish "picking" from "off".
#[derive(Debug, Clone, Copy)]
pub enum AbLoopState {
    Off,
    PickingA,
    PickingB { a: f64 },
    Active { a: f64, b: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Video,
    Image,
    All,
}

pub struct AppState {
    pub queue: Queue,
    pub image_queue: Queue,
    pub all_queue: Queue,
    pub mode: Mode,
    /// When `mode == All`, whether the currently presented item is a video
    /// (drives mpv vs image overlay visibility in the UI).
    pub all_current_is_video: bool,
    /// Grid (false) vs fullscreen single-image (true) — separate from the
    /// window's OS-level fullscreen state, matching the original's
    /// `subMode` ('grid'/'gallery') vs `isFullscreen` distinction.
    pub gallery_open: bool,
    /// Queue indices behind the grid's thumbnails, in the same order — built
    /// fresh each time the grid opens (see `toggle_gallery`), so a click on
    /// thumbnail `i` resolves via `gallery_order[i]` rather than recomputing
    /// `current_order()` (which could have drifted, e.g. a search query
    /// change, since the grid was built).
    pub gallery_order: Vec<usize>,
    /// Bumped every time the grid opens — background poster-thumbnail
    /// results are tagged with the generation active when they were
    /// requested, so results from a stale grid (closed/reopened, or the
    /// order changed, before they finished) get dropped instead of
    /// overwriting the wrong row.
    pub gallery_generation: u64,
    pub slideshow_on: bool,
    pub slideshow_duration: f64,
    // Shared handle to the UI slideshow timer (if created). Stored here so
    // mode-switching logic can stop it when video mode becomes active.
    pub slideshow_timer: Option<Rc<slint::Timer>>,
    /// Shared across both queues, matching the original's single global
    /// `shuffleOn`/`loopOn` (each queue keeps its own shuffle order, but one
    /// pair of toggles drives both).
    pub shuffle_on: bool,
    pub loop_on: bool,
    pub search_query: String,
    pub ab_loop: AbLoopState,
    pub(crate) gif_animation: Option<GifAnimation>,
    /// path -> content hash, cached so we don't re-hash (read up to 128KB)
    /// on every model rebuild.
    sprite_hash: HashMap<PathBuf, String>,
    /// hash -> status. `Done` is permanent once observed, matching the
    /// original's `spriteStatusCache` ("only permanent done state is cached").
    pub(crate) sprite_status: HashMap<String, SpriteStatus>,
    /// Sidebar list being filled incrementally after a large import.
    pub pending_playlist_rebuild: Option<super::loading::PlaylistRebuildJob>,
    pub library_loading: bool,
    pub library_loading_message: String,
    pub gallery_thumbs_pending: usize,
    pub gallery_thumbs_loaded: usize,
    pub gallery_thumbs_failed: usize,
    /// Post-batch retry passes for gallery rows that failed to decode/render.
    pub gallery_thumb_retry_pass: u8,
    /// Heavy thumbnail rebuild deferred until after the grid shell paints.
    pub pending_gallery_reload: bool,
    /// A tail-append was skipped because the grid was still generating
    /// thumbnails for an earlier batch — retried once that settles, instead
    /// of wiping and restarting the whole grid for every batch a large
    /// folder scan delivers (see `try_start_pending_gallery_append`).
    pub pending_gallery_append: bool,
    /// Bumped on clear so in-flight folder-scan batches are ignored.
    pub library_session: u64,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            queue: Queue::new(),
            image_queue: Queue::new(),
            all_queue: Queue::new(),
            mode: Mode::All,
            all_current_is_video: false,
            gallery_open: false,
            gallery_order: Vec::new(),
            gallery_generation: 0,
            slideshow_on: false,
            slideshow_duration: 8.0,
            slideshow_timer: None,
            shuffle_on: false,
            loop_on: false,
            search_query: String::new(),
            ab_loop: AbLoopState::Off,
            gif_animation: None,
            sprite_hash: HashMap::new(),
            sprite_status: HashMap::new(),
            pending_playlist_rebuild: None,
            library_loading: false,
            library_loading_message: String::new(),
            gallery_thumbs_pending: 0,
            gallery_thumbs_loaded: 0,
            gallery_thumbs_failed: 0,
            gallery_thumb_retry_pass: 0,
            pending_gallery_reload: false,
            pending_gallery_append: false,
            library_session: 0,
        }
    }

    /// Seeds the sprite-hash cache with a value already computed elsewhere
    /// (a folder scan's background thread) so `sprite_hash_for` doesn't
    /// redo that 128KB-read-plus-SHA1 on the UI thread the first time
    /// `rebuild_playlist_model` looks up this video's status glyph. A
    /// no-op if `path` is already cached.
    pub(crate) fn prime_sprite_hash(&mut self, path: PathBuf, hash: String) {
        crate::thumbnails::hash::prime_content_hash(path.clone(), hash.clone());
        self.sprite_hash.entry(path).or_insert(hash);
    }

    /// Hash for `path`, computed and cached on first lookup.
    pub(crate) fn sprite_hash_for(&mut self, path: &Path) -> Option<String> {
        if let Some(h) = self.sprite_hash.get(path) {
            return Some(h.clone());
        }
        let hash = match crate::thumbnails::hash::hash_video_file_cached(path) {
            Ok(hash) => hash,
            Err(err) => {
                crate::flick_debug!("[sprite] hash failed {}: {err}", path.display());
                return None;
            }
        };
        self.sprite_hash.insert(path.to_path_buf(), hash.clone());
        Some(hash)
    }

    /// Current sprite status for `path` — checks the on-disk cache (port of
    /// `checkSpriteStatus`/`sprite:status`) the first time, then trusts the
    /// in-memory cache for `Done`/`InProgress` afterward.
    pub(crate) fn sprite_status_for(&mut self, path: &Path) -> SpriteStatus {
        let Some(hash) = self.sprite_hash_for(path) else {
            return SpriteStatus::NotStarted;
        };
        if let Some(status) = self.sprite_status.get(&hash)
            && *status == SpriteStatus::Done
        {
            return SpriteStatus::Done;
        }
        if thumbnails::cache::is_cached(&hash) {
            self.sprite_status.insert(hash, SpriteStatus::Done);
            return SpriteStatus::Done;
        }
        self.sprite_status
            .get(&hash)
            .copied()
            .unwrap_or(SpriteStatus::NotStarted)
    }

    pub fn active_queue(&self) -> &Queue {
        match self.mode {
            Mode::Video => &self.queue,
            Mode::Image => &self.image_queue,
            Mode::All => &self.all_queue,
        }
    }
}
