#!/bin/bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 /path/to/R-ShareMouse.app 'Apple Development: ...'" >&2
  exit 2
fi

app=$1
identity=$2
root=$(cd "$(dirname "$0")/.." && pwd)
bin="$app/Contents/MacOS"

[[ -d "$app" && -f "$bin/rshare-gui" && -f "$bin/rshare-daemon" ]] || {
  echo "Expected a bundled GUI and daemon in $app" >&2
  exit 1
}
[[ "$identity" != "-" ]] || {
  echo "An ad-hoc signature changes the macOS TCC code identity on every build" >&2
  exit 1
}

codesign --force --sign "$identity" --identifier com.rsharemouse.daemon "$bin/rshare-daemon"
if [[ -f "$bin/rshare" ]]; then
  codesign --force --sign "$identity" --identifier com.rsharemouse.cli "$bin/rshare"
fi
codesign --force --sign "$identity" --options runtime \
  --entitlements "$root/apps/rshare-desktop/src-tauri/Entitlements.plist" "$app"
codesign --verify --deep --strict --verbose=2 "$app"
codesign -dr - "$app" 2>&1
