#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WIDTH=1920
HEIGHT=1080
REFRESH_RATE_MILLIHZ=60000
SKIP_BUILD=0
LOAD_DRIVER=0
VERIFY_DAEMON=0
KEEP_DISPLAY=0
BUNDLE_ID="io.rshare.mouse.vdisplay"
SERVICE_CLASS="RShareMacVirtualDisplay"
FRIENDLY_NAME="R-SHAREMOUSE"
SERIAL_STRING="RSM00000001"
OUT_ROOT="${OUT_ROOT:-$ROOT/target/macos-vdisplay}"
KEXT_PATH="${KEXT_PATH:-$OUT_ROOT/RShareMacVirtualDisplay.kext}"
REQUIRED_ARCHS="${REQUIRED_ARCHS:-x86_64 arm64e}"
ALLOW_UNSIGNED_KEXT="${ALLOW_UNSIGNED_KEXT:-0}"
SUPPORTED_MODES=(
    "1920x1080@60000"
    "1920x1080@144000"
    "1920x1080@90000"
    "2560x1440@144000"
    "2560x1440@90000"
    "2560x1440@60000"
    "3840x2160@60000"
    "1600x900@60000"
    "1280x720@90000"
    "1280x720@60000"
    "1024x768@75000"
    "1024x768@60000"
)

case "$ALLOW_UNSIGNED_KEXT" in
    0|1) ;;
    *)
        echo "ALLOW_UNSIGNED_KEXT must be 0 or 1." >&2
        exit 2
        ;;
esac

usage() {
    cat <<EOF
Usage: scripts/driver/validate-macos-vdisplay.sh [options]

Options:
  --mode WIDTHxHEIGHT@REFRESH_MILLIHZ
  --width WIDTH
  --height HEIGHT
  --refresh-rate-millihz VALUE
  --skip-build
  --load                              Install kext and submit the AuxKC update
  --verify-daemon-display-topology
  --keep-display
  --help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --mode)
            IFS='@' read -r resolution REFRESH_RATE_MILLIHZ <<<"${2:-}"
            IFS='xX' read -r WIDTH HEIGHT <<<"$resolution"
            shift 2
            ;;
        --width)
            WIDTH="${2:-}"
            shift 2
            ;;
        --height)
            HEIGHT="${2:-}"
            shift 2
            ;;
        --refresh-rate-millihz)
            REFRESH_RATE_MILLIHZ="${2:-}"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --load)
            LOAD_DRIVER=1
            shift
            ;;
        --verify-daemon-display-topology)
            VERIFY_DAEMON=1
            shift
            ;;
        --keep-display)
            KEEP_DISPLAY=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

MODE="${WIDTH}x${HEIGHT}@${REFRESH_RATE_MILLIHZ}"

step() {
    echo ""
    echo "== $* =="
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Missing required command: $1" >&2
        exit 1
    fi
}

assert_supported_mode() {
    for supported in "${SUPPORTED_MODES[@]}"; do
        if [[ "$supported" == "$MODE" ]]; then
            return
        fi
    done

    echo "Unsupported virtual display mode: $MODE" >&2
    echo "Supported modes: ${SUPPORTED_MODES[*]}" >&2
    exit 1
}

is_kext_signed() {
    codesign --verify --strict "$KEXT_PATH" >/dev/null 2>&1
}

unsigned_development_kext_allowed() {
    local sip_status

    [[ "$ALLOW_UNSIGNED_KEXT" == "1" ]] || return 1
    sip_status="$(csrutil status 2>/dev/null || true)"
    rg -Fq "System Integrity Protection status: disabled." <<<"$sip_status"
}

is_rshare_kext_loaded() {
    local loaded_kexts
    if ! loaded_kexts="$(kmutil showloaded --bundle-identifier "$BUNDLE_ID" --show all --list-only 2>/dev/null)"; then
        return 1
    fi
    rg -Fq "$BUNDLE_ID" <<<"$loaded_kexts"
}

run_kmutil_diagnostics() {
    kmutil print-diagnostics -z -p "$KEXT_PATH" 2>&1
}

