use std::path::{Path, PathBuf};
use std::process::Command;

/// Reveals `path` in the platform's file manager and selects/highlights the
/// file itself (Finder on macOS, Explorer on Windows, FileManager1 or common
/// file managers on Linux).
pub fn reveal_in_file_manager(path: &Path) {
    let path = resolve_reveal_path(path);
    if !path.exists() {
        eprintln!("show in folder: path does not exist: {}", path.display());
        return;
    }

    #[cfg(target_os = "macos")]
    macos_reveal(&path);
    #[cfg(target_os = "linux")]
    linux_reveal(&path);
    #[cfg(target_os = "windows")]
    windows_reveal(&path);
}

fn resolve_reveal_path(path: &Path) -> PathBuf {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    {
        let text = resolved.to_string_lossy();
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    resolved
}

#[cfg(target_os = "macos")]
fn macos_reveal(path: &Path) {
    if Command::new("open")
        .arg("-R")
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }

    let escaped = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let script = format!("tell application \"Finder\" to reveal POSIX file \"{escaped}\"");
    let _ = Command::new("osascript").args(["-e", &script]).status();
    let _ = Command::new("osascript")
        .args(["-e", "tell application \"Finder\" to activate"])
        .status();
}

#[cfg(target_os = "linux")]
fn linux_reveal(path: &Path) {
    if try_file_manager1_show(path) {
        return;
    }

    for cmd in [
        "nautilus",
        "dolphin",
        "nemo",
        "caja",
        "pcmanfm-qt",
        "pcmanfm",
    ] {
        if try_fm_select(cmd, path) {
            return;
        }
    }

    if let Some(dir) = path.parent() {
        let _ = Command::new("xdg-open").arg(dir).status();
    }
}

#[cfg(target_os = "linux")]
fn try_file_manager1_show(path: &Path) -> bool {
    let uri = path_to_file_uri(path);
    Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.FileManager1",
            "--object-path",
            "/org/freedesktop/FileManager1",
            "--method",
            "org.freedesktop.FileManager1.ShowItems",
            &format!("['{uri}']"),
            "''",
        ])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "linux")]
fn try_fm_select(command: &str, path: &Path) -> bool {
    Command::new(command)
        .args(["--select", &path.to_string_lossy()])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "windows")]
fn windows_reveal(path: &Path) {
    let path_text = path.to_string_lossy();
    let arg = if path_text.contains(' ') {
        format!("/select,\"{path_text}\"")
    } else {
        format!("/select,{path_text}")
    };
    let _ = Command::new("explorer").arg(arg).status();
}

#[cfg(target_os = "linux")]
fn path_to_file_uri(path: &Path) -> String {
    let path = resolve_reveal_path(path);
    let normalized = {
        let text = path.to_string_lossy();
        #[cfg(windows)]
        {
            text.replace('\\', "/")
        }
        #[cfg(not(windows))]
        {
            text.into_owned()
        }
    };
    let mut uri = String::from("file://");
    for byte in normalized.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' => {
                uri.push(*byte as char);
            }
            b':' if cfg!(windows) => uri.push(*byte as char),
            _ => {
                uri.push('%');
                uri.push_str(&format!("{byte:02X}"));
            }
        }
    }
    uri
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;

    #[test]
    fn file_uri_encodes_spaces() {
        let uri = path_to_file_uri(Path::new("/home/user/My Videos/clip.mp4"));
        assert!(uri.contains("My%20Videos"));
        assert!(uri.starts_with("file://"));
    }
}
