#!/bin/bash
# R-ShareMouse macOS Setup Script
#
# This script sets up R-ShareMouse on macOS including:
# - Accessibility permissions (required for global hotkeys)
# - Full disk access (optional, for file drag-drop)
# - Launch agent for auto-start

set -e

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}================================================${NC}"
echo -e "${BLUE}  R-ShareMouse macOS Setup${NC}"
echo -e "${BLUE}================================================${NC}"
echo ""

# Check if running on macOS
if [[ "$OSTYPE" != "darwin"* ]]; then
    echo "This script is for macOS only."
    exit 1
fi

# Build the app first
echo -e "${YELLOW}Building R-ShareMouse...${NC}"
./bin/macos/build.sh --release --app

# Create LaunchAgent plist for auto-start
echo ""
echo -e "${YELLOW}Setting up LaunchAgent for auto-start...${NC}"

LAUNCH_AGENTS_DIR="$HOME/Library/LaunchAgents"
PLIST_FILE="$LAUNCH_AGENTS_DIR/com.rshare.mouse.plist"

mkdir -p "$LAUNCH_AGENTS_DIR"

cat > "$PLIST_FILE" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.rshare.mouse</string>
    <key>ProgramArguments</key>
    <array>
        <string>$(pwd)/R-ShareMouse.app/Contents/MacOS/rshare-gui</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
        <key>Crashed</key>
        <true/>
    </dict>
</dict>
</plist>
EOF

echo -e "${GREEN}Created LaunchAgent plist${NC}"

echo ""

# Firewall configuration
echo -e "${YELLOW}Checking firewall configuration...${NC}"
./bin/macos/firewall.sh status

echo ""
echo -e "${YELLOW}Note: macOS firewall configuration requires root privileges${NC}"
echo "To configure firewall, run:"
echo "  sudo ./bin/macos/firewall.sh enable"

echo ""
echo -e "${GREEN}================================================${NC}"
echo -e "${GREEN}  Setup Complete!${NC}"
echo -e "${GREEN}================================================${NC}"
echo ""
echo "Next steps:"
echo ""
echo "1. Grant Accessibility permissions:"
echo "   - Open System Settings > Privacy & Security > Accessibility"
echo "   - Click '+' and add R-ShareMouse.app"
echo "   - Enable the checkbox"
echo ""
echo "2. (Optional) Grant Full Disk Access for file drag-drop:"
echo "   - Open System Settings > Privacy & Security > Full Disk Access"
echo "   - Click '+' and add R-ShareMouse.app"
echo ""
echo "3. Enable auto-start (optional):"
echo "   - Run: launchctl load '$PLIST_FILE'"
echo ""
echo "4. Launch the app:"
echo "   - Double-click R-ShareMouse.app"
echo "   - Or run: ./bin/macos/run.sh app"
echo ""
