#!/bin/bash
# R-ShareMouse Linux Setup Script
#
# This script sets up R-ShareMouse on Linux including:
# - Systemd service for auto-start
# - X11/Wayland detection
# - Permission checks

set -e

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m'

# Check if running on Linux
if [[ "$OSTYPE" != "linux-gnu"* ]]; then
    echo -e "${RED}This script is for Linux only.${NC}"
    exit 1
fi

echo -e "${BLUE}================================================${NC}"
echo -e "${BLUE}  R-ShareMouse Linux Setup${NC}"
echo -e "${BLUE}================================================${NC}"
echo ""

# Detect display server
if [ -n "$WAYLAND_DISPLAY" ]; then
    DISPLAY_SERVER="Wayland"
    echo -e "${YELLOW}Detected: Wayland${NC}"
    echo -e "${YELLOW}Note: Wayland has limited global input support.${NC}"
    echo -e "${YELLOW}Consider using X11 for full functionality.${NC}"
elif [ -n "$DISPLAY" ]; then
    DISPLAY_SERVER="X11"
    echo -e "${GREEN}Detected: X11${NC}"
else
    DISPLAY_SERVER="Unknown"
    echo -e "${RED}No display server detected!${NC}"
    echo -e "${RED}Make sure you're running in a graphical session.${NC}"
    exit 1
fi

echo ""

# Check for required dependencies
echo -e "${YELLOW}Checking dependencies...${NC}"

MISSING_DEPS=()

# Check for X11 development files
if [ "$DISPLAY_SERVER" = "X11" ]; then
    if ! pkg-config --exists x11 2>/dev/null; then
        MISSING_DEPS+=("libx11-dev")
    fi
    if ! pkg-config --exists xtst 2>/dev/null; then
        MISSING_DEPS+=("libxtst-dev")
    fi
fi

if [ ${#MISSING_DEPS[@]} -gt 0 ]; then
    echo -e "${RED}Missing dependencies: ${MISSING_DEPS[*]}${NC}"
    echo ""
    echo "Install them with:"
    echo "  Debian/Ubuntu: sudo apt-get install ${MISSING_DEPS[*]}"
    echo "  Fedora/RHEL: sudo dnf install ${MISSING_DEPS[*]}"
    echo "  Arch: sudo pacman -S ${MISSING_DEPS[*]}"
    echo ""
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
else
    echo -e "${GREEN}All dependencies satisfied${NC}"
fi

echo ""

# Build the project
echo -e "${YELLOW}Building R-ShareMouse...${NC}"
./bin/linux/build.sh --release

# Get the absolute path of the project
PROJECT_DIR="$(pwd)"
DAEMON_BIN="$PROJECT_DIR/target/release/rshare-daemon"
USER="$(whoami)"

# Create systemd service file
SERVICE_FILE="$HOME/.config/systemd/user/rshare-daemon.service"
mkdir -p "$(dirname "$SERVICE_FILE")"

echo -e "${YELLOW}Creating systemd service...${NC}"

cat > "$SERVICE_FILE" << EOF
[Unit]
Description=R-ShareMouse Daemon
After=network.target graph-session.target

[Service]
Type=simple
ExecStart=$DAEMON_BIN
Restart=on-failure
RestartSec=5

# Environment variables
Environment="DISPLAY=$DISPLAY"
Environment="WAYLAND_DISPLAY=$WAYLAND_DISPLAY"

[Install]
WantedBy=default.target
EOF

echo -e "${GREEN}Created service file: $SERVICE_FILE${NC}"

echo ""

# Firewall configuration
echo -e "${YELLOW}Checking firewall configuration...${NC}"
./bin/linux/firewall.sh status

echo ""
read -p "Configure firewall rules now? (Y/n) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Nn]$ ]]; then
    if [ -w /etc ]; then
        sudo ./bin/linux/firewall.sh enable
    else
        echo -e "${YELLOW}Firewall configuration requires root privileges${NC}"
        echo "Run manually: sudo ./bin/linux/firewall.sh enable"
    fi
fi

echo ""
echo -e "${GREEN}================================================${NC}"
echo -e "${GREEN}  Setup Complete!${NC}"
echo -e "${GREEN}================================================${NC}"
echo ""
echo "To enable and start the daemon:"
echo ""
echo "  # Reload systemd"
echo "  systemctl --user daemon-reload"
echo ""
echo "  # Enable auto-start on login"
echo "  systemctl --user enable rshare-daemon.service"
echo ""
echo "  # Start now"
echo "  systemctl --user start rshare-daemon.service"
echo ""
echo "  # Check status"
echo "  systemctl --user status rshare-daemon.service"
echo ""
echo "Or run manually:"
echo "  ./bin/linux/run.sh daemon"
echo ""
echo "Or run the desktop app:"
echo "  ./bin/linux/run.sh desktop"
echo ""
