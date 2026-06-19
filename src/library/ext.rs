use std::path::Path;

pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "webm", "ogg", "ogv", "mov", "mkv", "avi", "m4v", "mts", "ts", "wmv", "flv",
];

pub const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "avif", "tiff", "tif", "svg",
];

pub(crate) fn ext_lower(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_lowercase())
}

pub fn is_video_file(path: &Path) -> bool {
    ext_lower(path).is_some_and(|e| VIDEO_EXTS.contains(&e.as_str()))
}

pub fn is_image_file(path: &Path) -> bool {
    ext_lower(path).is_some_and(|e| IMAGE_EXTS.contains(&e.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_video_and_image_extensions_case_insensitively() {
        assert!(is_video_file(&PathBuf::from("clip.MP4")));
        assert!(is_image_file(&PathBuf::from("photo.PNG")));
        assert!(!is_video_file(&PathBuf::from("notes.txt")));
        assert!(!is_image_file(&PathBuf::from("notes.txt")));
    }
}
