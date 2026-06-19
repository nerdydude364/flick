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
        #[cfg(target_os = "linux")]
        {
            if link_mpv_static_linux() {
                return;
            }
        }

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
             If mpv is already installed, ensure `pkg-config --libs mpv` works.\n\
             \n\
             Release builds link a pinned, statically-built mpv instead (see\n\
             scripts/mpv-linux-static-build.sh) — set MPV_STATIC_DIR or populate\n\
             third_party/mpv-linux-static-{{amd64,arm64}} to opt into that locally."
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

/// Links a pinned mpv/ffmpeg/libass/libplacebo built statically from source
/// (see scripts/mpv-linux-static-build.sh), instead of whatever libmpv the
/// target machine's distro happens to ship — that distro-to-distro drift is
/// what caused libmpv2-sys's `MPV_CLIENT_API_MAJOR` check to reject mismatched
/// mpv builds on some machines (the "VersionMismatch" issue). Convention
/// mirrors the Windows `MPV_DEV_DIR` / `third_party/mpv-windows-{arch}` path
/// below: an explicit `MPV_STATIC_DIR` env var, or a conventional
/// `third_party/mpv-linux-static-{amd64,arm64}` directory populated by the
/// build script — absent both, this just returns false and falls through to
/// the normal dynamic pkg-config probing, so local `apt install libmpv-dev &&
/// cargo build` workflows are untouched.
#[cfg(target_os = "linux")]
fn link_mpv_static_linux() -> bool {
    let arch = std::env::var("MPV_ARCH").unwrap_or_else(|_| {
        match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default().as_str() {
            "x86_64" => "amd64".to_string(),
            "aarch64" => "arm64".to_string(),
            other => other.to_string(),
        }
    });

    let dir = std::env::var_os("MPV_STATIC_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(format!("third_party/mpv-linux-static-{arch}")));

    let lib_dir = dir.join("lib");
    if !lib_dir.join("libmpv.a").exists() {
        return false;
    }

    // libmpv2-sys's own build.rs unconditionally emits a plain (dynamic)
    // `cargo:rustc-link-lib=mpv`. If a libmpv.so* ever sat next to libmpv.a
    // here, the linker could silently prefer it over the static archive and
    // quietly defeat the entire point of this path — fail loudly instead.
    let has_dynamic_sibling = std::fs::read_dir(&lib_dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().starts_with("libmpv.so"));
    if has_dynamic_sibling {
        panic!(
            "{} contains a libmpv.so* alongside libmpv.a — this would let the \
             linker silently prefer the dynamic library and defeat the static \
             link entirely. Rebuild the prefix with scripts/mpv-linux-static-build.sh.",
            lib_dir.display()
        );
    }

    let pkgconfig_dir = lib_dir.join("pkgconfig");
    let pkg_config_path = match std::env::var("PKG_CONFIG_PATH") {
        Ok(existing) if !existing.is_empty() => {
            format!("{}:{existing}", pkgconfig_dir.display())
        }
        _ => pkgconfig_dir.display().to_string(),
    };

    let Ok(output) = std::process::Command::new("pkg-config")
        .args(["--static", "--libs", "mpv"])
        .env("PKG_CONFIG_PATH", pkg_config_path)
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }

    let mut saw_mpv = false;
    for token in String::from_utf8_lossy(&output.stdout).split_whitespace() {
        if let Some(path) = token.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={path}");
        } else if let Some(name) = token.strip_prefix("-l") {
            if name == "mpv" {
                // Force static even if some unrelated -L on the link line
                // happens to carry a stray libmpv.so — defense in depth on
                // top of the has_dynamic_sibling check above.
                println!("cargo:rustc-link-lib=static=mpv");
                saw_mpv = true;
            } else {
                println!("cargo:rustc-link-lib={name}");
            }
        } else if !token.is_empty() {
            println!("cargo:rustc-link-arg={token}");
        }
    }
    saw_mpv
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
