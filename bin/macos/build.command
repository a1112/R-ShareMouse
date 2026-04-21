#!/bin/bash
# Double-clickable macOS build launcher for Finder.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_ROOT"
clear

echo "R-ShareMouse macOS Build"
echo "========================"
echo ""
echo "Choose a build:"
echo "  1) Debug desktop"
echo "  2) Release desktop"
echo "  3) Release .app bundle (default)"
echo "  4) Debug all"
echo "  5) Release all"
echo ""
read -r -p "Selection [3]: " selection
selection="${selection:-3}"

case "$selection" in
    1)
        args=(desktop)
        ;;
    2)
        args=(--release desktop)
        ;;
    3)
        args=(--release --app)
        ;;
    4)
        args=(all)
        ;;
    5)
        args=(--release all)
        ;;
    *)
        echo "Unknown selection: $selection"
        echo ""
        read -r -p "Press Return to close this window..."
        exit 1
        ;;
esac

echo ""
echo "Running: bin/macos/build.sh ${args[*]}"
echo ""
"$SCRIPT_DIR/build.sh" "${args[@]}"

echo ""
echo "Build finished."
read -r -p "Press Return to close this window..."
