#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
pkg-config --atleast-version=1.0 libpipewire-0.3
mkdir -p "$ROOT/target/audio-driver/linux"
# pkg-config output consists of compiler flags, deliberately word-split.
c++ -std=c++17 -O2 -Wall -Wextra "$ROOT/drivers/linux/rshare-audio/bridge.cpp" $(pkg-config --cflags --libs libpipewire-0.3) -o "$ROOT/target/audio-driver/linux/rshare-pipewire-bridge"
"$ROOT/target/audio-driver/linux/rshare-pipewire-bridge" --version
