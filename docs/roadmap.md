# R-ShareMouse Roadmap

_Last updated: 2026-04-19_

## Current Stage

`R-ShareMouse` is currently in **Alpha-1**.

That means the project already has a usable product shell:
- Tauri desktop window and primary UI theme
- Local daemon IPC and status/control surface
- Device discovery/status display
- GUI/CLI service start, stop, status, connect, and disconnect entry points

It does **not** yet have a stable end-to-end input-sharing core:
- Windows capture backend is still not implemented as a real capture path
- Backend status is now more honest, but the backend abstraction is not yet a production-grade routing layer
- Clipboard, topology editing, reconnect behavior, and privilege-sensitive desktop scenarios are incomplete
- Lock screen, UAC, login, pairing, certificate trust, and advanced transfers are still outside the daily-use path

## Stage Definitions

### Alpha-1

**Goal:** Prove product structure and control plane.

**What exists**
- Desktop shell and navigation
- Background daemon and IPC control path
- Real service/device snapshots exposed to UI
- Basic connection-oriented product workflow

**What is missing**
- Stable cross-machine input capture and forwarding
- Reliable backend health that maps to daily usable behavior
- Real system-level edge cases

**Exit criteria**
- Core input path is no longer mocked, stubbed, or merely represented in status
- Windows-to-Windows keyboard and mouse sharing works in repeated manual validation

### Alpha-2

**Goal:** Close the first real input-sharing loop.

**Required**
- Real Windows capture backend
- Input backend selection tied to real operational capability
- End-to-end path: discover -> connect -> capture -> forward -> inject -> disconnect
- First dual-machine validation pass

**Exit criteria**
- Windows-to-Windows mouse move, click, wheel, and keyboard input work across two machines
- Disconnect and reconnect do not leave the service in a false healthy state
- UI/CLI backend state matches actual input capability

### Beta-1

**Goal:** Move from “works in a demo” to “can be trialed for office use”.

**Required**
- Layout/topology handling becomes stable
- Basic clipboard sync works across active peers
- Reconnect behavior after daemon restart or network interruption
- Useful error messages, logs, and operator-facing recovery actions

**Exit criteria**
- A normal office workflow can be trialed on Windows without manual recovery every few minutes

### Beta-2

**Goal:** Expand from one-platform proof into a broader daily-use candidate.

**Required**
- macOS primary-path support
- Device trust/pairing/certificate flow
- More reliable clipboard semantics and topology editing
- Better install/upgrade/runtime diagnostics

**Exit criteria**
- Cross-platform trial use is realistic for non-developer users

### Release Candidate

**Goal:** Validate whether the project can function as a ShareMouse-class replacement.

**Required**
- Long-run stability validation
- Login/UAC/locked desktop/helper strategy finalized
- Upgrade, rollback, diagnostics, and recovery paths
- Clear compatibility and risk envelope

**Exit criteria**
- Daily-use reliability and recovery characteristics are consistent enough for broader release evaluation

## Immediate Priorities

1. Implement the first real Windows capture backend so the product can move out of “control plane only”.
2. Tie backend health and selection to real capture/inject capability instead of partial adapter availability.
3. Run a dual-machine validation loop and fix the first round of end-to-end failures before adding more product surface area.

## What Not To Prioritize Yet

- Advanced UI polish beyond what is required to expose real backend state
- Virtual HID driver work before the user-mode Windows path is operational
- Lock-screen/UAC/login flows before normal unlocked-desktop sharing is stable
- Complex settings persistence beyond the minimum needed to validate the core workflow

## Definition Of “Useful Progress” Right Now

At this stage, useful progress is any change that increases the amount of **real input-sharing behavior** and decreases the amount of **reported-but-not-actually-available capability**.
