#!/bin/sh
set -eu
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/rshare/audio/bin/rshare-pipewire-bridge"
if [ ! -f "$DEST" ]; then exit 0; fi
if [ -e "$DEST.uninstalled" ]; then echo 'Existing backup preserved; move it before uninstalling.' >&2; exit 1; fi
mv "$DEST" "$DEST.uninstalled"
echo "Uninstalled. Backup: $DEST.uninstalled"
