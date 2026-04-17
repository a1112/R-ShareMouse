@echo off
REM R-ShareMouse Discovery Test for Windows

echo ================================================
echo   R-ShareMouse LAN Discovery Test
echo ================================================
echo.
echo This will scan for R-ShareMouse devices on your LAN.
echo Make sure:
echo   1. Windows Firewall allows UDP port 27432
echo   2. Other devices are running R-ShareMouse
echo.
pause

REM Check if build exists
if not exist "target\release\rshare.exe" (
    echo Building R-ShareMouse...
    cargo build --release --bin rshare
    if errorlevel 1 (
        echo Build failed!
        pause
        exit /b 1
    )
)

echo.
echo Starting discovery scan (30 seconds)...
echo Press Ctrl+C to stop early
echo.

target\release\rshare.exe discover --duration 30

echo.
echo.
pause
