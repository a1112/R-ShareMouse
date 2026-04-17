#!/bin/bash
# R-ShareMouse Discovery Test for macOS/Linux

echo "================================================"
echo "  R-ShareMouse LAN Discovery Test"
echo "================================================"
echo ""
echo "This will scan for R-ShareMouse devices on your LAN."
echo "Make sure:"
echo "  1. Firewall allows UDP port 27432"
echo "  2. Other devices are running R-ShareMouse"
echo ""
read -p "Press Enter to start..."

# Check if build exists
if [ ! -f "target/release/rshare" ]; then
    echo "Building R-ShareMouse..."
    cargo build --release --bin rshare
    if [ $? -ne 0 ]; then
        echo "Build failed!"
        exit 1
    fi
fi

echo ""
echo "Starting discovery scan (30 seconds)..."
echo "Press Ctrl+C to stop early"
echo ""

./target/release/rshare discover --duration 30

echo ""
echo "Done!"
