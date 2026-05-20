# Capability-Oriented Endpoint Architecture Design

Date: 2026-05-20

Status: planning document

Related research: [USB Device Forwarding Feasibility Research Archive](../research/2026-05-20-usb-device-forwarding-deep-research.md)

## Summary

R-ShareMouse should evolve as a capability-oriented endpoint sharing platform. The primary product path remains low-latency keyboard and mouse sharing. Generic USB forwarding is a separate experimental capability for devices that cannot be represented as input, audio, gamepad, display, clipboard, or file-transfer events.

The architecture must keep the daemon authoritative. GUI, CLI, and Tauri clients should consume a single daemon snapshot and send explicit action requests. They must not infer runtime truth from local UI state.

## Product Boundary

The default sharing path is event-level remoting:

- Keyboard, mouse, wheel, edge transition, and hotkey events use the input path.
- Gamepad state and buttons use a gamepad capability path.
- Speaker, microphone, and loopback capture use dedicated audio capability paths.
- Monitor geometry and cursor position use display topology and observation paths.
- Clipboard and file transfer use their own reliable application channels.
- Generic USB forwarding is reserved for devices that require real USB identity, descriptors, or vendor drivers.

This boundary prevents the project from turning every peripheral into USB-over-IP. That matters because full USB forwarding brings driver ownership, virtual bus materialization, isochronous transfer, security, and recovery risks that are not required for normal keyboard, mouse, audio, or display sharing.

## Capability Model

Each endpoint should advertise a stable capability snapshot. The daemon owns this snapshot and exposes it to UI and CLI.

Capability names to reserve:

- `input`: keyboard, mouse, wheel, hotkey, edge switching, and injection.
- `clipboard`: text, rich text, images, size limits, deduplication, and loopback suppression.
- `gamepad`: controller discovery, state observation, and future injection.
- `audio`: microphone, speaker, loopback, level, mute, routing, and stream state.
- `display_topology`: monitor geometry, DPI, cursor position, active display, and edge mapping.
- `usb_host`: local physical USB device enumeration and host-side transfer export.
- `usb_receiver`: receiver-side virtual USB materialization and claim/import state.
- `privileged_helper`: elevated input/helper coverage for lock screen, UAC, login, and system desktop.
- `diagnostics`: logs, latency, dropped events, backend health, and failure reasons.

Each capability should report:

- `capability_id`
- `kind`
- `version`
- `state`: `available`, `degraded`, `unavailable`, or `experimental`
- `health_reason`
- `permission_state`
- `latency_metrics`
- `last_event_at`
- `transport_state`

The exact Rust types can be introduced later, but any implementation should preserve these fields as the minimum design contract.

## Provider Interfaces To Reserve

The implementation should split capabilities behind provider-style boundaries instead of adding more one-off daemon fields.

Reserved provider concepts:

- `EndpointCapabilityProvider`: builds the per-device capability inventory.
- `EndpointEventProvider`: emits local observation events into the daemon event store.
- `EndpointInjectProvider`: accepts validated injection requests and returns explicit results.
- `InputCapabilityProvider`: captures and injects keyboard, mouse, wheel, and hotkey events.
- `GamepadCapabilityProvider`: observes controller topology and live state.
- `AudioCapabilityProvider`: observes endpoints and manages capture/playback/forwarding sessions.
- `DisplayTopologyProvider`: reports monitors, DPI, cursor position, and edge geometry.
- `UsbHostProvider`: enumerates exportable local USB devices and executes host transfers.
- `UsbReceiverProvider`: materializes remote USB devices through a platform-specific virtual bus.
- `DiagnosticsProvider`: publishes health, latency, queue, transport, and permission diagnostics.

These are design names, not a requirement to add traits immediately. The important rule is that future implementation should attach new device categories through capability providers, not by hardcoding UI-specific payloads.

## Event And Injection Bus

`EndpointEvent` and endpoint injection should remain the canonical observation/injection bus.

Design rules:

- Local providers publish normalized endpoint events to the daemon.
- The daemon stores recent events and exposes filtered local/remote streams.
- Remote observation and injection reuse the device transport protocol.
- UI pages render the daemon event stream; they do not subscribe directly to platform APIs.
- Injection requests must identify target device, capability, event kind, sequence number, and requested reliability.
- Injection results must return success/failure, latency, backend used, and failure reason.

Input events must remain low-latency and should not be delayed by heavy USB, audio, or diagnostic traffic.

## Transport Layering

The device-to-device transport should stay layered by traffic class:

- Realtime QUIC datagram: mouse move, high-frequency gamepad state, and disposable diagnostics.
- Reliable QUIC stream: key events, mouse button, wheel, hotkeys, layout control, clipboard control, endpoint injection requests, and injection results.
- Audio stream: dedicated stream/channel with its own buffering, drift, and underrun reporting.
- USB transfer stream: experimental, flow-controlled, explicitly claimed, and isolated from the primary input path.
- Local IPC: localhost JSON IPC remains a UI/CLI to daemon contract and does not need to expose transport internals except as diagnostics.

Mouse movement can drop stale frames. Keyboard, button, wheel, hotkey, clipboard, and control messages must be reliable and ordered.

## USB Forwarding Boundary

Generic USB forwarding must stay opt-in and experimental until receiver-side virtual USB materialization is implemented and tested.

USB forwarding is appropriate for:

- License dongles that require real VID/PID or vendor drivers.
- Vendor-specific scanners and card readers.
- Serial adapters, MCU boards, debug probes, and lab instruments.
- Storage devices only when file sharing or drive mapping is insufficient.

USB forwarding is not the default path for:

- Keyboard and mouse.
- Audio speakers, headphones, microphones, and loopback capture.
- Gamepads that can be represented as controller events.
- Display topology and cursor position.

Generic USB implementation must require allowlists, explicit claim/release, transfer flow control, hotplug recovery, and security review before it can move out of experimental status.

## Security And Trust

Capabilities must be tied to device trust and permissions.

Minimum design expectations:

- Device identity and transport certificate state are visible in diagnostics.
- Capability availability is false when the target device is untrusted.
- USB export requires explicit allowlist or per-device claim.
- Privileged helper state is reported separately from normal user-mode capability health.
- A degraded capability must include a user-actionable reason.

## Acceptance Criteria

This design is satisfied when subsequent implementation can:

- Render local and remote devices from a unified capability snapshot.
- Observe keyboard, mouse, gamepad, audio, display, and USB state through one endpoint event model.
- Inject input through validated endpoint injection requests with visible results.
- Keep HID/input latency isolated from USB and audio experiments.
- Explain why a capability is unavailable without relying on logs alone.
