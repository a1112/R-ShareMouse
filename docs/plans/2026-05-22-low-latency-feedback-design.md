# Low-Latency Feedback Monitoring Design

Date: 2026-05-22

## Goal

Build the monitoring and feedback system needed before latency optimization work. The first milestone is observability: make the daemon expose one consistent, low-latency source of truth for local input feedback, remote endpoint latency, and transport health. Actual transport and input-path optimization are intentionally deferred until the feedback baseline is reliable.

## Scope

In scope:

- Daemon-owned feedback state for input, remote latency probes, and transport health.
- Stable IPC fields that GUI and CLI clients can consume without duplicating health logic.
- Event-stream updates for fast UI feedback.
- Tests that prove default values, state transitions, and client model rendering.

Out of scope:

- QUIC/datagram performance tuning.
- Input backend rewrites.
- New pairing or trust behavior.
- Generic USB latency optimization.

## Existing Foundation

The repository already has the key building blocks:

- `ServiceStatusSnapshot.network` exposes transport diagnostics.
- `LocalControlDeviceSnapshot.recent_events` exposes local and remote diagnostic events.
- `LatencyProbe` and `LatencyProbeAck` measure remote round trip and remote processing time.
- The desktop model can summarize latency probe events for a selected device.
- CLI `status --detailed` already prints basic transport diagnostics.

The gap is that these signals are still event-shaped and scattered. UI and CLI code must infer state from recent events, and there is no daemon-owned summary that can say whether feedback is idle, pending, healthy, degraded, timed out, or unavailable.

## Recommended Approach

Add a daemon-owned latency feedback summary, built from existing diagnostic events and transport state.

The daemon remains authoritative. Clients render the summary and subscribe to event deltas, but they do not recompute product truth. This matches the Alpha-2 guidance that clients are read-only views of daemon-owned state.

## Data Model

Introduce a serializable feedback model in `rshare-core`, with safe defaults for old snapshots:

- `LatencyFeedbackSnapshot`
  - `generated_at_ms`
  - `local_input`
  - `remote_latency`
  - `transport`
- `LatencyFeedbackStatus`
  - `Idle`
  - `Pending`
  - `Healthy`
  - `Degraded`
  - `Timeout`
  - `Unavailable`
- `LocalInputFeedback`
  - latest event sequence and timestamp
  - latest keyboard/mouse event timestamp
  - capture path
  - event count
  - status
- `RemoteLatencyFeedback`
  - per-device summaries
  - last sent timestamp
  - last ACK timestamp
  - network RTT
  - raw RTT
  - estimated one-way latency
  - remote processing time
  - pending duration
  - status
- `TransportFeedback`
  - transport name
  - datagram availability
  - realtime degraded flag
  - RTT
  - last datagram receive age
  - datagram drops
  - reliable reset count
  - certificate trust state
  - status

The first implementation should prefer additive fields under existing IPC snapshots instead of replacing current event payloads. A good target is `ServiceStatusSnapshot.latency_feedback`, because status is the common CLI and desktop entry point. `LocalControlDeviceSnapshot.recent_events` should remain the event history used for detailed inspection and subscription updates.

## State Rules

Local input feedback:

- `Unavailable` when no capture backend is selected or backend health is degraded.
- `Idle` when the backend is healthy but no local input event has been observed.
- `Healthy` when recent keyboard or mouse events exist.
- `Degraded` when the aggregate backend health is degraded after events were previously observed.

Remote latency feedback:

- `Unavailable` when the target device is not connected.
- `Idle` when connected but no probe has run.
- `Pending` after a probe is sent and before an ACK or timeout.
- `Healthy` when the latest ACK is within threshold.
- `Degraded` when ACK exists but RTT exceeds the warning threshold or realtime transport is degraded.
- `Timeout` when a pending probe exceeds the configured timeout.

Transport feedback:

- `Unavailable` when there are no connected peers.
- `Healthy` when realtime datagrams are available and RTT is within threshold.
- `Degraded` when realtime is degraded, datagram drops increase, reliable resets occur, or RTT exceeds threshold.

Initial thresholds should be conservative constants in daemon code, not user settings:

- remote latency pending timeout: 1500 ms
- healthy RTT threshold: 50 ms
- degraded RTT threshold: 120 ms

These thresholds can become configuration later after real validation.

## Data Flow

```text
local input / network messages
  -> daemon records diagnostic events
  -> daemon updates feedback summary from state and recent events
  -> IPC status exposes latency_feedback
  -> LocalControls subscription continues to stream raw event deltas
  -> CLI and desktop render the same status labels and metrics
```

Remote latency probes continue to use the existing protocol. The feedback system should summarize the existing probe lifecycle instead of adding a new network message for the first milestone.

## Client Behavior

CLI:

- `rshare-cli status --detailed` prints a `Latency Feedback` section.
- It shows local input status, transport status, and remote latency status per connected peer.
- Existing network details remain visible for diagnostics.

Desktop:

- The desktop model consumes the daemon summary when present.
- It falls back to current `recent_events` parsing when connected to an older daemon.
- Device pages show pending, success, timeout, and degraded states from the daemon summary.

## Error Handling

All feedback fields must default safely:

- Missing summary means clients show `unavailable` or use current fallback logic.
- Missing timestamps become `None`/`null`, not zero-valued fake success.
- Timeout detection is daemon-side and monotonic enough for status display.
- Unknown devices must not poison the aggregate snapshot.

## Testing

Rust contract tests:

- IPC JSON round trips with the new feedback snapshot.
- Old/missing fields default safely.
- Status snapshot defaults remain healthy and empty.

Daemon tests:

- local input event updates local input feedback.
- latency probe sent creates pending feedback.
- ACK creates healthy feedback with RTT fields.
- stale pending probe becomes timeout.
- disconnected peer reports unavailable.
- degraded transport makes transport feedback degraded.

Frontend model tests:

- desktop model prefers daemon feedback over event inference.
- fallback from `recent_events` still works.
- pending, healthy, degraded, timeout, and unavailable states render consistently.

CLI tests:

- detailed status prints the latency feedback section from the daemon snapshot.

## Implementation Order

1. Add core feedback types and IPC defaults.
2. Add daemon aggregation helpers and unit tests.
3. Wire feedback into `ServiceStatusSnapshot`.
4. Update CLI detailed status output.
5. Update desktop model and tests.
6. Run focused Rust and frontend tests.

## Future Optimization Work

Once this monitoring layer is in place, optimization work can use these metrics as acceptance criteria:

- reduce remote ACK time for normal LAN input loops;
- avoid realtime fallback during steady input;
- lower UI feedback delay for local keyboard and mouse events;
- detect transport regressions before they become subjective user reports.
