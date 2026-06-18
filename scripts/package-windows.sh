#!/usr/bin/env bash
# Builds Flick_{version}_{arch}-setup.exe for Windows via cargo-packager (NSIS).
#
# Bundles libmpv-2.dll and its dependency closure next to flick.exe inside the
# installer — same idea as dylibbundler on macOS, but Windows needs every DLL
# copied explicitly (see packaging/windows-dlls staging below).
set -euo pipefail
cd "$(dirname "$0")/.."

MPV_ARCH="${MPV_ARCH:-amd64}"
export MPV_DEV_DIR="${MPV_DEV_DIR:-$(pwd)/third_party/mpv-windows-${MPV_ARCH}}"

echo "==> Setting up libmpv for Windows (${MPV_ARCH})"
./scripts/mpv-windows-setup.sh

echo "==> Staging runtime DLLs for the NSIS bundle"
mkdir -p packaging/windows-dlls
rm -f packaging/windows-dlls/*.dll
cp -f "${MPV_DEV_DIR}/bin/"*.dll packaging/windows-dlls/

echo "==> Checking for cargo packager, installing if missing"
cargo --list | grep packager || cargo install --locked cargo-packager

echo "==> Building NSIS installer"
cargo packager -c Packager.toml --formats nsis

echo "==> Done — see dist/"
echo ""
echo "Verification checklist:"
echo "  - Install the .exe on a clean Windows VM without mpv installed separately"
echo "  - Confirm video playback and file associations work"
echo "  - Re-run the format-diversity fixture set against the packaged build"
