#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/rshare/audio/bin"
mkdir -p "$DEST"
if [ -e "$DEST/rshare-pipewire-bridge" ]; then echo 'Existing bridge preserved. Remove explicitly before updating.' >&2; exit 1; fi
install -m 0755 "$ROOT/target/audio-driver/linux/rshare-pipewire-bridge" "$DEST/rshare-pipewire-bridge"
echo "Installed $DEST/rshare-pipewire-bridge"
