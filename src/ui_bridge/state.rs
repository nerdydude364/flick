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
}

pub struct AppState {
    pub queue: Queue,
    pub image_queue: Queue,
    pub mode: Mode,
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
}

impl AppState {
    pub fn new() -> Self {
        Self {
            queue: Queue::new(),
            image_queue: Queue::new(),
            mode: Mode::Video,
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
        }
    }

    /// Hash for `path`, computed and cached on first lookup.
    pub(crate) fn sprite_hash_for(&mut self, path: &Path) -> Option<String> {
        if let Some(h) = self.sprite_hash.get(path) {
            return Some(h.clone());
        }
        let hash = thumbnails::hash::hash_video_file(path).ok()?;
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
}
