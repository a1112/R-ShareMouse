#!/bin/bash
# R-ShareMouse macOS Firewall Configuration Script
#
# Usage:
#   ./bin/macos/firewall.sh status    # Check firewall status
#   ./bin/macos/firewall.sh enable    # Enable firewall rules
#   ./bin/macos/firewall.sh disable   # Disable firewall rules
#   ./bin/macos/firewall.sh install   # Install and enable

set -e

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m'

# R-ShareMouse ports
DISCOVERY_PORT=27432  # UDP
SERVICE_PORT=27435    # TCP

# Get config port from cargo config if available
CONFIG_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/rshare/config.toml"
if [ -f "$CONFIG_FILE" ]; then
    SERVICE_PORT=$(grep -E '^port\s*=' "$CONFIG_FILE" | head -1 | sed 's/port\s*=\s*//' | tr -d ' "')
fi

APP_NAME="R-ShareMouse"
PF_ANCHOR_FILE="/etc/pf.anchors/${APP_NAME}"
PF_CONF_FILE="/etc/pf.conf"
PF_CONF_BACKUP="/etc/pf.conf.backup"

# Check if running as root
check_root() {
    if [ "$EUID" -ne 0 ]; then
        echo -e "${RED}This command requires root privileges${NC}"
        echo "Please run with sudo:"
        echo "  sudo $0 $*"
        exit 1
    fi
}

# Check pf.conf backup
check_backup() {
    if [ ! -f "$PF_CONF_BACKUP" ]; then
        # Create backup of original pf.conf
        cp "$PF_CONF_FILE" "$PF_CONF_BACKUP" 2>/dev/null || true
    fi
}

# Restore from backup
restore_backup() {
    if [ -f "$PF_CONF_BACKUP" ]; then
        cp "$PF_CONF_BACKUP" "$PF_CONF_FILE"
        echo -e "${GREEN}Restored pf.conf from backup${NC}"
    fi
}

# Show firewall status
show_status() {
    echo -e "${BLUE}================================================${NC}"
    echo -e "${BLUE}  macOS Firewall Status${NC}"
    echo -e "${BLUE}================================================${NC}"
    echo ""

    # Check application firewall
    if /usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate >/dev/null 2>&1; then
        AF_STATUS=$(/usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate 2>/dev/null || echo "unknown")
        if echo "$AF_STATUS" | grep -q "enabled"; then
            echo -e "${GREEN}[✓]${NC} Application Firewall: Enabled"
        else
            echo -e "${YELLOW}[ ]${NC} Application Firewall: Disabled"
        fi
    else
        echo -e "${YELLOW}[?]${NC} Application Firewall: Unknown"
    fi

    # Check pf (packet filter)
    if pfctl -s info >/dev/null 2>&1; then
        PF_ENABLED=$(pfctl -s info 2>/dev/null | grep "Status: Enabled" || echo "")
        if [ -n "$PF_ENABLED" ]; then
            echo -e "${GREEN}[✓]${NC} PF (Packet Filter): Enabled"
        else
            echo -e "${YELLOW}[ ]${NC} PF (Packet Filter): Disabled"
        fi
    else
        echo -e "${YELLOW}[?]${NC} PF (Packet Filter): Unknown"
    fi

    echo ""
    echo -e "${BLUE}Required ports for $APP_NAME:${NC}"
    echo "  $DISCOVERY_PORT/udp  - Device discovery"
    echo "  $SERVICE_PORT/tcp    - Daemon service"
    echo ""

    # Check anchor file
    if [ -f "$PF_ANCHOR_FILE" ]; then
        echo -e "${GREEN}[✓]${NC} PF anchor file exists: $PF_ANCHOR_FILE"
        echo ""
        echo "Current rules:"
        cat "$PF_ANCHOR_FILE"
    else
        echo -e "${RED}[✗]${NC} PF anchor file not found: $PF_ANCHOR_FILE"
    fi

    # Check if anchor is loaded
    if pfctl -s Anchors 2>/dev/null | grep -q "$APP_NAME"; then
        echo -e "${GREEN}[✓]${NC} PF anchor is loaded"
    else
        echo -e "${YELLOW}[ ]${NC} PF anchor is not loaded"
    fi

    echo ""
}

