@echo off
REM R-ShareMouse Windows Setup Script

setlocal EnableExtensions EnableDelayedExpansion

set "SCRIPT_DIR=%~dp0"
for %%I in ("%SCRIPT_DIR%..\..") do set "REPO_ROOT=%%~fI"
pushd "%REPO_ROOT%" >nul || (
    echo [ERROR] Could not enter repository root: "%REPO_ROOT%"
    exit /b 1
)

echo ================================================
echo   R-ShareMouse Windows Setup
echo ================================================
echo.

if not defined OS (
    echo This script is for Windows only.
    popd >nul
    exit /b 1
)

echo [Building R-ShareMouse...]
echo.
call "%SCRIPT_DIR%build.bat" --release desktop
if errorlevel 1 (
    echo.
    echo [ERROR] Build failed
    popd >nul
    exit /b 1
)

echo.
echo [Creating startup shortcut...]

set "EXE_PATH=%REPO_ROOT%\target\release\rshare-gui.exe"
if not exist "%EXE_PATH%" (
    echo [ERROR] Desktop executable not found: "%EXE_PATH%"
    popd >nul
    exit /b 1
)

powershell -NoProfile -Command ^
  "$WshShell = New-Object -ComObject WScript.Shell; " ^
  "$Shortcut = $WshShell.CreateShortcut('%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\R-ShareMouse.lnk'); " ^
  "$Shortcut.TargetPath = '%EXE_PATH%'; " ^
  "$Shortcut.WorkingDirectory = '%REPO_ROOT%'; " ^
  "$Shortcut.Description = 'R-ShareMouse'; " ^
  "$Shortcut.Save()"

if errorlevel 1 (
    echo [WARNING] Could not create startup shortcut
) else (
    echo [SUCCESS] Startup shortcut created
)

echo.
echo [Checking firewall configuration...]
call "%SCRIPT_DIR%firewall.bat" status

echo.
set /p CONFIG_FW="Configure firewall rules now? (Y/n): "
if /i not "%CONFIG_FW%"=="n" (
    echo.
    echo [Configuring firewall rules...]
    call "%SCRIPT_DIR%firewall.bat" enable
    if errorlevel 1 (
        echo [WARNING] Could not add firewall rule (requires admin)
        echo Run as administrator: Right-click Command Prompt ^> Run as administrator
    )
)

echo.
echo ================================================
echo   Setup Complete!
echo ================================================
echo.
echo To run R-ShareMouse:
echo.
echo   1. Double-click: %REPO_ROOT%\target\release\rshare-gui.exe
echo   2. Or run: %SCRIPT_DIR%run.bat desktop
echo   3. Or use the Start Menu shortcut
echo.
echo Note: For global input on Windows, run as administrator
echo or configure proper privileges in Windows Security.
echo.

popd >nul
pause
