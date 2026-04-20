@echo off
REM R-ShareMouse Build Script for Windows
REM
REM Usage:
REM   bin\windows\build.bat              # Build all (debug)
REM   bin\windows\build.bat --release    # Build all (release)
REM   bin\windows\build.bat daemon       # Build daemon only

setlocal EnableExtensions EnableDelayedExpansion

set "SCRIPT_DIR=%~dp0"
for %%I in ("%SCRIPT_DIR%..\..") do set "REPO_ROOT=%%~fI"
pushd "%REPO_ROOT%" >nul || (
    echo [ERROR] Could not enter repository root: "%REPO_ROOT%"
    exit /b 1
)

set "BUILD_MODE=debug"
set "TARGET=all"

:parse_args
if "%~1"=="" goto end_parse
if /i "%~1"=="--release" (
    set "BUILD_MODE=release"
    shift
    goto parse_args
)
if /i "%~1"=="debug" (
    set "BUILD_MODE=debug"
    shift
    goto parse_args
)
if /i "%~1"=="daemon" (
    set "TARGET=daemon"
    shift
    goto parse_args
)
if /i "%~1"=="cli" (
    set "TARGET=cli"
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
if /i "%~1"=="-h" goto help
if /i "%~1"=="--help" goto help
echo Unknown option: %~1
popd >nul
exit /b 1

:end_parse
if /i "%BUILD_MODE%"=="release" (
    set "BUILD_FLAG=--release"
    echo [Building in RELEASE mode...]
) else (
    set "BUILD_FLAG="
    echo [Building in DEBUG mode...]
)

call :stop_runtime_before_build

if /i "%TARGET%"=="all" goto build_all
if /i "%TARGET%"=="daemon" goto build_daemon
if /i "%TARGET%"=="cli" goto build_cli
if /i "%TARGET%"=="gui" goto build_gui
if /i "%TARGET%"=="desktop" goto build_desktop

echo [ERROR] Unknown target: %TARGET%
popd >nul
exit /b 1

:build_all
echo [Building all binaries...]
cargo build %BUILD_FLAG% --workspace
goto after_build

:build_daemon
echo [Building rshare-daemon...]
cargo build %BUILD_FLAG% -p rshare-daemon
goto after_build

:build_cli
echo [Building rshare CLI...]
cargo build %BUILD_FLAG% -p rshare-cli
goto after_build

:build_gui
echo [Building rshare-gui...]
cargo build %BUILD_FLAG% -p rshare-gui
goto after_build

:build_desktop
echo [Building rshare-desktop (Tauri)...]
cargo build %BUILD_FLAG% -p rshare-desktop
goto after_build

:after_build
if errorlevel 1 (
    echo.
    echo [ERROR] Build failed
    popd >nul
    exit /b 1
)

echo.
echo [Build completed!]
echo.
echo Binaries location:
if /i "%BUILD_MODE%"=="release" (
    echo   %REPO_ROOT%\target\release\rshare.exe
    echo   %REPO_ROOT%\target\release\rshare-daemon.exe
    echo   %REPO_ROOT%\target\release\rshare-gui.exe
) else (
    echo   %REPO_ROOT%\target\debug\rshare.exe
    echo   %REPO_ROOT%\target\debug\rshare-daemon.exe
    echo   %REPO_ROOT%\target\debug\rshare-gui.exe
)

popd >nul
exit /b 0

:stop_runtime_before_build
set "DESKTOP_EXE=%REPO_ROOT%\target\debug\rshare-gui.exe"
set "DAEMON_EXE=%REPO_ROOT%\target\debug\rshare-daemon.exe"
if /i "%BUILD_MODE%"=="release" (
    set "DESKTOP_EXE=%REPO_ROOT%\target\release\rshare-gui.exe"
    set "DAEMON_EXE=%REPO_ROOT%\target\release\rshare-daemon.exe"
)

set "RUNTIME_STOPPED="
for /f "usebackq delims=" %%I in (`powershell -NoProfile -ExecutionPolicy Bypass -Command "$path = [IO.Path]::GetFullPath('%DESKTOP_EXE%'); Get-Process -Name 'rshare-gui' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and ([IO.Path]::GetFullPath($_.Path) -ieq $path) } | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction Stop; $_.Id }"`) do (
    echo [Stopping running rshare-desktop ^(PID %%I^) from %DESKTOP_EXE%...]
    set "RUNTIME_STOPPED=1"
)

for /f "usebackq delims=" %%I in (`powershell -NoProfile -ExecutionPolicy Bypass -Command "$path = [IO.Path]::GetFullPath('%DAEMON_EXE%'); Get-Process -Name 'rshare-daemon' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and ([IO.Path]::GetFullPath($_.Path) -ieq $path) } | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction Stop; $_.Id }"`) do (
    echo [Stopping running rshare-daemon ^(PID %%I^) from %DAEMON_EXE%...]
    set "RUNTIME_STOPPED=1"
)

if defined RUNTIME_STOPPED (
    powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Sleep -Milliseconds 500" >nul
)
goto :eof

:help
echo Usage: %~nx0 [OPTIONS] [TARGET]
echo.
echo Options:
echo   --release    Build in release mode ^(default: debug^)
echo   debug        Build in debug mode
echo.
echo Targets:
echo   all          Build all binaries ^(default^)
echo   daemon       Build rshare-daemon
echo   cli          Build rshare CLI
echo   gui          Build rshare-gui
echo   desktop      Build rshare-desktop ^(Tauri^)
echo.
echo Examples:
echo   %~nx0
echo   %~nx0 --release
echo   %~nx0 --release daemon
popd >nul
exit /b 0
