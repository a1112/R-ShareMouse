#!/bin/bash
# R-ShareMouse Run Script for macOS
#
# Usage:
#   ./bin/macos/run.sh              # Run daemon
#   ./bin/macos/run.sh daemon       # Run daemon
#   ./bin/macos/run.sh cli status   # Run CLI command
#   ./bin/macos/run.sh app          # Run .app bundle

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Parse arguments
TARGET="daemon"
CLI_ARGS=()
BUILD_MODE="debug"
USE_APP=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            BUILD_MODE="release"
            shift
            ;;
        --app)
            USE_APP=true
            TARGET="desktop"
            shift
            ;;
        daemon|gui|desktop|app)
            TARGET="$1"
            if [ "$1" = "app" ]; then
                USE_APP=true
                TARGET="desktop"
            fi
            shift
            ;;
        cli)
            TARGET="cli"
            shift
            CLI_ARGS=("$@")
            break
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS] [TARGET] [ARGS...]"
            echo ""
            echo "Options:"
            echo "  --release    Use release build (default: debug)"
            echo "  --app        Use .app bundle (for desktop target)"
            echo ""
            echo "Targets:"
            echo "  daemon       Run rshare-daemon (default)"
            echo "  cli          Run rshare CLI with args"
            echo "  gui          Run rshare-gui"
            echo "  desktop      Run rshare-desktop (Tauri)"
            echo "  app          Run .app bundle (same as --app desktop)"
            echo ""
            echo "Examples:"
            echo "  $0                      # Run daemon"
            echo "  $0 cli status           # Run 'rshare status'"
            echo "  $0 cli devices          # Run 'rshare devices'"
            echo "  $0 app                  # Run .app bundle"
            echo "  $0 --app                # Same as above"
            echo "  $0 --release daemon     # Run release build"
            exit 0
            ;;
        *)
            if [ "$TARGET" = "daemon" ] || [ "$TARGET" = "gui" ] || [ "$TARGET" = "desktop" ]; then
                echo -e "${RED}Unknown option: $1${NC}"
                exit 1
            fi
            ;;
    esac
done

# Determine binary directory
if [ "$BUILD_MODE" = "release" ]; then
    BIN_DIR="target/release"
else
    BIN_DIR="target/debug"
fi

# Run function
run_target() {
    local target=$1
    case $target in
        daemon)
            echo -e "${GREEN}Starting rshare-daemon...${NC}"
            if [ ! -f "$BIN_DIR/rshare-daemon" ]; then
                echo -e "${YELLOW}Daemon not found. Building first...${NC}"
                "$(dirname "$0")/build.sh" $([ "$BUILD_MODE" = "release" ] && echo "--release") daemon
            fi
            "$BIN_DIR/rshare-daemon"
            ;;
        cli)
            if [ ! -f "$BIN_DIR/rshare" ]; then
                echo -e "${YELLOW}CLI not found. Building first...${NC}"
                "$(dirname "$0")/build.sh" $([ "$BUILD_MODE" = "release" ] && echo "--release") cli
            fi
            "$BIN_DIR/rshare" "${CLI_ARGS[@]}"
            ;;
        gui)
            echo -e "${GREEN}Starting rshare-gui...${NC}"
            if [ ! -f "$BIN_DIR/rshare-gui" ]; then
                echo -e "${YELLOW}GUI not found. Building first...${NC}"
                "$(dirname "$0")/build.sh" $([ "$BUILD_MODE" = "release" ] && echo "--release") gui
            fi
            "$BIN_DIR/rshare-gui" &
            echo -e "${GREEN}GUI started in background${NC}"
            ;;
        desktop)
            if [ "$USE_APP" = true ]; then
                echo -e "${GREEN}Starting R-ShareMouse.app...${NC}"
                if [ ! -d "R-ShareMouse.app" ]; then
                    echo -e "${YELLOW}.app bundle not found. Building first...${NC}"
                    "$(dirname "$0")/build.sh" --app
                fi
                open "R-ShareMouse.app"
            else
                echo -e "${GREEN}Starting rshare-desktop...${NC}"
                if [ ! -f "$BIN_DIR/rshare-gui" ]; then
                    echo -e "${YELLOW}Desktop app not found. Building first...${NC}"
                    "$(dirname "$0")/build.sh" $([ "$BUILD_MODE" = "release" ] && echo "--release") desktop
                fi
                "$BIN_DIR/rshare-gui" &
                echo -e "${GREEN}Desktop app started in background${NC}"
            fi
            ;;
    esac
}

# Run target
run_target "$TARGET"
