#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
mkdir -p "$ROOT/target/audio-driver/macos"
xcrun clang++ -std=c++17 -Wall -Wextra -Werror -g -fsanitize=address,undefined -framework CoreAudio -framework CoreFoundation "$ROOT/drivers/macos/rshare-audio/driver_test.cpp" -o "$ROOT/target/audio-driver/macos/driver-test"
"$ROOT/target/audio-driver/macos/driver-test"
