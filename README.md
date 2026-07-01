# Flick

A lightweight, native desktop media player for mixed video and image libraries — built in Rust with [Slint](https://slint.dev) for UI and [libmpv](https://mpv.io) for playback.

Point it at a folder of videos and photos and it plays both, with a shared playlist/queue, shuffle, loop, search, and a hover-scrub sprite-sheet preview borrowed from professional video tools.

## Features

- **Mixed-media queue** — videos and images live in separate queues but share one sidebar, search box, and shuffle/loop toggle; adding files auto-switches the active mode based on what you just added.
- **Video playback** via libmpv: seek, 10s skip, A-B loop, variable speed (0.5x–2x), screenshots, external subtitles (`.srt`/`.vtt`/`.ass`/`.ssa`/`.sub`).
- **Scrub-bar thumbnail preview** — hovering the progress bar shows a real frame from the video at that timestamp, pulled from a sprite sheet generated once per file and cached on disk (keyed by a content hash, not the path, so renamed/moved files still hit the cache).
- **Image viewer** with zoom, rotation, animated GIF playback, and a slideshow mode with adjustable interval.
- **Folder import** — recursively scans a directory in the background, validating real file content (magic bytes) rather than trusting extensions, and streams results into the queue in batches so the UI never blocks.
- **Drag-to-reorder**, fullscreen with auto-hiding chrome, and full keyboard control (space, arrow keys, `[`/`]`, `Ctrl+F`, `Esc`).

## Installing on Linux (apt)

Debian/Ubuntu (amd64 and arm64) users can install Flick from a signed apt repository instead of downloading the `.deb` by hand:

```sh
curl -fsSL https://apt.flick.free/pubkey.gpg | sudo gpg --dearmor -o /usr/share/keyrings/flick-archive-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/flick-archive-keyring.gpg] https://apt.flick.free stable main" | sudo tee /etc/apt/sources.list.d/flick.list
sudo apt-get update
sudo apt-get install flick
```

Every tagged release re-signs and republishes this repository — see the `publish-apt` job in `.github/workflows/release.yml` and `scripts/publish-apt.sh`.

## Requirements

- Rust (edition 2024 toolchain — see `Cargo.toml`)
- A system install of **libmpv**:
  - macOS: `brew install mpv pkg-config`
  - Linux: `libmpv2`/`libmpv-dev` (or distro equivalent) for local `cargo build`/`run`
  - Windows: run `./scripts/mpv-windows-setup.sh` (downloads shinchiro's libmpv dev + runtime builds)
  - `build.rs` locates the library via `pkg-config` on Unix, falling back to a few common lib dirs, or `MPV_DEV_DIR` / `third_party/mpv-windows-*` on Windows.
  - Linux **release** builds instead statically link a pinned mpv/ffmpeg/libass/libplacebo built from source via `./scripts/mpv-linux-static-build.sh`, so published `.deb`/`.AppImage` artifacts don't depend on whatever mpv version (if any) the target machine happens to have — see `link_mpv_static_linux` in `build.rs`.

## Building & running

```sh
cargo build --release
cargo run --release -- [optional/path/to/file]
```

A path argument is optional and is queued at startup (handy for "open with" integrations).

## Testing

```sh
cargo test
```

Unit tests cover the playlist/queue logic (dedup, reordering, shuffle, filtering), extension/magic-byte detection, and the content-hashing used for the thumbnail cache. Two `#[ignore]`d spike tests in `src/thumbnails/` exercise the real mpv-based frame extraction pipeline against a local fixture video and aren't run by default — see the doc comments on those tests for how to run them manually.

## Packaging

Release bundles are built via [`cargo-packager`](https://github.com/crabnebula-dev/cargo-packager), configured in `Packager.toml`:

```sh
./scripts/package-macos.sh   # -> dist/Flick.app, dist/Flick.dmg
./scripts/package-linux.sh   # -> dist/*.deb, dist/*.AppImage
./scripts/package-windows.sh # -> dist/*-setup.exe (NSIS installer)
```

The macOS script additionally bundles libmpv and its dependency closure into the `.app` so it doesn't require Homebrew on the target machine. The Windows script does the same for the NSIS installer via `packaging/windows-dlls/`.

## Project layout

```
src/
  main.rs            Event loop wiring: mpv setup, the OpenGL render underlay, and every
                      Slint callback registration (UI <-> mpv/state glue lives in ui_bridge).
  ui_bridge.rs        AppState (queues, mode, shuffle/loop, A-B loop, sprite/GIF state) and
                      the functions that keep it in sync with the Slint AppWindow.
  dialogs.rs          Native file/folder picker wrappers (rfd).
  reveal.rs           "Show in Finder/Files" platform integration.
  library/            Media-type detection (extension + magic-byte validation) and the
                      background recursive folder scanner.
  playlist/           Queue data structure: ordering, shuffle, filtering, prev/next —
                      fully unit-tested and independent of mpv/Slint.
  thumbnails/         Sprite-sheet generation: content hashing, on-disk cache, frame
                      extraction via a headless mpv instance, and JPEG tiling.
ui/
  app-window.slint    Top-level window: layout, keyboard shortcuts, chrome auto-hide.
  sidebar.slint        Playlist panel: search, shuffle/loop, drag-to-reorder, hover preview.
  progress-bar.slint   Scrub bar with A-B loop markers and sprite-sheet hover preview.
  image-viewer.slint   Zoom/rotate/pan canvas for image mode.
  components.slint     Shared widgets (buttons, sliders, chips).
  theme.slint           Design tokens (colors, spacing, radii) — the single source of truth
                        for styling; components should never hardcode a color.
```

## Architecture notes

Video is rendered through an OpenGL "underlay": mpv draws directly into the same GL context Slint uses, driven from Slint's `RenderingState` notifications (see the `MpvUnderlay` type in `main.rs`). This avoids any frame-copy between mpv and the UI, at the cost of some carefully-scoped `unsafe` to satisfy lifetime/`'static` requirements — each `unsafe` block in `main.rs` carries a doc comment explaining the soundness argument.

Background work (folder scanning, sprite generation) runs on plain OS threads and reports back to the UI thread over `std::sync::mpsc` channels, drained by a periodic Slint timer — `Rc`/`RefCell` app state never crosses a thread boundary.

## Known limitations

- The packaged Linux `.deb`/`.AppImage` statically link mpv/ffmpeg/libass/libplacebo, so they have no runtime dependency on the target's own mpv version — verified by installing/running both on clean Ubuntu 22.04 and 24.04 containers (no mpv installed at all) and the AppImage on Fedora, with no mpv-related dynamic dependency in `readelf -d`/`ldd` output on any of them.
- Shuffle and loop are single global toggles shared by both the video and image queues (each queue keeps its own shuffle order, but one pair of switches drives both).