# Enable firewall rules
enable_rules() {
    check_root

    echo -e "${YELLOW}Configuring PF (Packet Filter) rules...${NC}"

    # Create backup
    check_backup

    # Create anchor file
    cat > "$PF_ANCHOR_FILE" << EOF
# R-ShareMouse firewall rules
# Allow device discovery (UDP)
pass in proto udp from any to any port $DISCOVERY_PORT

# Allow daemon service (TCP)
pass in proto tcp from any to any port $SERVICE_PORT

# Allow outgoing traffic
pass out proto udp from any to any port $DISCOVERY_PORT
pass out proto tcp from any to any port $SERVICE_PORT
EOF

    echo -e "${GREEN}Created anchor file: $PF_ANCHOR_FILE${NC}"

    # Check if anchor is already referenced in pf.conf
    if ! grep -q "rshare-mouse" "$PF_CONF_FILE" 2>/dev/null; then
        # Add anchor reference to pf.conf
        echo "" >> "$PF_CONF_FILE"
        echo "# R-ShareMouse anchor" >> "$PF_CONF_FILE"
        echo "anchor \"${APP_NAME}\"" >> "$PF_CONF_FILE"
        echo "load anchor \"${APP_NAME}\" from \"$PF_ANCHOR_FILE\"" >> "$PF_CONF_FILE"
        echo -e "${GREEN}Added anchor reference to pf.conf${NC}"
    fi

    # Load the anchor
    pfctl -e 2>/dev/null || echo "PF already enabled"
    pfctl -f "$PF_CONF_FILE" 2>/dev/null || true
    pfctl -a "${APP_NAME}" -f "$PF_ANCHOR_FILE"

    echo -e "${GREEN}Firewall rules enabled${NC}"
}

# Disable firewall rules
disable_rules() {
    check_root

    echo -e "${YELLOW}Removing PF rules...${NC}"

    # Remove anchor file
    if [ -f "$PF_ANCHOR_FILE" ]; then
        rm -f "$PF_ANCHOR_FILE"
        echo -e "${GREEN}Removed anchor file${NC}"
    fi

    # Remove anchor reference from pf.conf
    if [ -f "$PF_CONF_FILE" ]; then
        # Create temp file without our anchor
        grep -v "rshare-mouse\|R-ShareMouse" "$PF_CONF_FILE" > /tmp/pf.conf.tmp 2>/dev/null || true
        mv /tmp/pf.conf.tmp "$PF_CONF_FILE"
        echo -e "${GREEN}Removed anchor reference from pf.conf${NC}"
    fi

    # Reload pf.conf
    pfctl -f "$PF_CONF_FILE" 2>/dev/null || true

    echo -e "${GREEN}Firewall rules disabled${NC}"
}

# Main
ACTION="${1:-status}"

case $ACTION in
    status|check)
        show_status
        ;;
    enable|add|install)
        enable_rules
        echo ""
        show_status
        ;;
    disable|remove|uninstall)
        disable_rules
        echo ""
        show_status
        ;;
    restore)
        check_root
        restore_backup
        ;;
    -h|--help|help)
        echo "Usage: $0 [COMMAND]"
        echo ""
        echo "Commands:"
        echo "  status, check    Show firewall status and rule configuration"
        echo "  enable, add      Add firewall rules for R-ShareMouse (requires sudo)"
        echo "  disable, remove  Remove firewall rules for R-ShareMouse (requires sudo)"
        echo "  install          Same as enable"
        echo "  uninstall        Same as disable"
        echo "  restore          Restore pf.conf from backup (requires sudo)"
        echo ""
        echo "Ports:"
        echo "  $DISCOVERY_PORT/udp  - Device discovery"
        echo "  $SERVICE_PORT/tcp    - Daemon service"
        echo ""
        echo "Note: macOS requires root privileges to modify firewall rules."
        echo "      Use 'sudo $0 enable' to configure."
        echo ""
        ;;
    *)
        echo -e "${RED}Unknown command: $ACTION${NC}"
        echo "Use --help for usage information"
        exit 1
        ;;
esac
