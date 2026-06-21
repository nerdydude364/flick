use super::ext::{ext_lower, is_image_file, is_video_file};
use super::magic::confirm_media_magic;
use std::path::{Path, PathBuf};

const SCAN_BATCH_SIZE: usize = 50;

/// One scanned file plus the metadata that's cheap to fetch here on the
/// background scan thread but expensive enough (a `stat()`, or for videos a
/// 128KB read + SHA1) that doing it lazily on the UI thread once per file —
/// which is what `Queue::enqueue` and `AppState::sprite_hash_for` used to do
/// — blocks the UI for the whole length of a large import.
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: Option<u64>,
    /// Content hash (128 KB fingerprint) — computed during scan for every
    /// media file so gallery thumbnail workers and sidebar sprite status can
    /// reuse it without re-reading file headers on the UI thread.
    pub content_hash: Option<String>,
}

/// Recursively scans `root` for video/image files, validating magic bytes
/// (port of `confirmMediaMagic`) and delivering results in sorted batches via
/// `on_batch` — mirrors `walkDir`/`scanFolder` in the Electron app's main.js.
/// `jwalk` skips hidden files/dirs by default, matching the original's
/// `entry.name.startsWith('.')` skip.
pub fn scan_folder(root: &Path, mut on_batch: impl FnMut(Vec<ScannedFile>)) {
    let mut buffer = Vec::with_capacity(SCAN_BATCH_SIZE);

    for entry in jwalk::WalkDir::new(root) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let is_video = is_video_file(&path);
        if !is_video && !is_image_file(&path) {
            continue;
        }
        let ext = ext_lower(&path).unwrap_or_default();
        if !confirm_media_magic(&path, &ext) {
            continue;
        }

        let size = entry.metadata().ok().map(|m| m.len());
        let content_hash = crate::thumbnails::hash::hash_video_file(&path).ok();
        if let Some(ref hash) = content_hash {
            crate::thumbnails::hash::prime_content_hash(path.clone(), hash.clone());
        }
        buffer.push(ScannedFile {
            path,
            size,
            content_hash,
        });
        if buffer.len() >= SCAN_BATCH_SIZE {
            let mut batch = std::mem::take(&mut buffer);
            batch.sort_by(|a, b| a.path.cmp(&b.path));
            on_batch(batch);
        }
    }

    if !buffer.is_empty() {
        buffer.sort_by(|a, b| a.path.cmp(&b.path));
        on_batch(buffer);
    }
}
