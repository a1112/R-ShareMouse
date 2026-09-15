#!/bin/sh
set -eu
DEST=/Library/Audio/Plug-Ins/HAL/RShareAudio.driver
HELPER=/Library/PrivilegedHelperTools/org.rshare.audio.broker
PLIST=/Library/LaunchDaemons/org.rshare.audio.broker.plist
if [ "$(id -u)" -ne 0 ]; then echo 'Run with sudo.' >&2; exit 1; fi
if [ ! -e "$DEST" ]; then echo 'No RShare audio driver installation found.'; exit 0; fi
ID=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$DEST/Contents/Info.plist")
if [ "$ID" != org.rshare.audio.driver ]; then echo 'Unexpected bundle identity; refusing removal.' >&2; exit 1; fi
BACKUP=$(mktemp -d /Library/Audio/Plug-Ins/.rshare-audio-uninstalled-XXXXXX)
if [ -e "$PLIST" ]; then
    LABEL=$(/usr/libexec/PlistBuddy -c 'Print :Label' "$PLIST")
    if [ "$LABEL" != org.rshare.audio.broker ]; then echo 'Unexpected helper identity; refusing removal.' >&2; exit 1; fi
    if launchctl print system/org.rshare.audio.broker >/dev/null 2>&1; then launchctl bootout system "$PLIST"; fi
    mv "$PLIST" "$BACKUP/org.rshare.audio.broker.plist"
fi
if [ -e "$HELPER" ]; then mv "$HELPER" "$BACKUP/org.rshare.audio.broker"; fi
mv "$DEST" "$BACKUP/RShareAudio.driver"
echo "Removed from HAL; recoverable driver and helper backup: $BACKUP"
echo 'Reload audio at a convenient time with: sudo killall coreaudiod'
