@echo off
REM R-ShareMouse GUI Launcher for Windows

echo Starting R-ShareMouse GUI...

REM Check if build exists
if not exist "target\release\rshare-gui.exe" (
    echo Building R-ShareMouse GUI...
    cargo build --release --bin rshare-gui
    if errorlevel 1 (
        echo Build failed!
        pause
        exit /b 1
    )
)

start "" "target\release\rshare-gui.exe"
