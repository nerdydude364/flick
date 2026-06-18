#!/usr/bin/env bash
# Builds Flick.deb and Flick.AppImage for Linux (amd64, libmpv2).
#
# Unlike macOS, no manual dylib-bundling step is needed here:
#   - .deb declares a runtime dependency on the distro's libmpv package
#     (see [deb].depends in Packager.toml) and lets apt resolve it — nothing
#     is bundled into the .deb itself.
#   - .AppImage uses linuxdeploy (downloaded automatically by cargo-packager),
#     which walks the binary's ELF dependencies and bundles them
#     automatically, same idea as dylibbundler on macOS but built in.
#     [appimage].libs in Packager.toml is only for libraries linuxdeploy
#     doesn't auto-detect (e.g. mpv plugins loaded via dlopen rather than
#     linked directly) — may need adjusting once this is actually run.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> Checking for cargo packager, installing if missing"
cargo --list|grep packager || cargo install --locked cargo-packager

echo "==> Building .deb and .AppImage"
cargo packager -c Packager.toml --formats deb,appimage

echo "==> Done — see dist/"
echo ""
echo "Verification checklist:"
echo "  - Install the .deb on a clean container/VM where apt must resolve libmpv2"
echo "  - Run the .AppImage on a distro that deliberately lacks libmpv system-wide"
echo "  - Re-run the format-diversity fixture set (HEVC/VC-1/AVI/WMV/FLV samples)"
echo "    against both packaged builds, since a packaging mistake could silently"
echo "    regress format support even though dev-build testing passed"
