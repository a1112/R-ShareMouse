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

# Verify the actual networking helper as well as the outer GUI. This verifies
# signing/metadata only; it never asserts the user's TCC consent is granted.
codesign --verify --strict --verbose=2 "$bin/rshare-daemon"
helper_identity=$(codesign -dv --verbose=4 "$bin/rshare-daemon" 2>&1)
if ! grep -qx 'Identifier=com.rsharemouse.daemon' <<< "$helper_identity"; then
  echo "Unexpected networking helper identity" >&2
  exit 1
fi
/usr/libexec/PlistBuddy -c 'Print :NSLocalNetworkUsageDescription' "$app/Contents/Info.plist"
codesign -dr - "$bin/rshare-daemon" 2>&1
