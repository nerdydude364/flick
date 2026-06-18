use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    pub id: u64,
    pub name: String,
    pub path: PathBuf,
    /// Lowercased once at construction so search filtering doesn't
    /// re-lowercase every item's name on every keystroke.
    pub name_lower: String,
    /// File size in bytes, `stat()`'d once at construction so rebuilding the
    /// sidebar model (every keystroke, shuffle toggle, add/remove) doesn't
    /// re-stat every visible item. `None` if the stat failed (matches the
    /// original's blank-size-text behavior on failure).
    pub size_bytes: Option<u64>,
}
