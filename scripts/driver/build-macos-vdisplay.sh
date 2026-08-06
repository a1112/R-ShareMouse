#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DRIVER_DIR="$ROOT/drivers/macos/rshare-vdisplay"
SRC="$DRIVER_DIR/RShareMacVirtualDisplay.cpp"
PLIST="$DRIVER_DIR/Info.plist"
OUT_ROOT="${OUT_ROOT:-$ROOT/target/macos-vdisplay}"
BUILD_DIR="$OUT_ROOT/build"
KEXT_DIR="$OUT_ROOT/RShareMacVirtualDisplay.kext"
EXECUTABLE="rshare-vdisplay"
SDKROOT="${SDKROOT:-$(xcrun --sdk macosx --show-sdk-path)}"
ARCHS="${ARCHS:-x86_64 arm64e}"
KERNEL_HEADERS="$SDKROOT/System/Library/Frameworks/Kernel.framework/Versions/A/Headers"
bundle_executable="$KEXT_DIR/Contents/MacOS/$EXECUTABLE"

architectures=()
read -r -a architectures <<<"$ARCHS"
if [[ "${#architectures[@]}" -eq 0 ]]; then
    echo "ARCHS must contain at least one architecture." >&2
    exit 1
fi

if [[ ! -d "$KERNEL_HEADERS" ]]; then
    echo "Kernel headers were not found under SDKROOT=$SDKROOT" >&2
    exit 1
fi

rm -rf "$BUILD_DIR" "$KEXT_DIR"
mkdir -p "$BUILD_DIR" "$KEXT_DIR/Contents/MacOS"

arch_outputs=()
for arch in "${architectures[@]}"; do
    obj="$BUILD_DIR/RShareMacVirtualDisplay-$arch.o"
    executable="$BUILD_DIR/$EXECUTABLE-$arch"

    clang++ \
        -c \
        -arch "$arch" \
        -std=c++17 \
        -fapple-kext \
        -fno-exceptions \
        -fno-rtti \
        -fno-common \
        -nostdinc++ \
        -DKERNEL \
        -D__KERNEL__ \
        -I"$KERNEL_HEADERS" \
        -I"$DRIVER_DIR" \
        -Wall \
        -Wextra \
        -Wconversion \
        -Wsign-conversion \
        -Wshadow \
        -Wcast-align \
        -Werror \
        -o "$obj" \
        "$SRC"

    ld \
        -kext \
        -arch "$arch" \
        -syslibroot "$SDKROOT" \
        -o "$executable" \
        "$obj"

    arch_outputs+=("$executable")
done

if [[ "${#arch_outputs[@]}" -eq 1 ]]; then
    cp "${arch_outputs[0]}" "$bundle_executable"
else
    lipo -create "${arch_outputs[@]}" -output "$bundle_executable"
fi

if ! lipo "$bundle_executable" -verify_arch "${architectures[@]}"; then
    echo "Kext executable does not contain every requested architecture: ${architectures[*]}" >&2
    exit 1
fi

mach_headers="$(otool -hv -arch all "$bundle_executable")"
if ! printf '%s\n' "$mach_headers" | awk '
    $1 ~ /^MH_/ {
        found = 1
        if ($5 != "KEXTBUNDLE") {
            invalid = 1
        }
    }
    END { exit !(found && !invalid) }
'; then
    echo "Linker did not produce a Mach-O KEXTBUNDLE for every architecture:" >&2
    echo "$mach_headers" >&2
    exit 1
fi
for arch in "${architectures[@]}"; do
    global_symbols="$(nm -arch "$arch" -g "$bundle_executable")"
    if ! grep -Eq '[[:space:]]_kmod_info$' <<<"$global_symbols"; then
        echo "Kext executable does not export _kmod_info for architecture $arch." >&2
        exit 1
    fi
    load_commands="$(otool -l -arch "$arch" "$bundle_executable")"
    for section in __mod_init_func __mod_term_func; do
        if ! grep -Eq "^[[:space:]]*sectname[[:space:]]+$section$" <<<"$load_commands"; then
            echo "Kext executable is missing $section for architecture $arch." >&2
            exit 1
        fi
    done
done

cp "$PLIST" "$KEXT_DIR/Contents/Info.plist"
plutil -lint "$KEXT_DIR/Contents/Info.plist" >/dev/null
chmod 755 "$bundle_executable"
find "$KEXT_DIR" -name '._*' -delete

if [[ -n "${SIGN_IDENTITY:-}" ]]; then
    codesign --force --timestamp --sign "$SIGN_IDENTITY" "$KEXT_DIR"
else
    echo "Built unsigned kext bundle. Set SIGN_IDENTITY to codesign with a kernel-extension-capable certificate." >&2
fi

echo "$KEXT_DIR"
