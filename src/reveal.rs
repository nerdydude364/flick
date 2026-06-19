use std::path::Path;

/// Reveals `path` in the platform's file manager — selects it in Finder on
/// macOS and Explorer on Windows; on Linux, opens the containing folder (no
/// per-file selection, since that needs a D-Bus FileManager1 call whose support
/// varies enough across desktop environments that it's not worth the complexity
/// here).
pub fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let dir = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let arg = format!("/select,{}", path.display());
        let _ = std::process::Command::new("explorer").arg(arg).spawn();
    }
}
