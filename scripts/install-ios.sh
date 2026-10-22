#!/bin/bash
set -euo pipefail

if [[ $# -ne 1 || -z "${RSHARE_IOS_TEAM:-}" ]]; then
  echo "Usage: RSHARE_IOS_TEAM=YOUR_TEAM scripts/install-ios.sh DEVICE_UDID" >&2
  exit 2
fi
root=$(cd "$(dirname "$0")/.." && pwd)
device=$1
derived="$root/target/ios-build"
xcodegen generate --spec "$root/apps/rshare-ios/project.yml"
xcodebuild -project "$root/apps/rshare-ios/RShareMouse.xcodeproj" \
  -scheme RShareMouse -configuration Debug -destination "generic/platform=iOS" \
  -derivedDataPath "$derived" -allowProvisioningUpdates \
  "DEVELOPMENT_TEAM=$RSHARE_IOS_TEAM" build
xcrun devicectl device install app --device "$device" \
  "$derived/Build/Products/Debug-iphoneos/RShareMouse.app"
xcrun devicectl device process launch --device "$device" --terminate-existing com.rsharemouse.ios
