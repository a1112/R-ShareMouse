# R-ShareMouse Build and Run Scripts

This directory contains cross-platform scripts for building and running R-ShareMouse, organized by operating system.

## Directory Structure

```
bin/
├── build              # Universal build script (Unix)
├── run                # Universal run script (Unix)
├── build.cmd          # Universal build script (Windows)
├── run.cmd            # Universal run script (Windows)
├── linux/             # Linux-specific scripts
│   ├── build.sh
│   ├── run.sh
│   ├── install.sh
│   └── firewall.sh    # Firewall configuration
├── macos/             # macOS-specific scripts
│   ├── build.sh
│   ├── run.sh
│   ├── install.sh
│   └── firewall.sh    # Firewall configuration
└── windows/           # Windows-specific scripts
    ├── build.bat
    ├── run.bat
    ├── install.bat
    └── firewall.bat   # Firewall configuration
```

## Quick Start

### Linux / macOS

```bash
# Build all components
./bin/build

# Build in release mode
./bin/build --release

# Run daemon
./bin/run daemon

# Run CLI commands
./bin/run cli status
./bin/run cli devices

# Run desktop app
./bin/run desktop
```

### Windows

```cmd
REM Build all components
bin\build.cmd

REM Build in release mode
bin\build.cmd --release

REM Run daemon
bin\run.bat daemon

REM Run CLI commands
bin\run.bat cli status
bin\run.bat cli devices

REM Run desktop app
bin\run.bat desktop
```

## Firewall Configuration

Each platform includes a firewall configuration script that:
- Detects the current firewall status
- Shows required ports for R-ShareMouse
- Adds/removes firewall rules

### Linux

```bash
# Check firewall status
./bin/linux/firewall.sh status

# Enable firewall rules (requires sudo)
sudo ./bin/linux/firewall.sh enable

# Disable firewall rules (requires sudo)
sudo ./bin/linux/firewall.sh disable
```

Supported backends: `ufw`, `firewalld`, `iptables`

### macOS

```bash
# Check firewall status
./bin/macos/firewall.sh status

# Enable firewall rules (requires sudo)
sudo ./bin/macos/firewall.sh enable

# Disable firewall rules (requires sudo)
sudo ./bin/macos/firewall.sh disable
```

Uses `pf` (Packet Filter) for firewall rules.

### Windows

```cmd
REM Check firewall status
bin\windows\firewall.bat status

REM Enable firewall rules (may require admin)
bin\windows\firewall.bat enable

REM Disable firewall rules (may require admin)
bin\windows\firewall.bat disable
```

Uses Windows Firewall with `netsh advfirewall`.

### Required Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| 27432 | UDP | Device discovery (mDNS) |
| 27435 | TCP | Daemon service (IPC) |

*Note: The service port may be different if configured in `~/.config/rshare/config.toml`*

## Platform-Specific Features

### Linux

The Linux scripts include:
- X11/Wayland display server detection
- Systemd service setup (via `install.sh`)
- XTest input emulation support
- Firewall configuration (ufw/firewalld/iptables)

Install Linux daemon as a systemd service:
```bash
./bin/linux/install.sh
```

### macOS

The macOS scripts include:
- `.app` bundle creation with `--app` flag
- LaunchAgent setup for auto-start (via `install.sh`)
- Accessibility permissions guidance
- Firewall configuration (pf)

Build and create macOS app bundle:
```bash
./bin/macos/build.sh --app
```

Run the app bundle:
```bash
./bin/macos/run.sh app
```

Install macOS (creates LaunchAgent):
```bash
./bin/macos/install.sh
```

### Windows

The Windows scripts include:
- Startup shortcut creation (via `install.bat`)
- Firewall rule configuration
- Administrative privilege handling

Install Windows (creates startup shortcut):
```cmd
bin\windows\install.bat
```

## Build Options

| Option | Description |
|--------|-------------|
| `--release` | Build in release mode (optimized) |
| `debug` | Build in debug mode (default) |
| `daemon` | Build/run rshare-daemon only |
| `cli` | Build/run rshare CLI only |
| `gui` | Build/run rshare-gui (egui) only |
| `desktop` | Build/run rshare-desktop (Tauri) only |
| `--app` | Build .app bundle (macOS only) |

## Examples

### Development Workflow

```bash
# Quick debug build and run
./bin/build && ./bin/run daemon

# Test changes to CLI
./bin/build cli && ./bin/run cli status

# Test desktop app changes
./bin/build desktop && ./bin/run desktop
```

### Production Build

```bash
# Full release build
./bin/build --release

# Or per platform
./bin/linux/build.sh --release
./bin/macos/build.sh --release --app
bin\windows\build.bat --release
```

### Auto-Start Setup

```bash
# Linux - systemd service
./bin/linux/install.sh
systemctl --user enable rshare-daemon.service
systemctl --user start rshare-daemon.service

# macOS - LaunchAgent
./bin/macos/install.sh
launchctl load ~/Library/LaunchAgents/com.rshare.mouse.plist

# Windows - Startup shortcut
bin\windows\install.bat
```

## Components

| Component | Binary | Description |
|-----------|--------|-------------|
| `daemon` | `rshare-daemon` | Background service that manages input routing |
| `cli` | `rshare` | Command-line interface for device management |
| `gui` | `rshare-gui` | egui-based GUI application |
| `desktop` | `rshare-gui` | Tauri-based desktop application with system tray |

## Troubleshooting

### Firewall Issues

If devices can't discover each other:

1. Check firewall status:
   ```bash
   ./bin/linux/firewall.sh status    # Linux
   ./bin/macos/firewall.sh status    # macOS
   bin\windows\firewall.bat status   # Windows
   ```

2. Enable firewall rules if needed:
   ```bash
   sudo ./bin/linux/firewall.sh enable    # Linux
   sudo ./bin/macos/firewall.sh enable    # macOS
   bin\windows\firewall.bat enable        # Windows (as admin)
   ```

### Permission Issues

- **Linux**: May need to add user to `input` group for global input
- **macOS**: Grant Accessibility permissions in System Settings
- **Windows**: Run as administrator for global input access
