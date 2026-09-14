#!/usr/bin/env bash
# Stage the daemon BEFORE Tauri bundles/signs the app. The same sidecar
# configuration is used for local installs, DMGs, and signed update archives.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p tray/binaries
target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
staged="tray/binaries/autotrim-$target"
if [ "$target" = universal-apple-darwin ]; then
  cargo build --locked --release -p autotrim --target aarch64-apple-darwin
  cargo build --locked --release -p autotrim --target x86_64-apple-darwin
  lipo -create target/aarch64-apple-darwin/release/autotrim \
    target/x86_64-apple-darwin/release/autotrim -output "$staged"
else
  cargo build --locked --release -p autotrim --target "$target"
  cp "target/$target/release/autotrim" "$staged"
fi
# Tauri signs this sidecar and the enclosing app before making the archive.
