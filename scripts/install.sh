#!/usr/bin/env bash
# Install autoTrim: build the command-line binary, put it on PATH, start the
# daemon as a login service, and bundle the menu bar app into Applications.
# Run it again after pulling to upgrade everything in place.
#
#   ./scripts/install.sh              everything
#   ./scripts/install.sh --no-app     the command and the daemon only
#   ./scripts/install.sh --uninstall  remove all of it; your data stays
#
# Needs Rust (https://rustup.rs). The app also needs Node, for Tauri's CLI.
# AUTOTRIM_BIN_DIR picks where the binary goes: /usr/local/bin when it is
# writable, otherwise ~/.local/bin. The login service and the app are macOS
# only so far; elsewhere this installs the command and stops.
set -euo pipefail

usage() { sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; }

WITH_SERVICE=1
WITH_APP=1
UNINSTALL=0
for arg in "$@"; do
  case "$arg" in
    --no-service) WITH_SERVICE=0 ;;
    --no-app) WITH_APP=0 ;;
    --uninstall) UNINSTALL=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

if [ "$(uname -s)" != "Darwin" ]; then
  WITH_SERVICE=0
  WITH_APP=0
fi

BIN_DIR="${AUTOTRIM_BIN_DIR:-}"
if [ -z "$BIN_DIR" ]; then
  if [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then
    BIN_DIR=/usr/local/bin
  else
    BIN_DIR="$HOME/.local/bin"
  fi
fi
BIN="$BIN_DIR/autotrim"
LABEL=com.autotrim.daemon

case "$(uname -s)" in
  Darwin) DATA_DIR="${AUTOTRIM_DATA_DIR:-$HOME/Library/Application Support/autotrim}" ;;
  *) DATA_DIR="${AUTOTRIM_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/autotrim}" ;;
esac

on_path() {
  case ":$PATH:" in
    *":$1:"*) return 0 ;;
  esac
  return 1
}

if [ "$UNINSTALL" = 1 ]; then
  if [ "$WITH_SERVICE" = 1 ]; then
    # Any copy of the binary can remove the service; without one, do what
    # `service uninstall` does by hand.
    removed=0
    for cli in "$BIN" ./target/release/autotrim \
      /Applications/autoTrim.app/Contents/Resources/autotrim \
      "$HOME/Applications/autoTrim.app/Contents/Resources/autotrim" \
      /Applications/autoTrim.app/Contents/MacOS/autotrim \
      "$HOME/Applications/autoTrim.app/Contents/MacOS/autotrim"; do
      if [ -x "$cli" ]; then
        "$cli" service uninstall
        removed=1
        break
      fi
    done
    if [ "$removed" = 0 ]; then
      launchctl bootout "gui/$(id -u)/$LABEL" >/dev/null 2>&1 || true
      rm -f "$HOME/Library/LaunchAgents/$LABEL.plist"
    fi
    echo "removed the login service"
  fi
  if [ "$WITH_APP" = 1 ]; then
    osascript -e 'tell application "autoTrim" to quit' >/dev/null 2>&1 || true
    for app in /Applications/autoTrim.app "$HOME/Applications/autoTrim.app"; do
      if [ -d "$app" ]; then
        rm -rf "$app"
        echo "removed $app"
      fi
    done
  fi
  if [ -e "$BIN" ] || [ -L "$BIN" ]; then
    rm -f "$BIN"
    echo "removed $BIN"
  fi
  echo "your data is still in $DATA_DIR; delete that directory if you want nothing left"
  exit 0
fi

# 1. The command-line binary: the daemon, `scan`, and every verb.
if ! command -v cargo >/dev/null 2>&1; then
  echo "autoTrim is built from source and needs Rust. Install it with" >&2
  echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
  echo "then open a new terminal and run this script again." >&2
  exit 1
fi
cargo build --release -p autotrim
mkdir -p "$BIN_DIR"
# Rename into place rather than overwrite: a daemon running from the old
# file keeps it until the service restarts below.
cp target/release/autotrim "$BIN_DIR/.autotrim.new"
mv -f "$BIN_DIR/.autotrim.new" "$BIN"
echo "installed $BIN"

# 2. Fresh app installs choose background monitoring in the setup wizard.
# Preserve an existing service on upgrades; CLI-only installs keep the old default.
if [ "$WITH_APP" = 1 ] && command -v npx >/dev/null 2>&1 && [ ! -f "$HOME/Library/LaunchAgents/$LABEL.plist" ]; then
  WITH_SERVICE=0
fi
if [ "$WITH_SERVICE" = 1 ]; then
  "$BIN" service install
fi

# 3. The menu bar app, bundled with Tauri's CLI.
if [ "$WITH_APP" = 1 ]; then
  if command -v npx >/dev/null 2>&1; then
    ./scripts/install-app.sh
    # Use the app's copy for the CLI too, so subsequent in-app updates also
    # update commands run from a terminal. CLI-only installs stay standalone.
    APP_DEST=/Applications/autoTrim.app
    if [ ! -w /Applications ]; then APP_DEST="$HOME/Applications/autoTrim.app"; fi
    ln -sf "$APP_DEST/Contents/MacOS/autotrim" "$BIN_DIR/.autotrim.link"
    mv -f "$BIN_DIR/.autotrim.link" "$BIN"
    if [ "$WITH_SERVICE" = 1 ]; then "$BIN" service install; fi
  else
    echo "skipped the menu bar app: bundling it needs Node (npx)." >&2
    echo "Install Node and run ./scripts/install-app.sh, or pass --no-app." >&2
  fi
fi

echo
echo "done. Try:  autotrim scan"
if ! on_path "$BIN_DIR"; then
  echo "($BIN_DIR is not on your PATH; add it, or call $BIN directly)"
fi
if [ "$WITH_SERVICE" = 1 ]; then
  echo "The daemon is running; 'autotrim watch' shows what it sees."
else
  echo "'autotrim daemon' runs the daemon in this terminal."
fi
if [ "$WITH_APP" = 1 ]; then
  echo "'autotrim open' opens the menu bar app and its first-run setup wizard."
fi
