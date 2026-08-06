#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KEXT_PATH="${1:-$ROOT/target/macos-vdisplay/RShareMacVirtualDisplay.kext}"
INSTALL_PATH="${INSTALL_PATH:-/Library/Extensions/RShareMacVirtualDisplay.kext}"
BUNDLE_ID="io.rshare.mouse.vdisplay"
REQUIRED_ARCHS="${REQUIRED_ARCHS:-x86_64 arm64e}"
ALLOW_UNSIGNED_KEXT="${ALLOW_UNSIGNED_KEXT:-0}"

case "$ALLOW_UNSIGNED_KEXT" in
    0|1) ;;
    *)
        echo "ALLOW_UNSIGNED_KEXT must be 0 or 1." >&2
        exit 2
        ;;
esac

if [[ ! -d "$KEXT_PATH" ]]; then
    echo "Missing kext bundle: $KEXT_PATH" >&2
    echo "Run scripts/driver/build-macos-vdisplay.sh first." >&2
    exit 1
fi

if [[ "$EUID" -ne 0 ]]; then
    echo "Loading a macOS kext requires root. Re-run with sudo." >&2
    exit 1
fi

validate_kext_mach_o_type() {
    local executable="$KEXT_PATH/Contents/MacOS/rshare-vdisplay"
    local arch
    local global_symbols
    local load_commands
    local mach_headers
    local section
    local required_architectures=()

    if [[ ! -f "$executable" ]]; then
        echo "Missing kext executable: $executable" >&2
        return 1
    fi
    if ! command -v otool >/dev/null 2>&1; then
        echo "Missing required command: otool" >&2
        return 1
    fi
    if ! command -v nm >/dev/null 2>&1; then
        echo "Missing required command: nm" >&2
        return 1
    fi
    if ! command -v lipo >/dev/null 2>&1; then
        echo "Missing required command: lipo" >&2
        return 1
    fi

    read -r -a required_architectures <<<"$REQUIRED_ARCHS"
    if [[ "${#required_architectures[@]}" -eq 0 ]]; then
        echo "REQUIRED_ARCHS must contain at least one architecture." >&2
        return 1
    fi
    if ! lipo "$executable" -verify_arch "${required_architectures[@]}"; then
        echo "Kext executable is missing a required architecture: ${required_architectures[*]}" >&2
        return 1
    fi

    mach_headers="$(otool -hv -arch all "$executable")"
    if ! printf '%s\n' "$mach_headers" | awk '
        $1 ~ /^MH_/ {
            found = 1
            if ($5 != "KEXTBUNDLE") {
                invalid = 1
            }
        }
        END { exit !(found && !invalid) }
    '; then
        echo "Kext executable is not a Mach-O KEXTBUNDLE for every architecture:" >&2
        echo "$mach_headers" >&2
        return 1
    fi
    for arch in "${required_architectures[@]}"; do
        global_symbols="$(nm -arch "$arch" -g "$executable")"
        if ! grep -Eq '[[:space:]]_kmod_info$' <<<"$global_symbols"; then
            echo "Kext executable does not export _kmod_info for architecture $arch." >&2
            return 1
        fi
        load_commands="$(otool -l -arch "$arch" "$executable")"
        for section in __mod_init_func __mod_term_func; do
            if ! grep -Eq "^[[:space:]]*sectname[[:space:]]+$section$" <<<"$load_commands"; then
                echo "Kext executable is missing $section for architecture $arch." >&2
                return 1
            fi
        done
    done
}

normalize_kext_permissions() {
    local target="${1:?missing kext target}"
    chown -R root:wheel "$target"
    chmod -R go-w "$target"
    find "$target" -type d -exec chmod 755 {} +
    chmod 644 "$target/Contents/Info.plist"
    chmod 755 "$target/Contents/MacOS/rshare-vdisplay"
}

stage_kext_for_auxkc() {
    if [[ "$INSTALL_PATH" != "/Library/Extensions/RShareMacVirtualDisplay.kext" ]]; then
        echo "Refusing unexpected install path: $INSTALL_PATH" >&2
        return 1
    fi

    rm -rf "$INSTALL_PATH"
    /usr/bin/ditto --rsrc --extattr --noqtn "$KEXT_PATH" "$INSTALL_PATH"
    normalize_kext_permissions "$INSTALL_PATH"
}

