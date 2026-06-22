use std::path::{Path, PathBuf};

/// Platform-conventional cache directory (e.g. `~/Library/Caches/Flick/sprites`
/// on macOS, `~/.cache/flick/sprites` on Linux via `$XDG_CACHE_HOME`) — the
/// Electron app used `~/.flick/sprites`, but that was an Electron `userData`
/// convention this native app has no reason to keep.
pub fn sprites_dir() -> PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Flick").join("sprites");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn sprite_file(hash: &str) -> PathBuf {
    sprites_dir().join(format!("{hash}.jpg"))
}

pub fn meta_file(hash: &str) -> PathBuf {
    sprites_dir().join(format!("{hash}.json"))
}

pub fn is_cached(hash: &str) -> bool {
    sprite_file(hash).exists() && meta_file(hash).exists()
}

/// Separate subdirectory for image-mode gallery poster thumbnails — same
/// cache root as video sprites, but kept apart since these are single small
/// JPEGs with no companion metadata file (see `sprite_file`/`meta_file`).
pub fn posters_dir() -> PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Flick").join("posters");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn poster_file(hash: &str) -> PathBuf {
    posters_dir().join(format!("{hash}-s{}.jpg", super::poster::POSTER_SIZE))
}

pub fn is_poster_cached(hash: &str) -> bool {
    poster_is_ready(hash)
}

/// True when the poster JPEG exists and has non-zero size — guards against
/// reading a file that was renamed into place but isn't visible yet (seen on
/// some Linux filesystems) or a truncated write.
pub fn poster_is_ready(hash: &str) -> bool {
    poster_file(hash)
        .metadata()
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

/// Atomic write: temp file + rename, so a crash mid-write never leaves a
/// partially-written file behind — port of the tmp+rename pattern used for
/// the metadata JSON in main.js's `sprite:generate` handler.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// Marker files for content hashes whose source mpv couldn't even probe
/// (corrupt/truncated/invalid media) — separate from the poster/sprite
/// caches above since there's no thumbnail payload to store, just the fact
/// that generation was already proven hopeless. Persisted to disk (not just
/// in-memory) so a known-broken file costs one probe total, not one per
/// retry pass, per replay, or per app restart.
fn invalid_dir() -> PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Flick").join("invalid");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn invalid_marker_file(hash: &str) -> PathBuf {
    invalid_dir().join(format!("{hash}.broken"))
}

pub fn is_known_invalid(hash: &str) -> bool {
    invalid_marker_file(hash).exists()
}

pub fn mark_invalid(hash: &str) {
    let _ = write_atomic(&invalid_marker_file(hash), b"");
}
