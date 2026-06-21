#!/usr/bin/env bash
# Downloads and extracts shinchiro's libmpv dev + runtime packages for Windows.
#
# Expects MPV_ARCH=amd64 or arm64. Installs into third_party/mpv-windows-${MPV_ARCH}:
#   lib/mpv.lib       — MSVC import library (generated from libmpv-2.dll on Windows)
#   bin/*.dll         — libmpv-2.dll and the rest of mpv's runtime dependency closure
#
# Sets MPV_DEV_DIR when done. Re-runs are skipped if .installed is present unless
# MPV_FORCE=1.
set -euo pipefail
cd "$(dirname "$0")/.."

MPV_ARCH="${MPV_ARCH:-amd64}"
case "$MPV_ARCH" in
  amd64) SHINCHIRO_ARCH="x86_64" ;;
  arm64) SHINCHIRO_ARCH="aarch64" ;;
  *)
    echo "MPV_ARCH must be amd64 or arm64 (got: $MPV_ARCH)" >&2
    exit 1
    ;;
esac

MPV_DIR="$(pwd)/third_party/mpv-windows-${MPV_ARCH}"
export MPV_DEV_DIR="$MPV_DIR"

if [ -f "$MPV_DIR/.installed" ] && [ "${MPV_FORCE:-0}" != "1" ]; then
  echo "==> libmpv already installed at $MPV_DIR"
  exit 0
fi

resolve_7z() {
  if command -v 7z >/dev/null 2>&1; then
    command -v 7z
    return
  fi
  for candidate in \
    "/c/Program Files/7-Zip/7z.exe" \
    "/c/Program Files (x86)/7-Zip/7z.exe"; do
    if [ -f "$candidate" ]; then
      echo "$candidate"
      return
    fi
  done
  echo "7-Zip not found — install 7-Zip or add 7z to PATH" >&2
  exit 1
}

resolve_python() {
  if command -v python3 >/dev/null 2>&1; then
    command -v python3
    return
  fi
  if command -v python >/dev/null 2>&1; then
    command -v python
    return
  fi
  echo "Python 3 not found — install Python or add it to PATH" >&2
  exit 1
}

fetch_asset_url() {
  local kind="$1" # mpv-dev or mpv
  local python_cmd
  python_cmd="$(resolve_python)"
  "$python_cmd" - "$kind" "$SHINCHIRO_ARCH" <<'PY'
import json, re, sys, urllib.request

kind, arch = sys.argv[1], sys.argv[2]
# mpv-dev-x86_64-20260610-git-abc.7z / mpv-x86_64-20260610-git-abc.7z (skip v3/i686)
pattern = re.compile(rf"^{re.escape(kind)}-{re.escape(arch)}-\d{{8}}")
with urllib.request.urlopen(
    "https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/latest"
) as resp:
    data = json.load(resp)
for asset in data.get("assets", []):
    name = asset.get("name", "")
    if not name.endswith(".7z"):
        continue
    stem = name[:-3]
    if "-v3-" in name or "i686" in name:
        continue
    if pattern.match(stem):
        print(asset["browser_download_url"])
        break
else:
    sys.exit(f"No {kind} asset found for {arch}")
PY
}

SEVENZ="$(resolve_7z)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "==> Fetching libmpv dev + runtime URLs (${SHINCHIRO_ARCH})"
DEV_URL="$(fetch_asset_url mpv-dev)"
RUN_URL="$(fetch_asset_url mpv)"

echo "==> Downloading mpv-dev"
curl -fsSL "$DEV_URL" -o "$TMP/mpv-dev.7z"
echo "==> Downloading mpv runtime"
curl -fsSL "$RUN_URL" -o "$TMP/mpv-runtime.7z"

rm -rf "$MPV_DIR"
mkdir -p "$MPV_DIR/lib" "$MPV_DIR/bin"

echo "==> Extracting mpv-dev"
"$SEVENZ" x -y "-o$TMP/dev" "$TMP/mpv-dev.7z" >/dev/null
echo "==> Extracting mpv runtime"
"$SEVENZ" x -y "-o$TMP/runtime" "$TMP/mpv-runtime.7z" >/dev/null

# shinchiro archives flatten to a single directory of files.
shopt -s nullglob
find_root() {
  local root="$1"
  local marker="$2"
  find "$root" -name "$marker" -print -quit | xargs -I{} dirname {}
}

dev_root="$(find_root "$TMP/dev" 'libmpv-2.dll')"
runtime_root="$(find_root "$TMP/runtime" 'mpv.exe')"
if [ -z "$runtime_root" ]; then
  runtime_root="$(find_root "$TMP/runtime" 'd3dcompiler_43.dll')"
fi
if [ -z "$dev_root" ]; then
  echo "FAILED: could not locate libmpv-2.dll in downloaded dev archive" >&2
  exit 1
fi
if [ -z "$runtime_root" ]; then
  echo "FAILED: could not locate runtime root in downloaded runtime archive" >&2
  exit 1
fi
cp -f "$dev_root/libmpv-2.dll" "$MPV_DIR/bin/"
if [ -d "$dev_root/include" ]; then
  cp -a "$dev_root/include" "$MPV_DIR/"
fi
if [ -f "$dev_root/mpv.def" ]; then
  cp -f "$dev_root/mpv.def" "$MPV_DIR/lib/"
fi

cp -f "$runtime_root/"*.dll "$MPV_DIR/bin/"

if [ -f "$MPV_DIR/lib/mpv.lib" ]; then
  echo "==> Using bundled mpv.lib"
elif [ -n "${WINDIR:-}" ] || [ "${OS:-}" = "Windows_NT" ]; then
  echo "==> Generating mpv.lib for MSVC"
  MPV_LIB_DIR="$MPV_DIR/lib" MPV_DLL="$MPV_DIR/bin/libmpv-2.dll" MPV_ARCH="$MPV_ARCH" \
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(pwd)/scripts/generate-mpv-import-lib.ps1"
else
  echo "WARN: skipping mpv.lib generation (not on Windows — link step needs MSVC lib.exe)" >&2
fi

if [ ! -f "$MPV_DIR/lib/mpv.lib" ]; then
  echo "FAILED: mpv.lib not found at $MPV_DIR/lib/mpv.lib" >&2
  exit 1
fi

date -u +"%Y-%m-%dT%H:%M:%SZ" >"$MPV_DIR/.installed"
echo "==> libmpv ready at $MPV_DIR"