unsigned_development_kext_allowed() {
    local sip_status

    [[ "$ALLOW_UNSIGNED_KEXT" == "1" ]] || return 1
    sip_status="$(csrutil status 2>/dev/null || true)"
    grep -Fq "System Integrity Protection status: disabled." <<<"$sip_status"
}

validate_kext_mach_o_type

source_is_signed=0
if codesign --verify --strict "$KEXT_PATH" >/dev/null 2>&1; then
    source_is_signed=1
elif ! unsigned_development_kext_allowed; then
    echo "Kext is not signed or failed codesign verification: $KEXT_PATH" >&2
    echo "Rebuild with SIGN_IDENTITY set to a kernel-extension-capable signing identity." >&2
    echo "For an isolated development Mac with SIP already disabled, explicitly set ALLOW_UNSIGNED_KEXT=1." >&2
    exit 1
else
    echo "WARNING: accepting an unsigned development kext because ALLOW_UNSIGNED_KEXT=1 and SIP is disabled." >&2
fi

if [[ "$source_is_signed" -eq 1 ]] && command -v spctl >/dev/null 2>&1; then
    if ! assessment="$(spctl -a -vv -t install "$KEXT_PATH" 2>&1)"; then
        if ! unsigned_development_kext_allowed; then
            echo "$assessment" >&2
            echo "Kext failed Gatekeeper install assessment. Use a notarized Developer ID certificate approved for kernel extensions." >&2
            exit 1
        fi
        echo "$assessment" >&2
        echo "WARNING: continuing after install-policy rejection because unsigned development mode is explicitly enabled." >&2
    else
        echo "$assessment"
    fi
fi

stage_kext_for_auxkc
if [[ "$source_is_signed" -eq 1 ]]; then
    if ! codesign --verify --strict "$INSTALL_PATH" >/dev/null 2>&1; then
        echo "Installed kext failed codesign verification: $INSTALL_PATH" >&2
        exit 1
    fi
fi

kmutil_status=0
if command -v kmutil >/dev/null 2>&1; then
    diagnostics="$(kmutil print-diagnostics -z -p "$INSTALL_PATH" 2>&1)"
    echo "$diagnostics"
    if ! grep -Fq "Dependencies: OK" <<<"$diagnostics"; then
        echo "kmutil diagnostics did not confirm dependency resolution." >&2
        exit 1
    fi
    if grep -Fq "Error:" <<<"$diagnostics"; then
        echo "kmutil diagnostics reported kext errors after ownership normalization." >&2
        exit 1
    fi
    if kmutil_output="$(kmutil load -p "$INSTALL_PATH" 2>&1)"; then
        echo "$kmutil_output"
    else
        kmutil_status=$?
        echo "$kmutil_output" >&2
        case "$kmutil_status" in
            27|28) ;;
            *)
                echo "kmutil load failed with exit code $kmutil_status." >&2
                exit "$kmutil_status"
                ;;
        esac
    fi
elif command -v kextload >/dev/null 2>&1; then
    kextload "$INSTALL_PATH"
else
    echo "Neither kmutil nor kextload was found." >&2
    exit 1
fi

if command -v kmutil >/dev/null 2>&1; then
    loaded_kexts="$(
        kmutil showloaded --bundle-identifier "$BUNDLE_ID" --show loaded --list-only 2>/dev/null || true
    )"
else
    loaded_kexts="$(kextstat -b "$BUNDLE_ID" 2>/dev/null || true)"
fi
if grep -Fq "$BUNDLE_ID" <<<"$loaded_kexts"; then
    echo "rshare_kext_state=loaded"
elif [[ "$kmutil_status" -eq 27 ]]; then
    echo "rshare_kext_state=approval_required"
    echo "macOS staged the kext but requires user approval before rebuilding the Auxiliary Kernel Collection."
    echo "Approve it in System Settings, then rerun this command if macOS does not schedule the rebuild automatically."
elif [[ "$kmutil_status" -eq 28 ]]; then
    echo "rshare_kext_state=reboot_required"
    echo "macOS rebuilt or scheduled the Auxiliary Kernel Collection; restart macOS to activate it."
else
    echo "rshare_kext_state=reboot_required"
    echo "The kext is installed at $INSTALL_PATH, but is not active in the running kernel."
    echo "Approve it in System Settings if prompted, then restart macOS to boot the updated Auxiliary Kernel Collection."
fi