run_spctl_install_assessment() {
    spctl -a -vv -t install "$KEXT_PATH" 2>&1
}

cleanup_virtual_display() {
    if [[ "$KEEP_DISPLAY" -eq 1 ]]; then
        return
    fi

    cargo run -p rshare-cli -- display virtual remove --id rshare-vdisplay-1 >/dev/null 2>&1 || true
}

wait_for_loaded_kext() {
    local deadline=$((SECONDS + 5))
    while [[ "$SECONDS" -lt "$deadline" ]]; do
        if is_rshare_kext_loaded; then
            echo "$BUNDLE_ID is loaded"
            return 0
        fi
        sleep 1
    done

    return 1
}

wait_for_driver_status() {
    local deadline=$((SECONDS + 30))
    local last_output=""
    while [[ "$SECONDS" -lt "$deadline" ]]; do
        if output="$(cargo run -p rshare-cli -- display virtual driver-status --strict 2>&1)"; then
            echo "$output"
            return
        fi
        last_output="$output"
        echo "$output"
        sleep 1
    done

    echo "Timed out waiting for macOS virtual display user client." >&2
    echo "$last_output" >&2
    exit 1
}

validate_ioreg_service_output() {
    local output="$1"
    local missing=()
    local required_patterns=(
        "$SERVICE_CLASS"
        "RShareVirtualDisplay"
        "IODisplayEDID"
        "IODisplayEDIDOriginal"
        "DisplayVendorID"
        "DisplayProductID"
        "DisplaySerialNumber"
        "$FRIENDLY_NAME"
        "$SERIAL_STRING"
    )

    for pattern in "${required_patterns[@]}"; do
        if ! rg -Fq "$pattern" <<<"$output"; then
            missing+=("$pattern")
        fi
    done

    if [[ "${#missing[@]}" -gt 0 ]]; then
        printf 'missing IORegistry identity properties: %s\n' "${missing[*]}" >&2
        return 1
    fi
}

wait_for_ioreg_service() {
    local deadline=$((SECONDS + 30))
    local last_output=""
    while [[ "$SECONDS" -lt "$deadline" ]]; do
        output="$(ioreg -r -c "$SERVICE_CLASS" -d 2 2>/dev/null || true)"
        if [[ -n "$output" ]] && validate_ioreg_service_output "$output" >/dev/null 2>&1; then
            echo "$output"
            return
        fi
        last_output="$output"
        sleep 1
    done

    echo "Timed out waiting for $SERVICE_CLASS IORegistry identity properties." >&2
    if [[ -n "$last_output" ]]; then
        validate_ioreg_service_output "$last_output" || true
        echo "$last_output" >&2
    fi
    exit 1
}

wait_for_daemon_topology() {
    local deadline=$((SECONDS + 30))
    local last_output=""
    while [[ "$SECONDS" -lt "$deadline" ]]; do
        if output="$(cargo run -p rshare-cli -- display virtual verify --mode "$MODE" 2>&1)"; then
            echo "$output"
            return
        fi
        last_output="$output"
        echo "$output"
        sleep 1
    done

    echo "Timed out waiting for daemon display topology verification." >&2
    echo "$last_output" >&2
    exit 1
}

wait_for_daemon_virtual_display_api() {
    local deadline=$((SECONDS + 30))
    local last_output=""
    while [[ "$SECONDS" -lt "$deadline" ]]; do
        if output="$(cargo run -p rshare-cli -- display virtual list 2>&1)"; then
            echo "$output"
            return
        fi
        last_output="$output"
        echo "$output"
        sleep 1
    done

    echo "Timed out waiting for daemon virtual display IPC." >&2
    echo "$last_output" >&2
    exit 1
}

assert_supported_mode
require_command xcrun
require_command clang++
require_command plutil
require_command file
require_command lipo
require_command nm
require_command otool
require_command codesign
require_command ioreg
require_command kmutil
require_command spctl
require_command rg
if [[ "$LOAD_DRIVER" -eq 1 && "$EUID" -ne 0 ]]; then
    require_command sudo
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
    step "Build macOS virtual display kext"
    "$ROOT/scripts/driver/check-macos-vdisplay.sh"
    "$ROOT/scripts/driver/build-macos-vdisplay.sh"
