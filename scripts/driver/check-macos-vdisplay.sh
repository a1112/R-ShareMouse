#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SDK_PATH="${SDKROOT:-$(xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)}"

if [[ -z "${SDK_PATH}" || ! -d "${SDK_PATH}" ]]; then
  echo "macOS SDK was not found. Install Xcode or set SDKROOT." >&2
  exit 1
fi

KERNEL_HEADERS="${SDK_PATH}/System/Library/Frameworks/Kernel.framework/Versions/A/Headers"
if [[ ! -d "${KERNEL_HEADERS}" ]]; then
  echo "Kernel framework headers were not found under ${SDK_PATH}." >&2
  exit 1
fi

SOURCE="${ROOT_DIR}/drivers/macos/rshare-vdisplay/RShareMacVirtualDisplay.cpp"
COMMON_FLAGS=(
  -std=c++17
  -fapple-kext
  -fno-exceptions
  -fno-rtti
  -fno-common
  -nostdinc++
  -DKERNEL
  -D__KERNEL__
  -I"${KERNEL_HEADERS}"
  -I"${ROOT_DIR}/drivers/macos/rshare-vdisplay"
  -Wall
  -Wextra
  -Wconversion
  -Wsign-conversion
  -Wshadow
  -Wcast-align
  -Werror
)

clang++ -fsyntax-only "${COMMON_FLAGS[@]}" "${SOURCE}"
clang++ --analyze \
  "${COMMON_FLAGS[@]}" \
  -Xanalyzer -analyzer-output=text \
  -o /dev/null \
  "${SOURCE}"

echo "macOS virtual display driver syntax and static analysis checks passed"
