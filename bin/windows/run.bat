@echo off
REM R-ShareMouse Run Script for Windows
REM
REM Usage:
REM   bin\run.bat              # Run daemon
REM   bin\run.bat daemon       # Run daemon
REM   bin\run.bat cli status   # Run CLI command
REM   bin\run.bat gui          # Run GUI

setlocal enabledelayedexpansion

REM Parse arguments
set TARGET=daemon
set BUILD_MODE=debug
set CLI_ARGS=

:parse_args
if "%~1"=="" goto end_parse
if "%~1"=="--release" (
    set BUILD_MODE=release
    shift
    goto parse_args
)
if "%~1"=="daemon" (
    set TARGET=daemon
    shift
    goto parse_args
)
if "%~1"=="gui" (
    set TARGET=gui
    shift
    goto parse_args
)
if "%~1"=="desktop" (
    set TARGET=desktop
    shift
    goto parse_args
)
if "%~1"=="cli" (
    set TARGET=cli
    shift
    goto parse_cli_args
)
if "%~1"=="-h" goto help
if "%~1"=="--help" goto help
echo Unknown option: %~1
exit /b 1

:parse_cli_args
set CLI_ARGS=%CLI_ARGS% %~1
shift
if not "%~1"=="" goto parse_cli_args
goto end_parse

:end_parse

REM Determine binary directory
if "%BUILD_MODE%"=="release" (
    set BIN_DIR=target\release
) else (
    set BIN_DIR=target\debug
)

REM Run target
if "%TARGET%"=="daemon" (
    echo [Starting rshare-daemon...]
    if not exist "%BIN_DIR%\rshare-daemon.exe" (
        echo Daemon not found. Building first...
        call "%~dp0build.bat" %BUILD_MODE_FLAG% daemon
    )
    "%BIN_DIR%\rshare-daemon.exe"
) else if "%TARGET%"=="cli" (
    if not exist "%BIN_DIR%\rshare.exe" (
        echo CLI not found. Building first...
        call "%~dp0build.bat" cli
    )
    "%BIN_DIR%\rshare.exe" %CLI_ARGS%
) else if "%TARGET%"=="gui" (
    echo [Starting rshare-gui...]
    if not exist "%BIN_DIR%\rshare-gui.exe" (
        echo GUI not found. Building first...
        call "%~dp0build.bat" gui
    )
    start "" "%BIN_DIR%\rshare-gui.exe"
    echo [GUI started]
) else if "%TARGET%"=="desktop" (
    echo [Starting rshare-desktop...]
    if not exist "%BIN_DIR%\rshare-gui.exe" (
        echo Desktop app not found. Building first...
        call "%~dp0build.bat" desktop
    )
    start "" "%BIN_DIR%\rshare-gui.exe"
    echo [Desktop app started]
)

exit /b 0

:help
echo Usage: %~nx0 [OPTIONS] [TARGET] [ARGS...]
echo.
echo Options:
echo   --release    Use release build (default: debug)
echo.
echo Targets:
echo   daemon       Run rshare-daemon (default)
echo   cli          Run rshare CLI with args
echo   gui          Run rshare-gui
echo   desktop      Run rshare-desktop (Tauri)
echo.
echo Examples:
echo   %~nx0                      # Run daemon
echo   %~nx0 cli status           # Run 'rshare status'
echo   %~nx0 cli devices          # Run 'rshare devices'
echo   %~nx0 gui                  # Run GUI
echo   %~nx0 --release daemon     # Run release build
exit /b 0
