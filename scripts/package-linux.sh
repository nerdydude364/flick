#!/usr/bin/env bash
# Builds Flick.deb and Flick.AppImage for Linux.
#
# mpv/ffmpeg/libass/libplacebo are statically linked into the flick binary
# (see scripts/mpv-linux-static-build.sh and build.rs's link_mpv_static_linux)
# rather than resolved against whatever the distro/AppImage host happens to
# provide — this is what makes both outputs immune to the mpv
# "VersionMismatch" failure that motivated that change (libmpv2-sys hardcodes
# MPV_CLIENT_API_MAJOR=2 and rejects any other major version reported by a
# *dynamically* loaded libmpv). Everything else (X11/Wayland/GL, ALSA/
# PulseAudio/PipeWire, VAAPI/VDPAU) still comes from the host, same as before:
#   - .deb declares those as runtime deps (see [deb].depends in
#     Packager.toml) and lets apt resolve them — nothing is bundled.
#   - .AppImage uses linuxdeploy (downloaded automatically by cargo-packager),
#     which walks the binary's ELF dependencies and bundles them
#     automatically.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> Checking for cargo packager, installing if missing"
cargo --list|grep packager || cargo install --locked cargo-packager

echo "==> Building flick (release)"
cargo build --release

echo "==> Verifying mpv/ffmpeg/libass/libplacebo didn't leak back in as a dynamic dependency"
FORBIDDEN='libmpv|libavcodec|libavformat|libavutil|libavdevice|libavfilter|libswscale|libswresample|libass|libplacebo'
if readelf -d target/release/flick | grep -iE "$FORBIDDEN"; then
  echo "FAILED: target/release/flick has a dynamic dependency on one of mpv/ffmpeg/libass/libplacebo." >&2
  echo "It should be statically linked — check MPV_STATIC_DIR / third_party/mpv-linux-static-*" >&2
  echo "and re-run scripts/mpv-linux-static-build.sh, then rebuild." >&2
  exit 1
fi

echo "==> Building .deb and .AppImage"
cargo packager -c Packager.toml --formats deb,appimage

echo "==> Done — see dist/"
echo ""
echo "Verification checklist:"
echo "  - Install the .deb on a clean container/VM with no mpv/libmpv installed at all"
echo "  - Run the .AppImage on a distro that deliberately lacks mpv/libmpv system-wide"
echo "    (both should work identically to a distro that has some *other* mpv version"
echo "    installed, since mpv is statically linked in either way)"
echo "  - Re-run the format-diversity fixture set (HEVC/VC-1/AVI/WMV/FLV samples)"
echo "    against both packaged builds, since a packaging mistake could silently"
echo "    regress format support even though dev-build testing passed"
