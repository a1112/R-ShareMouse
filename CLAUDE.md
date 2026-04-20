# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

R-ShareMouse is a cross-platform mouse and keyboard sharing software written in Rust. It allows using one mouse and keyboard across multiple computers with low-latency encrypted communication (QUIC/TLS).

**Current Stage:** Alpha-2 (see `docs/roadmap.md`). The core runtime model is validated via automated tests. Manual dual-machine validation and desktop UI integration are pending.

## Common Commands

### Building

```bash
# Build entire workspace
cargo build --release

# Build specific crate/app
cargo build -p rshare-daemon
cargo build -p rshare-cli
cargo build -p rshare-gui

# Development build (faster)
cargo build
```

### Testing

```bash
# Run all tests
cargo test --workspace

# Test specific crate
cargo test -p rshare-core
cargo test -p rshare-net
cargo test -p rshare-daemon

# Run specific test
cargo test -p rshare-core runtime_contract
cargo test -p rshare-core session_state_machine
```

### Running Applications

```bash
# Start the daemon service
cargo run -p rshare-daemon

# CLI interface
cargo run -p rshare-cli -- status
cargo run -p rshare-cli -- devices
cargo run -p rshare-cli -- discover

# GUI (egui-based)
cargo run -p rshare-gui
```

## Architecture

The project follows a layered architecture with clear separation between core business logic, networking, input handling, and platform-specific code.

### Workspace Structure

```
apps/
  ├── rshare-cli/          # Command-line interface
  ├── rshare-daemon/       # Background daemon service (owns runtime state)
  ├── rshare-desktop/      # Tauri desktop app
  └── rshare-gui/          # egui-based GUI

crates/
  ├── rshare-common/       # Shared types (Direction, ScreenInfo, ButtonState)
  ├── rshare-core/         # Core business logic
  ├── rshare-net/          # Networking layer (QUIC, discovery, transport)
  ├── rshare-input/        # Input handling abstraction
  └── rshare-platform/     # Platform-specific implementations (Windows/macOS/Linux)
```

### Layer Responsibilities

**rshare-common**: Minimal shared types to prevent circular dependencies. Re-exported by other crates.

**rshare-core**: Canonical business logic including:
- Protocol definitions (`Message` enum for device communication)
- Layout graph topology (`LayoutGraph`, `LayoutNode`, `LayoutLink`)
- Session state machine (`CaptureSessionStateMachine`)
- Runtime state models (`PeerDirectoryEntry`, `BackendRuntimeState`, `ControlSessionState`)
- Device management and configuration
- IPC protocol for daemon control

**rshare-net**: QUIC-based networking with device discovery, connection management, and message transport.

**rshare-input**: Abstractions for input capture and injection. Backend selection logic determines which platform-specific implementation to use.

**rshare-platform**: Platform-specific clipboard, file drop, and privilege handling.

**rshare-daemon**: Background service that:
- Owns the canonical `LayoutGraph` and `CaptureSessionStateMachine`
- Routes input based on topology (not connection order)
- Manages peer discovery and connections
- Exposes runtime state via localhost IPC (port 27435)

## Key Architectural Concepts

### Alpha-2 Runtime Model

The daemon owns three core state structures:

1. **LayoutGraph**: Defines device topology and which edges lead to which targets
2. **CaptureSessionStateMachine**: Manages transitions between LocalReady, RemoteActive, and Suspended states
3. **BackendRuntimeState**: Tracks capture/inject health separately for truthful status reporting

### Input Routing Flow

```
Local Input → Edge Detection → LayoutGraph.resolve_target() →
  RemoteActive if connected → Forward via QUIC → Remote Inject
```

The daemon only forwards to targets that are:
1. Linked in the layout graph
2. Currently connected
3. Not degraded due to backend failures

### Backend Selection

Input backends are selected based on platform capability and health:
- `Portable`: User-mode fallback (currently implemented)
- Future: Virtual HID, elevated helper, etc.

Backend health is tracked separately for capture and injection. Both must be healthy for end-to-end input.

### IPC Protocol

Local clients (GUI, CLI) communicate with the daemon via newline-delimited JSON over TCP (localhost:27435). See `rshare-core/src/ipc.rs` for the full protocol.

## Development Guidelines

### State Ownership

- **Daemon is authoritative**: All runtime state lives in the daemon
- **Clients are read-only views**: GUI and CLI consume daemon snapshots
- **No redundant state**: Don't compute truth separately in the UI

### Testing Conventions

- Tests are co-located with modules (`mod.rs` contains `#[cfg(test)]` sections)
- Integration tests go in `tests/` directories
- Contract tests validate public API boundaries (e.g., `runtime_contract.rs`)
- Use TDD for new features: write failing test, implement, verify pass

### Platform Code

Platform-specific code uses `cfg` attributes:
```rust
#[cfg(windows)]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;
```

### Session State Transitions

When working with `CaptureSessionStateMachine`:
- `on_edge_hit()`: Local device edge hit, requires valid target
- `on_return_edge_hit()`: Return from remote to local
- `on_target_disconnect()`: Remote device disconnected
- `on_backend_degraded()`: Backend failure detected
- `reset()`: Recover from suspended state

## Current Focus (Alpha-2)

Per `docs/plans/2026-04-19-alpha-2-full-input-loop-implementation-plan.md`:
- Core runtime model is complete and tested
- Daemon uses layout-driven routing
- Backend health is tracked truthfully
- Pending: Manual dual-machine validation, desktop UI integration

## Relevant Files

- `docs/roadmap.md`: Project stages and exit criteria
- `docs/plans/`: Detailed implementation plans with validation checklists
- `crates/rshare-core/src/runtime.rs`: Runtime state models
- `crates/rshare-core/src/session.rs`: Session state machine
- `crates/rshare-core/src/layout.rs`: Layout graph and routing
- `apps/rshare-daemon/src/main.rs`: Daemon entry point and state management
