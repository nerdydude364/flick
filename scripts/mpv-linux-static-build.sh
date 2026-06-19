#!/usr/bin/env bash
# Builds a static libmpv for Linux from source and installs it (plus its
# dependency closure's pkg-config metadata) into
# third_party/mpv-linux-static-${MPV_ARCH}, so build.rs can link `flick`
# against a pinned mpv/ffmpeg/libass/libplacebo instead of whatever the
# target machine's distro happens to ship — that distro-version drift is
# what caused the "VersionMismatch" failures this script exists to avoid
# (libmpv2-sys hardcodes MPV_CLIENT_API_MAJOR=2 and rejects any other major
# version reported by the loaded libmpv at runtime).
#
# What ends up static (in libmpv.a, pulled in via `pkg-config --static`):
# mpv, ffmpeg (libav*/libsw*), libass, libplacebo, lcms2 — the pieces whose
# version/build could plausibly vary in a way that breaks playback or trips
# the API-major check. Everything else (X11/Wayland, EGL/GL, ALSA/PulseAudio/
# PipeWire, VAAPI/VDPAU, freetype/fontconfig/harfbuzz/fribidi/zlib/glib) stays
# on the target system's own libs, same as flick's other dynamic deps — these
# are stable, near-universal base-desktop ABIs that were never the source of
# the version-mismatch problem. freetype/fontconfig/harfbuzz/fribidi/zlib
# specifically are *available* as static archives via apt's own -dev
# packages, but Ubuntu ships their .a right next to the .so in the same
# directory, and the linker prefers .so when both sit in one search path —
# so they end up dynamic without any special-casing here, which is fine.
#
# Licensing note: this builds mpv with -Dgpl=true and ffmpeg with
# --enable-gpl (explicit project decision — see PR description). Combined
# with flick's Apache-2.0 source license, this makes the distributed
# flick binary a GPL-derivative work going forward; that's intentional, not
# an oversight introduced by this script.
#
# libplacebo has a small C++ translation unit (std::to_chars/from_chars in
# convert.cc), so the final consumer needs -lstdc++ on the link line —
# plain `cc` (unlike `c++`/`g++`) won't add that automatically. This script
# patches it directly into the installed mpv.pc's Libs: line so any
# consumer of `pkg-config --static --libs mpv` gets it for free.
#
# Reads version pins from scripts/mpv-linux-static-versions.txt (bumping
# that file is what should invalidate any CI cache of the built prefix).
# Idempotent: skips a stage if its expected output already exists, and
# skips the whole script if the prefix's fingerprint already matches the
# current pins file, unless MPV_FORCE=1.
set -euo pipefail
cd "$(dirname "$0")/.."

case "$(uname -m)" in
  x86_64) DEFAULT_ARCH=amd64 ;;
  aarch64) DEFAULT_ARCH=arm64 ;;
  *) DEFAULT_ARCH="$(uname -m)" ;;
esac
MPV_ARCH="${MPV_ARCH:-$DEFAULT_ARCH}"

VERSIONS_FILE="scripts/mpv-linux-static-versions.txt"
# shellcheck disable=SC1090
source "$VERSIONS_FILE"

PREFIX="$(pwd)/third_party/mpv-linux-static-${MPV_ARCH}"
export MPV_STATIC_DIR="$PREFIX"
FINGERPRINT="$(sha256sum "$VERSIONS_FILE" | cut -d' ' -f1)"

if [ -f "$PREFIX/.installed" ] && [ "$(cat "$PREFIX/.installed")" = "$FINGERPRINT" ] && [ "${MPV_FORCE:-0}" != "1" ]; then
  echo "==> Static mpv already built at $PREFIX (matches $VERSIONS_FILE)"
  if [ -n "${GITHUB_ENV:-}" ]; then
    echo "MPV_STATIC_DIR=$PREFIX" >>"$GITHUB_ENV"
  fi
  exit 0
fi

for tool in nasm cmake autoreconf libtoolize ninja meson pkg-config git python3 cc c++; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "FAILED: required build tool '$tool' not found." >&2
    echo "Install build tooling, e.g.:" >&2
    echo "  apt install build-essential nasm cmake autoconf automake libtool ninja-build python3 python3-pip pkg-config git" >&2
    echo "  pip install 'meson>=1.3.0'   # Ubuntu 22.04's apt meson is too old for mpv" >&2
    exit 1
  fi
done

MESON_VER="$(meson --version)"
MESON_MIN="1.3.0"
if [ "$(printf '%s\n%s\n' "$MESON_MIN" "$MESON_VER" | sort -V | head -n1)" != "$MESON_MIN" ]; then
  echo "FAILED: meson $MESON_VER is too old (mpv needs >=$MESON_MIN)." >&2
  echo "  pip install --upgrade 'meson>=$MESON_MIN'   # Ubuntu 22.04's apt meson is 0.61.2" >&2
  exit 1
fi

python3 -c "import jinja2" 2>/dev/null || pip3 install --quiet jinja2

DEV_MULTIARCH="$(dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null || echo x86_64-linux-gnu)"
SYSTEM_PKGCONFIG="/usr/lib/${DEV_MULTIARCH}/pkgconfig:/usr/lib/pkgconfig:/usr/share/pkgconfig"
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig:$SYSTEM_PKGCONFIG"

mkdir -p "$PREFIX"
SRC="$(pwd)/tmp/mpv-linux-static-src"
mkdir -p "$SRC"
NPROC="$(nproc)"

clone_pinned() {
  local url="$1" ref="$2" dest="$3"
  if [ -d "$dest/.git" ]; then
    echo "==> $dest already cloned, skipping"
    return
  fi
  echo "==> Cloning $url @ $ref"
  git clone --depth 1 --branch "$ref" "$url" "$dest"
}

