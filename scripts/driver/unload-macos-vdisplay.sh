#!/usr/bin/env bash
set -euo pipefail

BUNDLE_ID="${BUNDLE_ID:-io.rshare.mouse.vdisplay}"
INSTALL_PATH="${INSTALL_PATH:-/Library/Extensions/RShareMacVirtualDisplay.kext}"

if [[ "$EUID" -ne 0 ]]; then
    echo "Unloading a macOS kext requires root. Re-run with sudo." >&2
    exit 1
fi

if [[ "$INSTALL_PATH" != "/Library/Extensions/RShareMacVirtualDisplay.kext" ]]; then
    echo "Refusing unexpected install path: $INSTALL_PATH" >&2
    exit 1
fi

uses_kmutil=0
if command -v kmutil >/dev/null 2>&1; then
    uses_kmutil=1
    if unload_output="$(kmutil unload -b "$BUNDLE_ID" 2>&1)"; then
        echo "$unload_output"
    else
        unload_status=$?
        echo "$unload_output" >&2
        case "$unload_status" in
            3)
                echo "$BUNDLE_ID is not active in the running kernel; continuing with AuxKC removal."
                ;;
            27|28)
                echo "macOS deferred the running-kernel unload; continuing with AuxKC removal."
                ;;
            *)
                echo "kmutil unload failed with exit code $unload_status." >&2
                exit "$unload_status"
                ;;
        esac
    fi
elif command -v kextunload >/dev/null 2>&1; then
    if ! kextunload -b "$BUNDLE_ID"; then
        echo "The kext could not be unloaded from the running kernel." >&2
        exit 1
    fi
else
    echo "Neither kmutil nor kextunload was found." >&2
    exit 1
fi

rm -rf "$INSTALL_PATH"
echo "Removed $INSTALL_PATH."
if [[ "$uses_kmutil" -eq 1 ]]; then
    rebuild_status=0
    if rebuild_output="$(kmutil rebuild 2>&1)"; then
        echo "$rebuild_output"
    else
        rebuild_status=$?
        echo "$rebuild_output" >&2
        case "$rebuild_status" in
            27|28) ;;
            *)
                echo "kmutil rebuild failed with exit code $rebuild_status." >&2
                exit "$rebuild_status"
                ;;
        esac
    fi
    if [[ "$rebuild_status" -eq 27 ]]; then
        echo "rshare_kext_state=removal_approval_required"
    else
        echo "rshare_kext_state=reboot_required"
    fi
fi
echo "Restart macOS to boot without $BUNDLE_ID after the Auxiliary Kernel Collection update."
