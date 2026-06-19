use crate::library::ext::{IMAGE_EXTS, VIDEO_EXTS};
use rfd::FileDialog;
use std::path::PathBuf;

/// Combined media picker used by the sidebar's file-open action.
pub fn open_media_files() -> Option<Vec<PathBuf>> {
    FileDialog::new()
        .set_title("Open Media (Image/Video)")
        .add_filter(
            "Media Files (Image/Video)",
            &[VIDEO_EXTS, IMAGE_EXTS].concat(),
        )
        .add_filter("Video Files", VIDEO_EXTS)
        .add_filter("Image Files", IMAGE_EXTS)
        .add_filter("All Files", &["*"])
        .pick_files()
}

/// Port of `openFolderAndSend`'s dialog.
pub fn open_folder() -> Option<PathBuf> {
    FileDialog::new().set_title("Open Folder").pick_folder()
}

/// Subtitle file picker — not present in the Electron app (subtitles were a
/// FEATURES.md TODO), new here since mpv supports them essentially for free.
pub fn open_subtitle_file() -> Option<PathBuf> {
    FileDialog::new()
        .set_title("Open Subtitle File")
        .add_filter("Subtitle Files", &["srt", "vtt", "ass", "ssa", "sub"])
        .add_filter("All Files", &["*"])
        .pick_file()
}