echo "=== [1/5] lcms2 ($LCMS2_VERSION) ==="
if [ ! -f "$PREFIX/lib/liblcms2.a" ]; then
  clone_pinned https://github.com/mm2/Little-CMS.git "$LCMS2_VERSION" "$SRC/lcms2"
  (
    cd "$SRC/lcms2"
    ./autogen.sh
    CFLAGS="-fPIC" ./configure --prefix="$PREFIX" --enable-static --disable-shared
    make -j"$NPROC"
    make install
  )
else
  echo "already built, skipping"
fi

echo "=== [2/5] libass ($LIBASS_VERSION) ==="
if [ ! -f "$PREFIX/lib/libass.a" ]; then
  clone_pinned https://github.com/libass/libass.git "$LIBASS_VERSION" "$SRC/libass"
  (
    cd "$SRC/libass"
    ./autogen.sh
    CFLAGS="-fPIC" ./configure --prefix="$PREFIX" --enable-static --disable-shared
    make -j"$NPROC"
    make install
  )
else
  echo "already built, skipping"
fi

echo "=== [3/5] ffmpeg ($FFMPEG_VERSION) ==="
if [ ! -f "$PREFIX/lib/libavcodec.a" ]; then
  clone_pinned https://github.com/FFmpeg/FFmpeg.git "$FFMPEG_VERSION" "$SRC/ffmpeg"
  (
    cd "$SRC/ffmpeg"
    ./configure \
      --prefix="$PREFIX" \
      --enable-static --disable-shared \
      --enable-gpl \
      --enable-libass \
      --enable-vaapi --enable-vdpau \
      --disable-programs --disable-doc --disable-debug \
      --extra-cflags="-fPIC" \
      --pkg-config-flags="--static"
    make -j"$NPROC"
    make install
  )
else
  echo "already built, skipping"
fi

echo "=== [4/5] libplacebo ($LIBPLACEBO_VERSION) ==="
if [ ! -f "$PREFIX/lib/libplacebo.a" ]; then
  clone_pinned https://code.videolan.org/videolan/libplacebo.git "$LIBPLACEBO_VERSION" "$SRC/libplacebo"
  (
    cd "$SRC/libplacebo"
    git config submodule.3rdparty/fast_float.shallow false
    # Vulkan-Headers is needed even with -Dvulkan=disabled: the no-op stub
    # implementation (src/vulkan/stubs.c) still references Vulkan's struct/
    # type definitions for ABI stability, even though it adds no actual
    # libvulkan runtime dependency (headers only, no library to link).
    git submodule update --init 3rdparty/fast_float 3rdparty/glad 3rdparty/jinja 3rdparty/markupsafe 3rdparty/Vulkan-Headers
    rm -rf build
    meson setup build \
      --prefix="$PREFIX" --libdir=lib \
      --default-library=static \
      -Dvulkan=disabled -Dshaderc=disabled -Dglslang=disabled -Dd3d11=disabled \
      -Dopengl=enabled -Ddemos=false -Dtests=false -Dbench=false
    ninja -C build
    ninja -C build install
  )
else
  echo "already built, skipping"
fi

echo "=== [5/5] mpv ($MPV_VERSION) ==="
if [ ! -f "$PREFIX/lib/libmpv.a" ]; then
  clone_pinned https://github.com/mpv-player/mpv.git "$MPV_VERSION" "$SRC/mpv"
  (
    cd "$SRC/mpv"
    rm -rf build
    meson setup build \
      --prefix="$PREFIX" --libdir=lib \
      --default-library=static \
      -Dlibmpv=true -Dcplayer=false -Dtests=false -Dgpl=true -Dbuild-date=false
    ninja -C build
    ninja -C build install
  )
else
  echo "already built, skipping"
fi

if [ -n "$(find "$PREFIX/lib" -maxdepth 1 -name 'libmpv.so*' 2>/dev/null)" ]; then
  echo "FAILED: $PREFIX/lib contains a libmpv.so* alongside libmpv.a — this would let the" >&2
  echo "linker silently prefer the dynamic library and defeat the static link entirely." >&2
  exit 1
fi

# Patched onto libplacebo.pc specifically (not mpv.pc): pkg-config's
# --static expansion resolves left-to-right, so a library that provides a
# symbol must be emitted *after* the library that needs it. libplacebo.pc's
# own Libs: line is where -lplacebo itself gets emitted, so appending
# -lstdc++ there guarantees it always lands immediately after -lplacebo
# regardless of where libplacebo's Libs end up in the overall expansion —
# appending it to mpv.pc's Libs: instead would put it too early (right
# after -lmpv, long before -lplacebo is ever emitted), which silently
# breaks the static link (undefined reference to std::to_chars/from_chars).
LIBPLACEBO_PC="$PREFIX/lib/pkgconfig/libplacebo.pc"
if ! grep -q -- '-lstdc++' "$LIBPLACEBO_PC"; then
  sed -i 's/^Libs: \(.*\)$/Libs: \1 -lstdc++/' "$LIBPLACEBO_PC"
fi

echo "=== Verifying pkg-config --static --libs mpv ==="
STATIC_LIBS="$(pkg-config --static --libs mpv)"
echo "$STATIC_LIBS"
case "$STATIC_LIBS" in
  *-lmpv*) ;;
  *)
    echo "FAILED: pkg-config --static --libs mpv did not include -lmpv — broken .pc chain." >&2
    exit 1
    ;;
esac

echo "$FINGERPRINT" >"$PREFIX/.installed"
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "MPV_STATIC_DIR=$PREFIX" >>"$GITHUB_ENV"
fi
echo "==> Static mpv ready at $PREFIX"
