#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
OUT="$ROOT/target/audio-driver/macos/RShareAudio.driver"
mkdir -p "$OUT/Contents/MacOS"
xcrun clang++ -std=c++17 -fblocks -Wall -Wextra -Werror -O2 -fvisibility=hidden -arch arm64 -arch x86_64 -mmacosx-version-min=13.0 -bundle -framework CoreAudio -framework CoreFoundation -Wl,-exported_symbol,_RShareAudioFactory "$ROOT/drivers/macos/rshare-audio/driver.cpp" -o "$OUT/Contents/MacOS/RShareAudio"
cp "$ROOT/drivers/macos/rshare-audio/Info.plist" "$OUT/Contents/Info.plist"
codesign --force --sign - "$OUT"
plutil -lint "$OUT/Contents/Info.plist"

xcrun clang++ -std=c++17 -fblocks -Wall -Wextra -Werror -O2 -arch arm64 -arch x86_64 -mmacosx-version-min=13.0 "$ROOT/drivers/macos/rshare-audio/broker.cpp" -o "$ROOT/target/audio-driver/macos/org.rshare.audio.broker"
codesign --force --sign - "$ROOT/target/audio-driver/macos/org.rshare.audio.broker"
