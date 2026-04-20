#!/bin/bash
# R-ShareMouse Linux Firewall Configuration Script
#
# Usage:
#   ./bin/linux/firewall.sh status    # Check firewall status
#   ./bin/linux/firewall.sh enable    # Enable firewall rules
#   ./bin/linux/firewall.sh disable   # Disable firewall rules
#   ./bin/linux/firewall.sh install   # Install and enable

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

# Detect firewall backend
detect_firewall() {
    if command -v ufw >/dev/null 2>&1; then
        echo "ufw"
    elif command -v firewall-cmd >/dev/null 2>&1; then
        echo "firewalld"
    elif command -v iptables >/dev/null 2>&1; then
        echo "iptables"
    else
        echo "none"
    fi
}

# Check if rules exist
check_rules_ufw() {
    ufw status | grep -q "$DISCOVERY_PORT.*udp" && echo "discovery" || echo ""
    ufw status | grep -q "$SERVICE_PORT.*tcp" && echo "service" || echo ""
}

check_rules_firewalld() {
    firewall-cmd --list-ports 2>/dev/null | grep -q "${DISCOVERY_PORT}/udp" && echo "discovery" || echo ""
    firewall-cmd --list-ports 2>/dev/null | grep -q "${SERVICE_PORT}/tcp" && echo "service" || echo ""
}

check_rules_iptables() {
    iptables -L INPUT -n | grep -q "dpt:$DISCOVERY_PORT" && echo "discovery" || echo ""
    iptables -L INPUT -n | grep -q "dpt:$SERVICE_PORT" && echo "service" || echo ""
}

# Add rules
add_rules_ufw() {
    echo -e "${YELLOW}Configuring UFW rules...${NC}"

    # Allow discovery port (UDP)
    ufw allow ${DISCOVERY_PORT}/udp comment "${APP_NAME} Discovery" 2>/dev/null || true

    # Allow service port (TCP)
    ufw allow ${SERVICE_PORT}/tcp comment "${APP_NAME} Service" 2>/dev/null || true

    echo -e "${GREEN}UFW rules added${NC}"
}

add_rules_firewalld() {
    echo -e "${YELLOW}Configuring firewalld rules...${NC}"

    # Allow discovery port (UDP)
    firewall-cmd --permanent --add-port=${DISCOVERY_PORT}/udp 2>/dev/null || true

    # Allow service port (TCP)
    firewall-cmd --permanent --add-port=${SERVICE_PORT}/tcp 2>/dev/null || true

    # Reload to apply
    firewall-cmd --reload 2>/dev/null || true

    echo -e "${GREEN}firewalld rules added${NC}"
}

add_rules_iptables() {
    echo -e "${YELLOW}Configuring iptables rules...${NC}"

    # Allow discovery port (UDP)
    iptables -A INPUT -p udp --dport ${DISCOVERY_PORT} -j ACCEPT 2>/dev/null || true

    # Allow service port (TCP)
    iptables -A INPUT -p tcp --dport ${SERVICE_PORT} -j ACCEPT 2>/dev/null || true

    # Save rules (varies by distribution)
    if command -v iptables-save >/dev/null 2>&1; then
        if [ -w /etc/iptables/rules.v4 ]; then
            iptables-save > /etc/iptables/rules.v4 2>/dev/null || true
        elif [ -w /etc/sysconfig/iptables ]; then
            iptables-save > /etc/sysconfig/iptables 2>/dev/null || true
        fi
    fi

    echo -e "${GREEN}iptables rules added${NC}"
    echo -e "${YELLOW}Note: Rules may not persist after reboot. Consider using ufw or firewalld.${NC}"
}

# Remove rules
remove_rules_ufw() {
    echo -e "${YELLOW}Removing UFW rules...${NC}"

    ufw delete allow ${DISCOVERY_PORT}/udp 2>/dev/null || true
    ufw delete allow ${SERVICE_PORT}/tcp 2>/dev/null || true

    echo -e "${GREEN}UFW rules removed${NC}"
}

remove_rules_firewalld() {
    echo -e "${YELLOW}Removing firewalld rules...${NC}"

    firewall-cmd --permanent --remove-port=${DISCOVERY_PORT}/udp 2>/dev/null || true
    firewall-cmd --permanent --remove-port=${SERVICE_PORT}/tcp 2>/dev/null || true
    firewall-cmd --reload 2>/dev/null || true

    echo -e "${GREEN}firewalld rules removed${NC}"
}

remove_rules_iptables() {
    echo -e "${YELLOW}Removing iptables rules...${NC}"

    iptables -D INPUT -p udp --dport ${DISCOVERY_PORT} -j ACCEPT 2>/dev/null || true
    iptables -D INPUT -p tcp --dport ${SERVICE_PORT} -j ACCEPT 2>/dev/null || true

    echo -e "${GREEN}iptables rules removed${NC}"
}

