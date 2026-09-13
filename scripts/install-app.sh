#!/usr/bin/env bash
# Build the menu bar app as a real .app bundle and put it in Applications,
# so it has an icon, opens with a double click or `autotrim open`, and can
# be kept in the Dock. macOS only for now; Linux and Windows get a bundle
# from the same `tauri build` once someone tests them there.
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "install-app.sh only knows macOS so far; run 'npx @tauri-apps/cli@2 build' in tray/ and install the bundle by hand." >&2
  exit 1
fi

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

# The daemon is the command-line binary, not the app. A copy rides along
# inside the bundle so the window's "Run in background" can install it as
# the login service on a machine that never ran cargo.
cargo build --release

cd tray
npx --yes @tauri-apps/cli@2 build --bundles app

APP="../target/release/bundle/macos/autoTrim.app"
mkdir -p "$APP/Contents/Resources"
cp ../target/release/autotrim "$APP/Contents/Resources/autotrim"
# The bundle only carries the linker's ad-hoc signature; re-sign it so the
# added binary is covered too.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true

DEST="/Applications/autoTrim.app"
if [ ! -w /Applications ]; then
  DEST="$HOME/Applications/autoTrim.app"
  mkdir -p "$HOME/Applications"
fi

# Quit a running copy so the swap is clean; LaunchServices would otherwise
# keep pointing at the old binary until restart.
osascript -e 'tell application "autoTrim" to quit' >/dev/null 2>&1 || true
rm -rf "$DEST"
cp -R "$APP" "$DEST"
echo "installed $DEST"
echo "open it from Applications, or with: autotrim open"
