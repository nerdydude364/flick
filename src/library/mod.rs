pub mod ext;
pub mod magic;
pub mod scan;

pub use ext::{MediaKind, is_video_file, media_kind};
pub use scan::{ScannedFile, scan_folder};

use std::path::PathBuf;

/// Port of `filterValidMedia`: spot-checks magic bytes for individually
/// dialog-picked files (folder scans already do this in `scan_folder`).
/// Note this only rejects files whose extension is in the explicit
/// magic-check list in `magic.rs` and whose *bytes* don't match — any other
/// extension passes through unchecked, same gap as the original.
pub fn filter_valid_media(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|p| {
            let ext = ext::ext_lower(p).unwrap_or_default();
            magic::confirm_media_magic(p, &ext)
        })
        .collect()
}