fi

step "Validate kext bundle"
if [[ ! -d "$KEXT_PATH" ]]; then
    echo "Missing kext bundle: $KEXT_PATH" >&2
    exit 1
fi
test -f "$KEXT_PATH/Contents/Info.plist"
test -f "$KEXT_PATH/Contents/MacOS/rshare-vdisplay"
plutil -lint "$KEXT_PATH/Contents/Info.plist"
executable="$KEXT_PATH/Contents/MacOS/rshare-vdisplay"
file "$executable"
required_architectures=()
read -r -a required_architectures <<<"$REQUIRED_ARCHS"
if [[ "${#required_architectures[@]}" -eq 0 ]]; then
    echo "REQUIRED_ARCHS must contain at least one architecture." >&2
    exit 1
fi
if ! lipo "$executable" -verify_arch "${required_architectures[@]}"; then
    echo "Kext executable is missing a required architecture: ${required_architectures[*]}" >&2
    exit 1
fi
mach_headers="$(otool -hv -arch all "$executable")"
echo "$mach_headers"
if ! printf '%s\n' "$mach_headers" | awk '
    $1 ~ /^MH_/ {
        found = 1
        if ($5 != "KEXTBUNDLE") {
            invalid = 1
        }
    }
    END { exit !(found && !invalid) }
'; then
    echo "Kext executable is not a Mach-O KEXTBUNDLE for every architecture." >&2
    exit 1
fi
appledouble_files="$(find "$KEXT_PATH" -name '._*' -print)"
if [[ -n "$appledouble_files" ]]; then
    echo "AppleDouble metadata files were found in the kext bundle." >&2
    echo "$appledouble_files" >&2
    exit 1
fi

step "Check kext symbol hygiene"
for arch in "${required_architectures[@]}"; do
    global_symbols="$(nm -arch "$arch" -g "$executable")"
    if ! rg -q '[[:space:]]_kmod_info$' <<<"$global_symbols"; then
        echo "Kext executable does not export _kmod_info for architecture $arch." >&2
        exit 1
    fi
    load_commands="$(otool -l -arch "$arch" "$executable")"
    for section in __mod_init_func __mod_term_func; do
        if ! rg -q "^[[:space:]]*sectname[[:space:]]+$section$" <<<"$load_commands"; then
            echo "Kext executable is missing $section for architecture $arch." >&2
            exit 1
        fi
    done
done
all_symbols="$(nm -m "$KEXT_PATH/Contents/MacOS/rshare-vdisplay")"
forbidden_symbols="$(
    rg '(__cxa|___cxa|___gxx_personality|__ZGV|_objc_|_swift_|dyld_stub_binder)' \
        <<<"$all_symbols" || true
)"
if [[ -n "$forbidden_symbols" ]]; then
    echo "Kext executable references user-space C++/Objective-C/Swift runtime symbols:" >&2
    echo "$forbidden_symbols" >&2
    exit 1
fi
echo "No forbidden user-space runtime symbols found"

step "Run kmutil diagnostics"
diagnostics="$(run_kmutil_diagnostics)"
echo "$diagnostics"
if ! rg -Fq "Dependencies: OK" <<<"$diagnostics"; then
    echo "kmutil diagnostics did not confirm dependency resolution." >&2
    exit 1
fi
unexpected_diagnostics_errors="$(
    printf '%s\n' "$diagnostics" \
        | rg '^\s*(Error:|.*error:)' \
        | rg -v '^\s*Error:\s*$|Invalid ownership' || true
)"
if [[ -n "$unexpected_diagnostics_errors" ]]; then
    echo "kmutil diagnostics reported unexpected errors:" >&2
    echo "$unexpected_diagnostics_errors" >&2
    exit 1
fi
if rg -Fq "Invalid ownership" <<<"$diagnostics"; then
    echo "kmutil diagnostics reported user-owned bundle files; load-macos-vdisplay.sh normalizes root:wheel under sudo."
