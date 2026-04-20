@echo off
REM R-ShareMouse Windows Firewall Configuration Script
REM
REM Usage:
REM   bin\windows\firewall.bat status    # Check firewall status
REM   bin\windows\firewall.bat enable    # Enable firewall rules
REM   bin\windows\firewall.bat disable   # Disable firewall rules
REM   bin\windows\firewall.bat install   # Install and enable

setlocal enabledelayedexpansion

set DISCOVERY_PORT=27432
set SERVICE_PORT=27435
set APP_NAME=R-ShareMouse
set RULE_NAME_DISCOVERY=%APP_NAME% - Discovery
set RULE_NAME_SERVICE=%APP_NAME% - Service

set ACTION=%~1
if "%ACTION%"=="" set ACTION=status

if /i "%ACTION%"=="status" goto status
if /i "%ACTION%"=="check" goto status
if /i "%ACTION%"=="enable" goto enable
if /i "%ACTION%"=="add" goto enable
if /i "%ACTION%"=="install" goto enable
if /i "%ACTION%"=="disable" goto disable
if /i "%ACTION%"=="remove" goto disable
if /i "%ACTION%"=="uninstall" goto disable
if /i "%ACTION%"=="-h" goto help
if /i "%ACTION%"=="--help" goto help
if /i "%ACTION%"=="help" goto help
echo Unknown command: %ACTION%
echo Use --help for usage information
exit /b 1

:status
echo ================================================
echo   Windows Firewall Status
echo ================================================
echo.

REM Check if firewall is enabled
netsh advfirewall show allprofiles state | findstr "State" >nul
if errorlevel 1 (
    echo [!] Unable to determine firewall status
) else (
    netsh advfirewall show allprofiles state | findstr "State"
)

echo.
echo Required ports for %APP_NAME%:
echo   %DISCOVERY_PORT%/udp  - Device discovery
echo   %SERVICE_PORT%/tcp    - Daemon service
echo.

REM Check discovery rule
netsh advfirewall firewall show rule name="%RULE_NAME_DISCOVERY%" >nul 2>&1
if errorlevel 1 (
    echo [X] Port %DISCOVERY_PORT%/udp - Rule not found
) else (
    echo [OK] Port %DISCOVERY_PORT%/udp - Rule exists
)

REM Check service rule
netsh advfirewall firewall show rule name="%RULE_NAME_SERVICE%" >nul 2>&1
if errorlevel 1 (
    echo [X] Port %SERVICE_PORT%/tcp - Rule not found
) else (
    echo [OK] Port %SERVICE_PORT%/tcp - Rule exists
)

echo.
exit /b 0

:enable
echo [Configuring Windows Firewall rules...]
echo.

REM Get the executable path
set EXE_PATH=%~dp0\..\..\target\release\rshare-gui.exe
if not exist "%EXE_PATH%" set EXE_PATH=%~dp0\..\..\target\debug\rshare-gui.exe

REM Check if exe exists
if exist "%EXE_PATH%" goto found_exe
echo [WARNING] Executable not found, using generic rules
set EXE_PATH=any

:found_exe
echo Executable: %EXE_PATH%
echo.

REM Add discovery rule (UDP)
echo [Adding rule for port %DISCOVERY_PORT%/udp...]
netsh advfirewall firewall add rule name="%RULE_NAME_DISCOVERY%" dir=in action=allow protocol=UDP localport=%DISCOVERY_PORT% program="%EXE_PATH%" enable=yes profile=any >nul 2>&1
if errorlevel 1 (
    echo [ERROR] Failed to add discovery rule
    echo You may need to run as administrator
    exit /b 1
) else (
    echo [OK] Added discovery rule
)

REM Add service rule (TCP)
echo [Adding rule for port %SERVICE_PORT%/tcp...]
netsh advfirewall firewall add rule name="%RULE_NAME_SERVICE%" dir=in action=allow protocol=TCP localport=%SERVICE_PORT% program="%EXE_PATH%" enable=yes profile=any >nul 2>&1
if errorlevel 1 (
    echo [ERROR] Failed to add service rule
    echo You may need to run as administrator
    exit /b 1
) else (
    echo [OK] Added service rule
)

echo.
echo [Firewall rules enabled]
echo.
goto show_rules

:show_rules
echo Current rules:
echo.
netsh advfirewall firewall show rule name="%RULE_NAME_DISCOVERY%" | findstr "Rule Name Dir Action"
netsh advfirewall firewall show rule name="%RULE_NAME_SERVICE%" | findstr "Rule Name Dir Action"
echo.
exit /b 0

:disable
echo [Removing Windows Firewall rules...]
echo.

REM Remove discovery rule
netsh advfirewall firewall show rule name="%RULE_NAME_DISCOVERY%" >nul 2>&1
if not errorlevel 1 (
    netsh advfirewall firewall delete rule name="%RULE_NAME_DISCOVERY%" >nul 2>&1
    echo [OK] Removed discovery rule
) else (
    echo [INFO] Discovery rule not found
)

REM Remove service rule
netsh advfirewall firewall show rule name="%RULE_NAME_SERVICE%" >nul 2>&1
if not errorlevel 1 (
    netsh advfirewall firewall delete rule name="%RULE_NAME_SERVICE%" >nul 2>&1
    echo [OK] Removed service rule
) else (
    echo [INFO] Service rule not found
)

echo.
echo [Firewall rules disabled]
echo.
exit /b 0

:help
echo Usage: %~nx0 [COMMAND]
echo.
echo Commands:
echo   status, check    Show firewall status and rule configuration
echo   enable, add      Add firewall rules for R-ShareMouse
echo   disable, remove  Remove firewall rules for R-ShareMouse
echo   install          Same as enable
echo   uninstall        Same as disable
echo.
echo Ports:
echo   %DISCOVERY_PORT%/udp  - Device discovery
echo   %SERVICE_PORT%/tcp    - Daemon service
echo.
echo Note: May require administrator privileges.
echo       Right-click Command Prompt and select "Run as administrator"
echo.
exit /b 0
