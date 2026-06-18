#!/usr/bin/env bash
# Builds Flick.app and Flick.dmg for macOS.
#
# This can't just be `cargo packager --formats app,dmg` in one shot: cargo-packager's
# dmg step *regenerates* the .app bundle from scratch internally (re-copying the raw,
# un-bundled binary), silently undoing the dylibbundler fix below if it runs after
# app-bundling once already happened. So: build the app, fix it up, then build the dmg
# directly from the already-fixed .app via hdiutil (bypassing cargo-packager's own
# dmg step, and its `create-dmg` dependency, entirely — that script's Finder-prettifying
# AppleScript step also doesn't work in sandboxed/automation-restricted environments).
set -euo pipefail
cd "$(dirname "$0")/.."

APP="dist/Flick.app"
DMG="dist/Flick.dmg"

echo "==> Checking for cargo packager, installing if missing"
cargo --list|grep packager || cargo install --locked cargo-packager

echo "==> Building .app bundle"
cargo packager -c Packager.toml --formats app

echo "==> Bundling libmpv + its dependency closure into Contents/Frameworks"
dylibbundler -od -b \
  -x "$APP/Contents/MacOS/flick" \
  -d "$APP/Contents/Frameworks/" \
  -p "@executable_path/../Frameworks/"

echo "==> Verifying no absolute Homebrew/local paths remain in the bundle"
if otool -L "$APP/Contents/MacOS/flick" | grep -qE '/opt/homebrew|/usr/local'; then
  echo "FAILED: main binary still references an absolute Homebrew/local path" >&2
  exit 1
fi
for lib in "$APP"/Contents/Frameworks/*.dylib; do
  if otool -L "$lib" | tail -n +2 | grep -qE '/opt/homebrew|/usr/local'; then
    echo "FAILED: $lib still references an absolute Homebrew/local path" >&2
    exit 1
  fi
done
echo "    clean — bundle is self-contained"

echo "==> Building .dmg directly from the fixed .app (skips cargo-packager's dmg step)"
rm -f "$DMG"
hdiutil create -volname "Flick" -srcfolder "$APP" -ov -format UDZO "$DMG"

echo "==> Done: $APP, $DMG"
