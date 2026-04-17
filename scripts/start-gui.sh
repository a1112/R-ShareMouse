#!/bin/bash
# R-ShareMouse GUI Launcher for macOS/Linux

echo "Starting R-ShareMouse GUI..."

# Check if build exists
if [ ! -f "target/release/rshare-gui" ]; then
    echo "Building R-ShareMouse GUI..."
    cargo build --release --bin rshare-gui
    if [ $? -ne 0 ]; then
        echo "Build failed!"
        exit 1
    fi
fi

./target/release/rshare-gui &
