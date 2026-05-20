# Capability Interface Roadmap Implementation Plan

Date: 2026-05-20

Status: executable planning document

Related design: [Capability-Oriented Endpoint Architecture Design](./2026-05-20-capability-oriented-endpoint-architecture-design.md)

## Summary

This plan turns the capability-oriented architecture into staged implementation work. It does not require immediate Rust code changes. It defines the order in which future changes should add public runtime interfaces, daemon-owned state, UI consumption, and tests.

## P0.5: Stabilize Current Input And Endpoint Foundations

Goal: make the current Alpha-2 input loop and endpoint observation usable for dual-machine testing.

Required work:

- Keep keyboard, mouse, wheel, edge switching, and quick-return hotkeys on the primary input path.
- Ensure local and remote endpoint events are observable from the daemon snapshot and device page.
- Report injection latency, transport type, backend health, and failure reason in the UI.
- Validate combo-key forwarding, mouse position reporting, and automatic edge penetration on two LAN machines.
- Keep local IPC unchanged unless a field is required for truthful diagnostics.

Exit criteria:

- Dual-machine test shows live input observation and remote injection results.
- UI shows current input backend health and injection latency without requiring log inspection.
- A failed capture or injection backend is reported as degraded, not silently treated as healthy.

## Alpha-3: Add Capability Registry

Goal: introduce a daemon-owned capability inventory without changing every UI page independently.

Required work:

- Add a capability snapshot model covering `input`, `clipboard`, `gamepad`, `audio`, `display_topology`, `usb_host`, `usb_receiver`, `privileged_helper`, and `diagnostics`.
- Include capability state, version, permission state, health reason, and last event time.
- Map existing runtime state into the new snapshot instead of duplicating UI-specific state.
- Make GUI and CLI consume the same capability snapshot for device status.
- Keep old fields available during migration if existing UI depends on them.

Exit criteria:

- Local and discovered devices show capability availability from one daemon source.
- UI no longer invents per-device capability state from local-only assumptions.
- CLI can print capability status for a selected device.

## Alpha-3.1: Normalize Endpoint Events

Goal: make observation and injection extensible across device categories.

Required work:

- Normalize event kinds for keyboard, mouse, wheel, gamepad, display, audio, and USB observation.
- Add a consistent filter model for local device, remote device, capability kind, and event kind.
- Preserve recent event history with bounded memory and rate limits.
- Return injection result records with status, latency, backend, and failure reason.
- Add protocol compatibility handling for peers that do not support a newer event kind.

Exit criteria:

- Device page can switch between local and remote event streams.
- Keyboard and mouse events update per-device monitors without UI-only simulation.
- Unsupported event kinds degrade clearly instead of breaking the stream.

## Beta-1: Productize Daily Input Sharing

Goal: make the main keyboard/mouse sharing path reliable enough for daily office validation.

Required work:

- Persist layout topology and keep remote offline nodes remembered but hidden from active layout.
- Preserve right-side append ordering for newly discovered peers unless the user changes layout.
- Keep tray, background daemon ownership, start/stop semantics, and close-to-tray behavior stable.
- Add reconnect behavior for daemon restart, peer restart, sleep/wake, and short network loss.
- Improve logs and diagnostics so a failed dual-machine run has actionable output.

Exit criteria:

- A user can start `rshare-desktop`, discover peers, and use the layout without rebuilding topology after every restart.
- Closing the desktop window does not kill the daemon unless explicitly requested.
- 24-hour LAN usage does not require manual service restarts for normal network transitions.

## Beta-2: Expand Non-USB Capabilities

Goal: add useful endpoint capabilities without taking on generic USB virtual bus risk.

Required work:

- Add clipboard policy, deduplication, loopback suppression, and size limits.
- Add gamepad observation and eventual injection as event-level state.
- Add audio endpoint observation, routing controls, capture/playback session state, and latency diagnostics.
- Add display topology details, cursor position, DPI, and active monitor reporting.
- Add trust, pairing, certificate state, and permission prompts to capability diagnostics.

Exit criteria:

- Audio/gamepad/display capability state is visible and truthful even before full forwarding is complete.
- Trust and permission failures are visible from UI and CLI.
- Non-USB capability work does not regress input latency.

## Experimental USB Track

Goal: keep generic USB forwarding available for research without blocking the product path.

Required work:

- Keep `usb_host` and `usb_receiver` capability states separate.
- Require explicit device allowlist and claim/release state before export.
- Keep USB transfer flow control and cancellation independent from input forwarding.
- Implement receiver-side virtual USB materialization as a separate driver milestone.
- Add isochronous support only after control, bulk, and interrupt transfer paths are stable.

Exit criteria:

- USB capability can be disabled without affecting keyboard/mouse sharing.
- USB failures cannot starve input or audio channels.
- USB status clearly says whether receiver-side virtual bus support exists on the platform.

## RC: Release Hardening

Goal: turn the feature set into a release candidate with operational confidence.

Required work:

- Run 24-hour and 72-hour soak tests across Windows to Windows and Windows to macOS paths.
- Validate lock screen, UAC, login, sleep/wake, fast user switching, and helper degradation semantics.
- Add upgrade, rollback, configuration migration, and diagnostic bundle workflows.
- Finalize platform compatibility matrix and known limitations.
- Freeze public IPC fields or add explicit versioning and migrations.

Exit criteria:

- Release candidate passes long-run stability and restart/reconnect scenarios.
- Diagnostics explain failures without requiring source-code inspection.
- Experimental USB remains clearly labeled if it is not production ready.

## Verification For Each Implementation Batch

Every future code batch should include:

- Unit tests for new model or protocol behavior.
- Integration tests for daemon IPC and device transport when public contracts change.
- UI validation when capability fields are added or renamed.
- Manual dual-machine notes when behavior depends on real hardware.
- A roadmap/doc update when stage exit criteria change.
