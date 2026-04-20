#!/bin/bash
# R-ShareMouse Build Script for macOS
#
# Usage:
#   ./bin/macos/build.sh              # Build all (debug)
#   ./bin/macos/build.sh --release    # Build all (release)
#   ./bin/macos/build.sh daemon       # Build daemon only
#   ./bin/macos/build.sh --app        # Build .app bundle

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Parse arguments
BUILD_MODE="debug"
TARGET="all"
BUILD_APP=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            BUILD_MODE="release"
            shift
            ;;
        --app)
            BUILD_APP=true
            TARGET="desktop"
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
            echo "  --app        Build macOS .app bundle (requires desktop target)"
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
            echo "  $0 --app              # Build .app bundle"
            echo "  $0 --release --app    # Build release .app bundle"
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

# Build .app bundle
build_app_bundle() {
    local app_name="R-ShareMouse.app"
    local contents_dir="$app_name/Contents"
    local macos_dir="$contents_dir/MacOS"
    local resources_dir="$contents_dir/Resources"
    local exe_dir="target/$BUILD_MODE"

    echo -e "${BLUE}Creating .app bundle...${NC}"

    # Remove existing .app
    rm -rf "$app_name"

    # Create directory structure
    mkdir -p "$macos_dir"
    mkdir -p "$resources_dir"

    # Create Info.plist
    cat > "$contents_dir/Info.plist" << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>rshare-gui</string>
    <key>CFBundleIdentifier</key>
    <string>com.rshare.mouse</string>
    <key>CFBundleName</key>
    <string>R-ShareMouse</string>
    <key>CFBundleVersion</key>
    <string>1.0.0</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0.0</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSUIElement</key>
    <false/>
    <key>NSAppleScriptEnabled</key>
    <false/>
</dict>
</plist>
EOF

    # Copy executable
    cp "$exe_dir/rshare-gui" "$macos_dir/"

    # Copy icon if exists
    if [ -f "apps/rshare-desktop/src-tauri/icons/icon.icns" ]; then
        cp "apps/rshare-desktop/src-tauri/icons/icon.icns" "$resources_dir/"
    fi

    echo -e "${GREEN}Created: $app_name${NC}"
}

# Run build
build_target "$TARGET"

# Build .app if requested
if [ "$BUILD_APP" = true ]; then
    build_app_bundle
fi

# Show results
echo ""
echo -e "${GREEN}Build completed!${NC}"
echo ""
echo "Binaries location:"
if [ "$BUILD_MODE" = "release" ]; then
    echo "  target/release/rshare        # CLI"
    echo "  target/release/rshare-daemon # Daemon"
    echo "  target/release/rshare-gui    # GUI"
    if [ "$BUILD_APP" = true ]; then
        echo "  R-ShareMouse.app          # macOS App Bundle"
    fi
else
    echo "  target/debug/rshare        # CLI"
    echo "  target/debug/rshare-daemon # Daemon"
    echo "  target/debug/rshare-gui    # GUI"
    if [ "$BUILD_APP" = true ]; then
        echo "  R-ShareMouse.app          # macOS App Bundle"
    fi
fi
