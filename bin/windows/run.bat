@echo off
REM R-ShareMouse Run Script for Windows
REM
REM Usage:
REM   bin\windows\run.bat              # Run daemon
REM   bin\windows\run.bat daemon       # Run daemon
REM   bin\windows\run.bat cli status   # Run CLI command
REM   bin\windows\run.bat gui          # Run GUI

setlocal EnableExtensions EnableDelayedExpansion

set "SCRIPT_DIR=%~dp0"
for %%I in ("%SCRIPT_DIR%..\..") do set "REPO_ROOT=%%~fI"
pushd "%REPO_ROOT%" >nul || (
    echo [ERROR] Could not enter repository root: "%REPO_ROOT%"
    exit /b 1
)

set "TARGET=daemon"
set "BUILD_MODE=debug"
set "CLI_ARGS="

:parse_args
if "%~1"=="" goto end_parse
if /i "%~1"=="--release" (
    set "BUILD_MODE=release"
    shift
    goto parse_args
)
if /i "%~1"=="daemon" (
    set "TARGET=daemon"
    shift
    goto parse_args
)
if /i "%~1"=="gui" (
    set "TARGET=gui"
    shift
    goto parse_args
)
if /i "%~1"=="desktop" (
    set "TARGET=desktop"
    shift
    goto parse_args
)
if /i "%~1"=="cli" (
    set "TARGET=cli"
    shift
    goto parse_cli_args
)
if /i "%~1"=="-h" goto help
if /i "%~1"=="--help" goto help
echo Unknown option: %~1
popd >nul
exit /b 1

:parse_cli_args
if "%~1"=="" goto end_parse
set "CLI_ARGS=!CLI_ARGS! %~1"
shift
goto parse_cli_args

:end_parse
if /i "%BUILD_MODE%"=="release" (
    set "BIN_DIR=%REPO_ROOT%\target\release"
    set "BUILD_MODE_FLAG=--release"
) else (
    set "BIN_DIR=%REPO_ROOT%\target\debug"
    set "BUILD_MODE_FLAG="
)

if /i "%TARGET%"=="daemon" goto run_daemon
if /i "%TARGET%"=="cli" goto run_cli
if /i "%TARGET%"=="gui" goto run_gui
if /i "%TARGET%"=="desktop" goto run_desktop

echo [ERROR] Unknown target: %TARGET%
popd >nul
exit /b 1

:run_daemon
echo [Starting rshare-daemon...]
if not exist "%BIN_DIR%\rshare-daemon.exe" (
    echo Daemon not found. Building first...
    call "%SCRIPT_DIR%build.bat" %BUILD_MODE_FLAG% daemon
    if errorlevel 1 (
        popd >nul
        exit /b 1
    )
)
"%BIN_DIR%\rshare-daemon.exe"
set "EXIT_CODE=%ERRORLEVEL%"
popd >nul
exit /b %EXIT_CODE%

:run_cli
if not exist "%BIN_DIR%\rshare.exe" (
    echo CLI not found. Building first...
    call "%SCRIPT_DIR%build.bat" %BUILD_MODE_FLAG% cli
    if errorlevel 1 (
        popd >nul
        exit /b 1
    )
)
"%BIN_DIR%\rshare.exe" %CLI_ARGS%
set "EXIT_CODE=%ERRORLEVEL%"
popd >nul
exit /b %EXIT_CODE%

:run_gui
echo [Starting rshare-gui...]
if not exist "%BIN_DIR%\rshare-gui.exe" (
    echo GUI not found. Building first...
    call "%SCRIPT_DIR%build.bat" %BUILD_MODE_FLAG% gui
    if errorlevel 1 (
        popd >nul
        exit /b 1
    )
)
start "" "%BIN_DIR%\rshare-gui.exe"
echo [GUI started]
popd >nul
exit /b 0

:run_desktop
echo [Starting rshare-desktop...]
if not exist "%BIN_DIR%\rshare-gui.exe" (
    echo Desktop app not found. Building first...
    call "%SCRIPT_DIR%build.bat" %BUILD_MODE_FLAG% desktop
    if errorlevel 1 (
        popd >nul
        exit /b 1
    )
)
"%BIN_DIR%\rshare-gui.exe"
set "EXIT_CODE=%ERRORLEVEL%"
call :wait_for_daemon_cleanup
popd >nul
exit /b %EXIT_CODE%

:wait_for_daemon_cleanup
set "DAEMON_EXE=%BIN_DIR%\rshare-daemon.exe"
if not exist "%DAEMON_EXE%" goto :eof

set "DAEMON_RUNNING="
for /f "usebackq delims=" %%I in (`powershell -NoProfile -ExecutionPolicy Bypass -Command "$path = [IO.Path]::GetFullPath('%DAEMON_EXE%'); Get-Process -Name 'rshare-daemon' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and ([IO.Path]::GetFullPath($_.Path) -ieq $path) } | Select-Object -ExpandProperty Id"`) do (
    set "DAEMON_RUNNING=1"
)
if not defined DAEMON_RUNNING goto :eof

echo [Waiting for rshare-daemon to exit...]
for /l %%N in (1,1,15) do (
    powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Sleep -Milliseconds 200" >nul
    set "DAEMON_RUNNING="
    for /f "usebackq delims=" %%I in (`powershell -NoProfile -ExecutionPolicy Bypass -Command "$path = [IO.Path]::GetFullPath('%DAEMON_EXE%'); Get-Process -Name 'rshare-daemon' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and ([IO.Path]::GetFullPath($_.Path) -ieq $path) } | Select-Object -ExpandProperty Id"`) do (
        set "DAEMON_RUNNING=1"
    )
    if not defined DAEMON_RUNNING goto :eof
)

echo [WARN] rshare-daemon is still running after desktop exit, forcing stop...]
powershell -NoProfile -ExecutionPolicy Bypass -Command "$path = [IO.Path]::GetFullPath('%DAEMON_EXE%'); Get-Process -Name 'rshare-daemon' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and ([IO.Path]::GetFullPath($_.Path) -ieq $path) } | Stop-Process -Force" >nul
goto :eof

:help
echo Usage: %~nx0 [OPTIONS] [TARGET] [ARGS...]
echo.
echo Options:
echo   --release    Use release build ^(default: debug^)
echo.
echo Targets:
echo   daemon       Run rshare-daemon ^(default^)
echo   cli          Run rshare CLI with args
echo   gui          Run rshare-gui
echo   desktop      Run rshare-desktop and wait for daemon cleanup
echo.
echo Examples:
echo   %~nx0
echo   %~nx0 cli status
echo   %~nx0 cli devices
echo   %~nx0 gui
echo   %~nx0 --release daemon
popd >nul
exit /b 0
