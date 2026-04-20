#!/bin/bash
# R-ShareMouse Build Script for Linux/macOS
#
# Usage:
#   ./bin/build.sh              # Build all (debug)
#   ./bin/build.sh --release    # Build all (release)
#   ./bin/build.sh daemon       # Build daemon only
#   ./bin/build.sh --release gui

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Parse arguments
BUILD_MODE="debug"
TARGET="all"

while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            BUILD_MODE="release"
            shift
            ;;
        debug)
            BUILD_MODE="debug"
            shift
            ;;
        daemon|cli|gui|desktop)
            TARGET="$1"
            shift
            ;;
        all)
            TARGET="all"
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS] [TARGET]"
            echo ""
            echo "Options:"
            echo "  --release    Build in release mode (default: debug)"
            echo "  debug        Build in debug mode"
            echo ""
            echo "Targets:"
            echo "  all          Build all binaries (default)"
            echo "  daemon       Build rshare-daemon"
            echo "  cli          Build rshare CLI"
            echo "  gui          Build rshare-gui"
            echo "  desktop      Build rshare-desktop (Tauri)"
            echo ""
            echo "Examples:"
            echo "  $0                    # Build all in debug mode"
            echo "  $0 --release          # Build all in release mode"
            echo "  $0 --release daemon   # Build daemon in release mode"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Build flags
if [ "$BUILD_MODE" = "release" ]; then
    BUILD_FLAG="--release"
    echo -e "${GREEN}Building in RELEASE mode...${NC}"
else
    BUILD_FLAG=""
    echo -e "${YELLOW}Building in DEBUG mode...${NC}"
fi

# Build function
build_target() {
    local target=$1
    case $target in
        all)
            echo -e "${GREEN}Building all binaries...${NC}"
            cargo build $BUILD_FLAG --workspace
            ;;
        daemon)
            echo -e "${GREEN}Building rshare-daemon...${NC}"
            cargo build $BUILD_FLAG -p rshare-daemon
            ;;
        cli)
            echo -e "${GREEN}Building rshare CLI...${NC}"
            cargo build $BUILD_FLAG -p rshare-cli
            ;;
        gui)
            echo -e "${GREEN}Building rshare-gui...${NC}"
            cargo build $BUILD_FLAG -p rshare-gui
            ;;
        desktop)
            echo -e "${GREEN}Building rshare-desktop (Tauri)...${NC}"
            cargo build $BUILD_FLAG -p rshare-desktop
            ;;
        *)
            echo -e "${RED}Unknown target: $target${NC}"
            exit 1
            ;;
    esac
}

# Run build
build_target "$TARGET"

# Show results
echo ""
echo -e "${GREEN}Build completed!${NC}"
echo ""
echo "Binaries location:"
if [ "$BUILD_MODE" = "release" ]; then
    echo "  target/release/rshare        # CLI"
    echo "  target/release/rshare-daemon # Daemon"
    echo "  target/release/rshare-gui    # GUI"
    echo "  target/release/rshare-gui    # Desktop (Tauri)"
else
    echo "  target/debug/rshare        # CLI"
    echo "  target/debug/rshare-daemon # Daemon"
    echo "  target/debug/rshare-gui    # GUI"
    echo "  target/debug/rshare-gui    # Desktop (Tauri)"
fi
