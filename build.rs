fn main() {
    slint_build::compile("ui/app-window.slint").unwrap();
    link_mpv();
}

/// libmpv2-sys emits `cargo:rustc-link-lib=mpv` but no search path. Resolve
/// the library location via pkg-config (Linux libmpv-dev, Homebrew mpv, …)
/// or MPV_DEV_DIR / third_party/mpv-windows-* on Windows.
fn link_mpv() {
    #[cfg(unix)]
    {
        if pkg_config_link_mpv() || probe_mpv_lib_dirs() {
            return;
        }

        panic!(
            "Could not find libmpv for linking.\n\
             \n\
             Install mpv's development library, then rebuild:\n\
               Debian/Ubuntu:  sudo apt install libmpv-dev mpv\n\
               Fedora/RHEL:    sudo dnf install mpv-devel mpv\n\
               Arch:           sudo pacman -S mpv\n\
               macOS/Homebrew: brew install mpv\n\
             \n\
             If mpv is already installed, ensure `pkg-config --libs mpv` works."
        );
    }

    #[cfg(windows)]
    {
        if probe_windows_mpv_lib_dir() {
            return;
        }

        panic!(
            "Could not find libmpv for linking.\n\
             \n\
             Run ./scripts/mpv-windows-setup.sh (sets up third_party/mpv-windows-*),\n\
             or set MPV_DEV_DIR to a directory containing lib/mpv.lib and bin/libmpv-2.dll."
        );
    }
}

#[cfg(unix)]
fn pkg_config_link_mpv() -> bool {
    let Ok(status) = std::process::Command::new("pkg-config")
        .args(["--exists", "mpv"])
        .status()
    else {
        return false;
    };
    if !status.success() {
        return false;
    }

    let Ok(output) = std::process::Command::new("pkg-config")
        .args(["--libs-only-L", "mpv"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }

    let mut linked = false;
    for token in String::from_utf8_lossy(&output.stdout).split_whitespace() {
        if let Some(path) = token.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={path}");
            linked = true;
        }
    }
    linked
}

#[cfg(unix)]
fn probe_mpv_lib_dirs() -> bool {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/opt/homebrew/lib", "/usr/local/lib"]
    } else {
        &[
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
            "/usr/lib64",
            "/usr/lib",
            "/usr/local/lib",
        ]
    };

    let mut found = false;
    for dir in candidates {
        let dir = std::path::Path::new(dir);
        if dir.join("libmpv.so").exists() || dir.join("libmpv.dylib").exists() {
            println!("cargo:rustc-link-search=native={}", dir.display());
            found = true;
        }
    }
    found
}

#[cfg(windows)]
fn probe_windows_mpv_lib_dir() -> bool {
    use std::path::{Path, PathBuf};

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("MPV_DEV_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    if let Ok(arch) = std::env::var("MPV_ARCH") {
        candidates.push(PathBuf::from(format!("third_party/mpv-windows-{arch}")));
    }
    candidates.extend([
        PathBuf::from("third_party/mpv-windows-amd64"),
        PathBuf::from("third_party/mpv-windows-arm64"),
    ]);

    for dir in candidates {
        if link_mpv_from_windows_dir(&dir) {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn link_mpv_from_windows_dir(dir: &Path) -> bool {
    let lib_dir = dir.join("lib");
    if lib_dir.join("mpv.lib").exists() {
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
        return true;
    }
    false
}
