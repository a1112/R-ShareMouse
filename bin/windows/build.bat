@echo off
REM R-ShareMouse Build Script for Windows
REM
REM Usage:
REM   bin\build.bat              # Build all (debug)
REM   bin\build.bat --release    # Build all (release)
REM   bin\build.bat daemon       # Build daemon only

setlocal enabledelayedexpansion

REM Parse arguments
set BUILD_MODE=debug
set TARGET=all

:parse_args
if "%~1"=="" goto end_parse
if "%~1"=="--release" (
    set BUILD_MODE=release
    shift
    goto parse_args
)
if "%~1"=="debug" (
    set BUILD_MODE=debug
    shift
    goto parse_args
)
if "%~1"=="daemon" (
    set TARGET=daemon
    shift
    goto parse_args
)
if "%~1"=="cli" (
    set TARGET=cli
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
if "%~1"=="-h" goto help
if "%~1"=="--help" goto help
echo Unknown option: %~1
exit /b 1

:end_parse

REM Set build flags
if "%BUILD_MODE%"=="release" (
    set BUILD_FLAG=--release
    echo [Building in RELEASE mode...]
) else (
    set BUILD_FLAG=
    echo [Building in DEBUG mode...]
)

REM Build function
if "%TARGET%"=="all" (
    echo [Building all binaries...]
    cargo build %BUILD_FLAG% --workspace
) else if "%TARGET%"=="daemon" (
    echo [Building rshare-daemon...]
    cargo build %BUILD_FLAG% -p rshare-daemon
) else if "%TARGET%"=="cli" (
    echo [Building rshare CLI...]
    cargo build %BUILD_FLAG% -p rshare-cli
) else if "%TARGET%"=="gui" (
    echo [Building rshare-gui...]
    cargo build %BUILD_FLAG% -p rshare-gui
) else if "%TARGET%"=="desktop" (
    echo [Building rshare-desktop (Tauri)...]
    cargo build %BUILD_FLAG% -p rshare-desktop
)

echo.
echo [Build completed!]
echo.
echo Binaries location:
if "%BUILD_MODE%"=="release" (
    echo   target\release\rshare.exe        # CLI
    echo   target\release\rshare-daemon.exe # Daemon
    echo   target\release\rshare-gui.exe    # GUI
) else (
    echo   target\debug\rshare.exe        # CLI
    echo   target\debug\rshare-daemon.exe # Daemon
    echo   target\debug\rshare-gui.exe    # GUI
)

exit /b 0

:help
echo Usage: %~nx0 [OPTIONS] [TARGET]
echo.
echo Options:
echo   --release    Build in release mode (default: debug)
echo   debug        Build in debug mode
echo.
echo Targets:
echo   all          Build all binaries (default)
echo   daemon       Build rshare-daemon
echo   cli          Build rshare CLI
echo   gui          Build rshare-gui
echo   desktop      Build rshare-desktop (Tauri)
echo.
echo Examples:
echo   %~nx0                    # Build all in debug mode
echo   %~nx0 --release          # Build all in release mode
echo   %~nx0 --release daemon   # Build daemon in release mode
exit /b 0