# Show status
show_status() {
    FIREWALL=$(detect_firewall)

    echo -e "${BLUE}================================================${NC}"
    echo -e "${BLUE}  Firewall Status${NC}"
    echo -e "${BLUE}================================================${NC}"
    echo ""

    if [ "$FIREWALL" = "none" ]; then
        echo -e "${YELLOW}No firewall detected${NC}"
    else
        echo -e "${GREEN}Firewall backend: $FIREWALL${NC}"
        echo ""

        case $FIREWALL in
            ufw)
                if command -v ufw >/dev/null 2>&1; then
                    # Redirect stderr to avoid permission errors
                    ufw status 2>/dev/null | head -10 || echo -e "${YELLOW}Requires root for full status${NC}"
                fi
                ;;
            firewalld)
                if command -v firewall-cmd >/dev/null 2>&1; then
                    echo "Active zones:"
                    firewall-cmd --get-active-zones 2>/dev/null || echo -e "${YELLOW}Requires root for status${NC}"
                    echo ""
                    echo "Open ports:"
                    firewall-cmd --list-ports 2>/dev/null || true
                fi
                ;;
            iptables)
                if command -v iptables >/dev/null 2>&1; then
                    echo "INPUT chain rules:"
                    iptables -L INPUT -n 2>/dev/null | grep -E "(${DISCOVERY_PORT}|${SERVICE_PORT})" || echo -e "${YELLOW}Requires root for status${NC}"
                fi
                ;;
        esac
    fi

    echo ""
    echo -e "${BLUE}Required ports for $APP_NAME:${NC}"
    echo "  $DISCOVERY_PORT/udp  - Device discovery"
    echo "  ${SERVICE_PORT}/tcp    - Daemon service"
    echo ""

    # Check if rules exist (may require root)
    if [ "$FIREWALL" != "none" ]; then
        STATUS_DISCOVERY=""
        STATUS_SERVICE=""
        case $FIREWALL in
            ufw) STATUS_DISCOVERY=$(check_rules_ufw 2>/dev/null | grep discovery); STATUS_SERVICE=$(check_rules_ufw 2>/dev/null | grep service) ;;
            firewalld) STATUS_DISCOVERY=$(check_rules_firewalld 2>/dev/null | grep discovery); STATUS_SERVICE=$(check_rules_firewalld 2>/dev/null | grep service) ;;
            iptables) STATUS_DISCOVERY=$(check_rules_iptables 2>/dev/null | grep discovery); STATUS_SERVICE=$(check_rules_iptables 2>/dev/null | grep service) ;;
        esac

        echo -e "${BLUE}R-ShareMouse rules:${NC}"
        if [ -n "$STATUS_DISCOVERY" ]; then
            echo -e "  ${GREEN}[✓]${NC} Port $DISCOVERY_PORT/udp (discovery)"
        else
            echo -e "  ${RED}[✗]${NC} Port $DISCOVERY_PORT/udp (discovery)"
        fi

        if [ -n "$STATUS_SERVICE" ]; then
            echo -e "  ${GREEN}[✓]${NC} Port $SERVICE_PORT/tcp (service)"
        else
            echo -e "  ${RED}[✗]${NC} Port $SERVICE_PORT/tcp (service)"
        fi
    fi

    echo ""
}

# Main
ACTION="${1:-status}"

case $ACTION in
    status|check)
        show_status
        exit 0
        ;;
    enable|add|install)
        set -e
        FIREWALL=$(detect_firewall)
        if [ "$FIREWALL" = "none" ]; then
            echo -e "${RED}No firewall backend detected${NC}"
            echo "Install ufw, firewalld, or configure iptables manually."
            exit 1
        fi
        add_rules_${FIREWALL}
        echo ""
        show_status
        ;;
    disable|remove|uninstall)
        set -e
        FIREWALL=$(detect_firewall)
        if [ "$FIREWALL" = "none" ]; then
            echo -e "${YELLOW}No firewall backend detected${NC}"
            exit 0
        fi
        remove_rules_${FIREWALL}
        echo ""
        show_status
        ;;
    -h|--help|help)
        echo "Usage: $0 [COMMAND]"
        echo ""
        echo "Commands:"
        echo "  status, check    Show firewall status and rule configuration"
        echo "  enable, add      Add firewall rules for R-ShareMouse"
        echo "  disable, remove  Remove firewall rules for R-ShareMouse"
        echo "  install          Same as enable"
        echo "  uninstall        Same as disable"
        echo ""
        echo "Ports:"
        echo "  $DISCOVERY_PORT/udp  - Device discovery"
        echo "  $SERVICE_PORT/tcp    - Daemon service"
        echo ""
        ;;
    *)
        echo -e "${RED}Unknown command: $ACTION${NC}"
        echo "Use --help for usage information"
        exit 1
        ;;
esac
