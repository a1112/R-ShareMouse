# Low-Latency Local Feedback Design

Date: 2026-05-23

## Goal

Complete low-latency local feedback for keyboard, mouse, and gamepad activity by making the daemon the authoritative source of feedback state while keeping the existing localhost WebSocket as the fast UI event stream.

## Scope

In scope:

- Daemon-owned local input feedback that includes keyboard, mouse, and gamepad activity.
- Additive IPC fields for gamepad feedback in `LatencyFeedbackSnapshot.local_input`.
- Desktop UI feedback that prefers daemon summaries and uses WebSocket events for immediate visual updates.
- CLI and frontend tests that prove all three input families report consistent status.

Out of scope:

- Changing device-to-device transport. Peer input traffic remains QUIC.
- Introducing Tencent Cloud, public relay, remote WebSocket, or WebSocket-based peer transport.
- Rewriting capture or injection backends.
- Optimizing QUIC transport performance before the feedback baseline is trustworthy.

## Current State

The project already has the main foundation:

- `ServiceStatusSnapshot.latency_feedback` exposes daemon-owned local input, remote latency, and transport health.
- `ws://127.0.0.1:27436/local-controls` streams `LocalControlEvent` updates to desktop clients.
- The desktop UI merges WebSocket events into `LocalControlsSnapshot` for immediate keyboard, mouse, and gamepad visuals.
- Hardware assets already render keyboard, mouse, and gamepad surfaces through manifests.

The gap is that `DaemonState::local_input_feedback()` currently treats only keyboard and mouse events as eligible local input feedback. Gamepad state is visible in `LocalControlsSnapshot`, but it is not part of the daemon-owned low-latency summary.

## Architecture

Keep the same transport split:

```text
device-to-device input/control traffic -> QUIC
desktop/CLI to local daemon status      -> localhost JSON IPC
desktop live local event feed           -> localhost WebSocket
```

The daemon remains the source of truth. The UI may use WebSocket events to render a fresh frame before the next status poll, but it must not invent a separate product-level health model.

## Data Model

Extend `LocalInputFeedback` additively:

- `latest_gamepad_event_ms`
- `latest_gamepad_id`
- `latest_gamepad_event_kind`
- `latest_gamepad_button`
- `latest_gamepad_axis`

Existing fields remain compatible:

- `event_count` becomes the aggregate keyboard + mouse + gamepad event count.
- `latest_event_ms` and `latest_sequence` may point to keyboard, mouse, or gamepad, whichever is newest.
- `latest_keyboard_event_ms` and `latest_mouse_event_ms` keep their current meaning.

No existing field is removed or repurposed incompatibly.

## Daemon Behavior

`DaemonState::local_input_feedback()` should:

- Include local `Keyboard`, `Mouse`, and `Gamepad` diagnostic events.
- Continue excluding remote mirrored events and `remote-daemon` diagnostics.
- Report `Unavailable` when no capture backend mode is selected.
- Report `Idle` when the selected backend is present and no local input event has been observed.
- Report `Healthy` when any eligible keyboard, mouse, or gamepad event exists.
- Report `Degraded` when backend aggregate health is degraded.

Gamepad event count should come from `LocalControlDeviceSnapshot.gamepads[*].event_count`, using saturating addition.

## WebSocket Behavior

Keep the existing endpoint:

```text
ws://127.0.0.1:27436/local-controls
```

On connect, the daemon sends a full `DaemonResponse::LocalControls` snapshot. After that it sends `DaemonResponse::LocalControlEvent` entries. This is already enough for low-latency UI updates, so the first implementation should not add a second WebSocket message kind.

The desktop UI should:

- Apply keyboard, mouse, and gamepad events immediately to local state.
- Preserve daemon `latency_feedback` from the status snapshot as the authoritative summary.
- Show event-derived freshness only as a temporary UI freshness signal until the next status refresh arrives.

## Desktop UI Behavior

The Devices page should expose a compact low-latency feedback strip:

- Keyboard: status, event count, latest event age.
- Mouse: status, event count, latest event age.
- Gamepad: status, event count, latest event age, latest button or axis.
- Transport: QUIC health and RTT from daemon `latency_feedback.transport`.
- Remote RTT: selected remote probe summary from daemon feedback with existing event fallback.

The keyboard, mouse, and gamepad hardware panels should keep their current manifest-based visuals. Their active highlights continue to come from the WebSocket-updated `LocalControlsSnapshot`.

## CLI Behavior

`rshare-cli status --detailed` already prints local input feedback. It should include gamepad details when available, without hiding existing keyboard and mouse details.

## Error Handling

- Missing gamepad fields default to `None` and do not break older clients.
- A machine without a gamepad should still report keyboard/mouse feedback normally.
- Gamepad disconnect events count as local input feedback because they matter for operator visibility.
- WebSocket disconnect does not change daemon truth; the UI falls back to the next polling snapshot.

## Testing

Rust tests:

- IPC JSON round trips include the new optional gamepad feedback fields.
- Old snapshots without gamepad fields default safely.
- Daemon local input feedback includes gamepad-only events.
- Aggregate event count saturates across keyboard, mouse, and gamepad counters.
- Backend degraded status applies to keyboard, mouse, and gamepad feedback.

Frontend tests:

- WebSocket-style gamepad events update the local controls model immediately.
- Local feedback display maps daemon keyboard, mouse, and gamepad fields into stable UI rows.
- Existing remote latency summary still prefers daemon feedback and keeps event fallback behavior.

Verification:

- `cargo test -p rshare-core latency_feedback`
- `cargo test -p rshare-daemon local_input_feedback`
- `cargo test -p rshare-cli latency_feedback`
- `npm test` in `other/figma-ui`
- `npm run build` in `other/figma-ui`

## Acceptance Criteria

- Mouse, keyboard, and gamepad activity appears immediately in the desktop UI through the local WebSocket stream.
- `status --detailed` reports local input feedback with gamepad details when gamepad events exist.
- `ServiceStatusSnapshot.latency_feedback.local_input` is sufficient for clients to determine local feedback status without recomputing health.
- Device-to-device low-latency traffic remains QUIC.
- WebSocket remains a localhost-only UI event stream.
