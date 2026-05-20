# R-ShareMouse Roadmap

Last updated: 2026-05-20

Related documents:

- [Capability-Oriented Endpoint Architecture Design](./plans/2026-05-20-capability-oriented-endpoint-architecture-design.md)
- [Capability Interface Roadmap Implementation Plan](./plans/2026-05-20-capability-interface-roadmap-implementation-plan.md)
- [USB Device Forwarding Feasibility Research Archive](./research/2026-05-20-usb-device-forwarding-deep-research.md)
- [Endpoint Event Injection and Observation Design](./plans/2026-05-16-endpoint-event-injection-and-observation-design.md)
- [HID and Experimental USB Forwarding](./plans/2026-05-02-hid-and-experimental-usb-forwarding.md)

## Current Stage

`R-ShareMouse` is in the **Alpha-2 / P0.5 validation stage**.

The project has moved beyond a UI/control-plane prototype. The daemon owns the runtime model, the desktop app is connected to daemon snapshots, endpoint event observation exists, and the transport direction is now low-latency QUIC rather than a purely TCP-oriented prototype.

The project is not yet a formal daily-use release. The next gate is real dual-machine validation of the primary input loop:

- keyboard, mouse, wheel, and combo-key forwarding
- mouse position observation and layout-driven edge switching
- quick return to the machine that owns the cursor
- automatic edge penetration
- truthful backend health and injection latency reporting
- restart, reconnect, sleep/wake, and daemon/service ownership behavior

Generic USB forwarding remains experimental and is not part of the daily-use exit criteria.

## Product Direction

R-ShareMouse should be a capability-oriented endpoint sharing platform.

The primary product path is:

1. Low-latency keyboard and mouse sharing.
2. Clipboard, layout persistence, reconnect, and diagnostics for office use.
3. Event-level expansion for gamepad, audio, display topology, and endpoint observation.
4. Experimental generic USB forwarding only for devices that truly require real USB identity.

The guiding rule is:

> Human input goes through events, media goes through dedicated channels, and only tool/dongle/board-class devices go through real USB forwarding.

## Stage Definitions

### Alpha-2: Full Input Main Loop

Goal: prove the real cross-machine input loop.

Required:

- Dual-machine discovery and connection over the active device transport.
- Layout-driven edge switching and return-to-local behavior.
- Mouse move, mouse button, wheel, key, combo-key, and quick-return forwarding.
- Mouse position and endpoint input observation visible in the desktop UI.
- Backend health, injection result, and latency reporting that reflects actual behavior.
- Automatic service startup from desktop only when daemon IPC is unavailable.

Exit criteria:

- Windows-to-Windows LAN validation can share keyboard and mouse in normal unlocked desktop use.
- UI shows discovered peers, current layout, backend health, and recent injection latency.
- Failure cases report degraded/unavailable capability instead of false healthy status.

### Alpha-3: Capability Registry And Endpoint Model

Goal: make every device feature report through a unified daemon-owned capability model.

Required:

- Capability registry for `input`, `clipboard`, `gamepad`, `audio`, `display_topology`, `usb_host`, `usb_receiver`, `privileged_helper`, and `diagnostics`.
- Unified local and remote device snapshots consumed by GUI and CLI.
- Endpoint event filters by device, capability, and event kind.
- Injection results with status, latency, backend, and failure reason.
- Protocol compatibility behavior for peers that do not support a newer capability.

Exit criteria:

- The device page can display local and remote capability state from the daemon without UI-specific truth.
- CLI can inspect capability health and explain unavailable/degraded states.
- Adding a new endpoint category does not require a separate UI-private state model.

### Beta-1: Daily Office Trial

Goal: move from successful endpoint tests to practical office use.

Required:

- Persistent layout topology with remembered offline devices and active layouts that only show online nodes.
- Automatic right-side append ordering for newly discovered peers unless the user edits the layout.
- Clipboard sync with loopback suppression and size limits.
- Reconnect after peer restart, daemon restart, sleep/wake, and short network loss.
- Tray/background service semantics that are predictable and testable.
- Logs and diagnostics usable by non-developer operators.

Exit criteria:

- A normal office workflow can run on Windows LAN without frequent manual recovery.
- 24-hour usage does not require restarting the service for normal network transitions.
- UI and CLI expose enough diagnostics to understand common failures.

### Beta-2: Cross-Platform And Non-USB Capabilities

Goal: expand to a broader daily-use candidate without making generic USB the product core.

Required:

- macOS primary-path input sharing and permission guidance.
- Linux primary-path sharing with explicit Wayland/X11 limitations.
- Trust, pairing, certificate, and fingerprint diagnostics.
- Gamepad observation and injection path planning.
- Audio endpoint observation, routing controls, stream state, and latency diagnostics.
- Display topology, DPI, cursor position, and active monitor reporting.

Exit criteria:

- Windows-to-macOS and Windows-to-Linux trials are realistic for technical users.
- Trust and permission failures are visible and actionable.
- Input latency does not regress as audio/gamepad/display capabilities are added.

### Experimental USB Track

Goal: keep generic USB forwarding isolated, explicit, and safe.

Required before production consideration:

- Separate `usb_host` and `usb_receiver` capability states.
- Explicit allowlist and claim/release lifecycle.
- Flow control, cancellation, reset, hotplug, and reconnect behavior.
- Receiver-side virtual USB bus materialization on each supported platform.
- Security review for BadUSB-style risks and untrusted device import.
- Isochronous transfer support only after control, bulk, and interrupt are stable.

Exit criteria:

- USB can be disabled without affecting input sharing.
- USB failures cannot starve input, clipboard, or audio channels.
- UI clearly labels USB as experimental until receiver-side virtual bus support is production-ready.

### Release Candidate

Goal: validate whether R-ShareMouse can be evaluated as a ShareMouse-class replacement.

Required:

- 24-hour and 72-hour soak tests across two-machine and three-machine setups.
- Restart loops, sleep/wake, network jitter, Wi-Fi/wired transitions, and daemon crash recovery.
- Lock screen, UAC, login, user switching, and privileged-helper degradation semantics.
- Installer, auto-start, upgrade, rollback, diagnostics bundle, and compatibility matrix.
- Public IPC and protocol versioning or migration policy.

Exit criteria:

- Daily-use reliability and recovery characteristics are consistent enough for broader release testing.
- Critical failures produce actionable diagnostics.
- Experimental features are clearly separated from release-blocking primary capabilities.

## Immediate Priorities

1. Complete dual-machine Alpha-2 validation for the input main loop.
2. Add capability registry planning and then implementation so UI/CLI consume one daemon-owned truth source.
3. Keep endpoint observation and injection latency visible in the desktop device page.
4. Stabilize tray/background daemon ownership and restart behavior.
5. Keep generic USB forwarding isolated as an experimental track.

## What Not To Prioritize Yet

- Generic USB receiver-side virtual bus work before keyboard/mouse sharing is stable.
- USB audio or camera forwarding through generic USB while dedicated audio paths are available.
- Advanced visual polish that does not expose runtime truth or improve validation.
- Cross-WAN behavior before LAN discovery, trust, and reconnect are solid.

## Definition Of Useful Progress

Useful progress is any change that increases real, measurable cross-machine endpoint behavior while preserving truthful capability reporting.

For the current stage, the highest-value changes are those that reduce latency, improve dual-machine validation, expose backend and transport failures clearly, or make the daemon snapshot a more complete source of truth.
