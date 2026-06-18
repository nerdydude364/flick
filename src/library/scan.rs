use super::ext::{ext_lower, is_image_file, is_video_file};
use super::magic::confirm_media_magic;
use std::path::{Path, PathBuf};

const SCAN_BATCH_SIZE: usize = 50;

/// Recursively scans `root` for video/image files, validating magic bytes
/// (port of `confirmMediaMagic`) and delivering results in sorted batches via
/// `on_batch` — mirrors `walkDir`/`scanFolder` in the Electron app's main.js.
/// `jwalk` skips hidden files/dirs by default, matching the original's
/// `entry.name.startsWith('.')` skip.
pub fn scan_folder(root: &Path, mut on_batch: impl FnMut(Vec<PathBuf>)) {
    let mut buffer = Vec::with_capacity(SCAN_BATCH_SIZE);

    for entry in jwalk::WalkDir::new(root) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !is_video_file(&path) && !is_image_file(&path) {
            continue;
        }
        let ext = ext_lower(&path).unwrap_or_default();
        if !confirm_media_magic(&path, &ext) {
            continue;
        }

        buffer.push(path);
        if buffer.len() >= SCAN_BATCH_SIZE {
            let mut batch = std::mem::take(&mut buffer);
            batch.sort();
            on_batch(batch);
        }
    }

    if !buffer.is_empty() {
        buffer.sort();
        on_batch(buffer);
    }
}
