#!/bin/sh
# Explicit developer installation; never restarts coreaudiod or changes defaults.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
SRC="$ROOT/target/audio-driver/macos"
DEST=/Library/Audio/Plug-Ins/HAL/RShareAudio.driver
HELPER=/Library/PrivilegedHelperTools/org.rshare.audio.broker
PLIST=/Library/LaunchDaemons/org.rshare.audio.broker.plist
if [ "$(id -u)" -ne 0 ]; then echo 'Run with sudo after building the plug-in.' >&2; exit 1; fi
for item in "$DEST" "$HELPER" "$PLIST"; do
    if [ -e "$item" ]; then echo "Existing installation preserved: $item. Uninstall explicitly before replacing it." >&2; exit 1; fi
done
codesign --verify --strict "$SRC/RShareAudio.driver"
codesign --verify --strict "$SRC/org.rshare.audio.broker"
mkdir -p /Library/Audio/Plug-Ins/HAL /Library/PrivilegedHelperTools /Library/LaunchDaemons
STAGE=$(mktemp -d /Library/Audio/Plug-Ins/HAL/.rshare-audio-XXXXXX)
PLUGIN_MOVED=0
HELPER_MOVED=0
PLIST_MOVED=0
SUCCESS=0
cleanup() {
    if [ "$SUCCESS" -eq 0 ]; then
        if [ "$PLIST_MOVED" -eq 1 ]; then launchctl bootout system "$PLIST" 2>/dev/null || true; rm -f "$PLIST"; fi
        if [ "$HELPER_MOVED" -eq 1 ]; then rm -f "$HELPER"; fi
        if [ "$PLUGIN_MOVED" -eq 1 ]; then rm -rf "$DEST"; fi
    fi
    rm -rf "$STAGE"
}
trap cleanup EXIT HUP INT TERM
ditto "$SRC/RShareAudio.driver" "$STAGE/RShareAudio.driver"
cp "$SRC/org.rshare.audio.broker" "$STAGE/broker"
cp "$ROOT/drivers/macos/rshare-audio/org.rshare.audio.broker.plist" "$STAGE/broker.plist"
chown -R root:wheel "$STAGE"
chmod -R go-w "$STAGE/RShareAudio.driver"
chmod 0755 "$STAGE/broker"
chmod 0644 "$STAGE/broker.plist"
mv "$STAGE/RShareAudio.driver" "$DEST"
PLUGIN_MOVED=1
mv "$STAGE/broker" "$HELPER"
HELPER_MOVED=1
mv "$STAGE/broker.plist" "$PLIST"
PLIST_MOVED=1
launchctl bootstrap system "$PLIST"
SUCCESS=1
echo 'Installed driver and memory broker. Reload audio at a convenient time with: sudo killall coreaudiod'
