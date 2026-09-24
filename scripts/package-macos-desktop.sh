#!/bin/bash
set -euo pipefail

usage() {
  echo "Usage: APPLE_SIGNING_IDENTITY='Apple Development: ...' $0 [--install]" >&2
  exit 2
}

[[ $# -le 1 ]] || usage
[[ $# -eq 0 || $1 == --install ]] || usage
[[ $(uname -s) == Darwin ]] || { echo "macOS is required" >&2; exit 1; }

identity=${APPLE_SIGNING_IDENTITY:-}
[[ -n "$identity" && "$identity" != - ]] || {
  echo "Set APPLE_SIGNING_IDENTITY to a persistent Apple signing certificate; ad-hoc signatures cannot retain input permissions across updates" >&2
  exit 1
}

root=$(cd "$(dirname "$0")/.." && pwd)
bundle="$root/apps/rshare-desktop/target/release/bundle/macos/R-ShareMouse.app"
installed="$root/R-ShareMouse.app"

npm ci --prefix "$root/apps/rshare-desktop-frontend"
cargo build --manifest-path "$root/Cargo.toml" -p rshare-daemon -p rshare-cli --release --locked
(
  cd "$root/apps/rshare-desktop"
  APPLE_SIGNING_IDENTITY="$identity" npx --yes @tauri-apps/cli@2.11.4 build --bundles app
)

cp "$root/target/release/rshare-daemon" "$bundle/Contents/MacOS/rshare-daemon"
cp "$root/target/release/rshare" "$bundle/Contents/MacOS/rshare"
"$root/scripts/sign-macos-desktop.sh" "$bundle" "$identity"

if [[ $# -eq 1 ]]; then
  if pgrep -f "$installed/Contents/MacOS/rshare-(gui|daemon)" >/dev/null; then
    echo "Quit R-ShareMouse and its daemon, then rerun with --install" >&2
    exit 1
  fi

  if [[ -e "$installed" ]]; then
    backup_dir="$HOME/.Trash/R-ShareMouse-updates"
    mkdir -p "$backup_dir"
    backup="$backup_dir/R-ShareMouse-$(date +%Y%m%d-%H%M%S).app.backup"
    mv "$installed" "$backup"
  fi
  if ! mv "$bundle" "$installed"; then
    if [[ -n ${backup:-} ]]; then mv "$backup" "$installed"; fi
    exit 1
  fi
  if ! codesign --verify --deep --strict "$installed"; then
    mv "$installed" "$bundle"
    if [[ -n ${backup:-} ]]; then mv "$backup" "$installed"; fi
    exit 1
  fi
  echo "Installed: $installed"
else
  echo "Signed bundle: $bundle"
fi
