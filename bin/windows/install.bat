@echo off
REM R-ShareMouse Windows Setup Script
REM
REM This script sets up R-ShareMouse on Windows including:
REM - Building the project
REM - Creating startup shortcut
REM - Setting up firewall rules

setlocal enabledelayedexpansion

echo ================================================
echo   R-ShareMouse Windows Setup
echo ================================================
echo.

REM Check if running on Windows
if not defined OS (
    echo This script is for Windows only.
    exit /b 1
)

echo [Building R-ShareMouse...]
echo.
call bin\windows\build.bat --release desktop

if errorlevel 1 (
    echo.
    echo [ERROR] Build failed!
    exit /b 1
)

echo.
echo [Creating startup shortcut...]

REM Get the absolute path of the project
set PROJECT_DIR=%~dp0
set PROJECT_DIR=%PROJECT_DIR:~0,-1%
set EXE_PATH=%PROJECT_DIR%\target\release\rshare-gui.exe

REM Create startup shortcut using PowerShell
set PS_SCRIPT=%TEMP%\create_shortcut.psil

echo Set-ShellApplication -New ^(
echo     New-Object -ComObject WScript.Shell^).CreateShortcut^(
echo     '%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\R-ShareMouse.lnk'^) ^|
echo     Select-Object -ExpandProperty TargetPath ^| ^(
echo     New-Object -ComObject WScript.Shell^).CreateShortcut^(
echo     '%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\R-ShareMouse.lnk'^)^;
echo $Shortcut.TargetPath = '%EXE_PATH%'
echo $Shortcut.WorkingDirectory = '%PROJECT_DIR%'
echo $Shortcut.Description = 'R-ShareMouse - Share mouse and keyboard across computers'
echo $Shortcut.Save

powershell -NoProfile -Command ^
  "$WshShell = New-Object -ComObject WScript.Shell; " ^
  "$Shortcut = $WshShell.CreateShortcut('%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\R-ShareMouse.lnk'); " ^
  "$Shortcut.TargetPath = '%EXE_PATH%'; " ^
  "$Shortcut.WorkingDirectory = '%PROJECT_DIR%'; " ^
  "$Shortcut.Description = 'R-ShareMouse'; " ^
  "$Shortcut.Save"

if errorlevel 1 (
    echo [WARNING] Could not create startup shortcut
) else (
    echo [SUCCESS] Startup shortcut created
)

echo.
echo [Checking firewall configuration...]
call bin\windows\firewall.bat status

echo.
set /p CONFIG_FW="Configure firewall rules now? (Y/n): "
if /i not "%CONFIG_FW%"=="n" (
    echo.
    echo [Configuring firewall rules...]
    call bin\windows\firewall.bat enable
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
echo   1. Double-click: target\release\rshare-gui.exe
echo   2. Or run: bin\windows\run.bat desktop
echo   3. Or use the Start Menu shortcut
echo.
echo Note: For global input on Windows, run as administrator
echo or configure proper privileges in Windows Security.
echo.

pause