fi

step "Check kernel dependency state"
if ! loaded_iographics="$(
    kmutil showloaded --bundle-identifier com.apple.iokit.IOGraphicsFamily --show loaded --list-only
)"; then
    echo "Failed to query the loaded IOGraphicsFamily dependency." >&2
    exit 1
fi
if ! rg -Fq "com.apple.iokit.IOGraphicsFamily" <<<"$loaded_iographics"; then
    echo "com.apple.iokit.IOGraphicsFamily is not reported as loaded." >&2
    exit 1
fi

if is_kext_signed; then
    echo "codesign verification passed"
    step "Assess kext install signing policy"
    if assessment="$(run_spctl_install_assessment)"; then
        echo "$assessment"
    elif unsigned_development_kext_allowed; then
        echo "$assessment"
        echo "WARNING: continuing after install-policy rejection because ALLOW_UNSIGNED_KEXT=1 and SIP is disabled."
    else
        echo "$assessment"
        echo "spctl install assessment rejected the kext signature."
        echo "Use a Developer ID certificate approved for kernel extensions before loading or daemon topology validation."
        if [[ "$LOAD_DRIVER" -eq 1 || "$VERIFY_DAEMON" -eq 1 ]]; then
            exit 1
        fi
    fi
else
    echo "codesign verification failed: kext is unsigned or not accepted by local policy"
    if unsigned_development_kext_allowed; then
        echo "WARNING: accepting an unsigned development kext because ALLOW_UNSIGNED_KEXT=1 and SIP is disabled."
    elif [[ "$LOAD_DRIVER" -eq 1 || "$VERIFY_DAEMON" -eq 1 ]]; then
        echo "Rebuild with SIGN_IDENTITY set before loading or daemon topology validation." >&2
        echo "For an isolated development Mac with SIP already disabled, explicitly set ALLOW_UNSIGNED_KEXT=1." >&2
        exit 1
    fi
fi

if is_rshare_kext_loaded; then
    echo "$BUNDLE_ID is already loaded"
else
    echo "$BUNDLE_ID is not loaded"
fi

step "Probe virtual display user client"
cargo run -p rshare-cli -- display virtual driver-status

if [[ "$LOAD_DRIVER" -eq 1 ]]; then
    step "Install kext and submit Auxiliary Kernel Collection update"
    if [[ "$EUID" -eq 0 ]]; then
        "$ROOT/scripts/driver/load-macos-vdisplay.sh" "$KEXT_PATH"
    else
        sudo env ALLOW_UNSIGNED_KEXT="$ALLOW_UNSIGNED_KEXT" \
            "$ROOT/scripts/driver/load-macos-vdisplay.sh" "$KEXT_PATH"
    fi
    if ! wait_for_loaded_kext; then
        step "Kext installation pending approval or restart"
        echo "$BUNDLE_ID is installed but not active in the running kernel."
        echo "On Apple silicon, use Startup Security Utility to select Reduced Security and allow user management of kernel extensions."
        echo "Approve the R-ShareMouse extension in System Settings if prompted, restart macOS, then rerun:"
        echo "  scripts/driver/validate-macos-vdisplay.sh --skip-build --verify-daemon-display-topology"
        exit 3
    fi
    step "Verify IORegistry service identity"
    wait_for_ioreg_service
    wait_for_driver_status
fi

if [[ "$VERIFY_DAEMON" -eq 1 ]]; then
    if ! is_rshare_kext_loaded; then
        echo "$BUNDLE_ID must be loaded before daemon topology verification." >&2
        exit 1
    fi

    step "Verify daemon display topology"
    trap cleanup_virtual_display EXIT
    cargo build -p rshare-daemon -p rshare-cli
    wait_for_ioreg_service
    wait_for_driver_status
    cargo run -p rshare-cli -- start --daemon
    wait_for_daemon_virtual_display_api
    cargo run -p rshare-cli -- display virtual create --mode "$MODE" --name "R-ShareMouse Virtual Display"
    wait_for_daemon_topology
fi

step "macOS virtual display validation preflight passed"
