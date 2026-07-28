# Low-Latency Control and Display Architecture Design

Date: 2026-07-28  
Status: Approved

## Executive Summary

R-ShareMouse will evolve in two isolated stages:

1. Make keyboard, mouse, gamepad, daemon state, and desktop feedback bounded, measurable, and low latency.
2. Add a separate single-display 1080p60 remote video path for a selected physical or IDD virtual display.

The selected architecture is a QoS-layered refactor rather than a set of timing tweaks or a full daemon rewrite. The input path will no longer wait for diagnostics, UI state, audio, clipboard, USB, screenshots, or video. Video will use a separate media connection and native GPU pipeline so media congestion cannot create control-path head-of-line blocking.

The first acceptance environment is Windows 11 to Windows 11 over a wired LAN with a 1000 Hz mouse. The peer protocol may be versioned incompatibly during Alpha; old peers must be rejected explicitly during the handshake.

## Confirmed Product Decisions

- Deliver control and state-display latency improvements before remote video.
- Use Windows 11 wired-LAN dual-machine validation as the first release gate.
- Permit a peer protocol version bump and reject incompatible nodes.
- Target one selected physical or IDD virtual display per media session.
- Target 1920x1080 at 60 fps with hardware encoding and decoding.
- Target video glass-to-glass p95 latency at or below 65 ms.
- Keep input control available when media fails.
- Close media immediately when the trusted control identity or authorization session fails.

## Current Architecture Findings

The current repository already has:

- daemon-owned layout, session, backend health, and endpoint state;
- compact QUIC datagrams for mouse movement and gamepad snapshots;
- a persistent reliable QUIC stream for other messages;
- local WebSocket input feedback;
- display topology, static screenshot preview, and Windows IDD virtual displays;
- remote latency probes and status summaries.

It does not have a continuous remote video pipeline. Existing display capture is a user-triggered local BMP thumbnail transported through JSON.

The audit identified the following structural latency risks:

1. Each captured input event records and broadcasts `InputDiagnostic` and `EndpointEventDelta` before the real control message is sent.
2. Datagram, reliable input, diagnostics, audio, clipboard, and USB share one connection writer; all reliable traffic shares one stream.
3. Daemon, network manager, and connection-pool mutexes are held across asynchronous sends.
4. Capture paths use unbounded queues, turning backpressure into stale input, growing latency, and memory growth.
5. Reliable and realtime receive paths merge into shared FIFO queues and are processed by one daemon event task.
6. Input injection is synchronous on a Tokio worker and the portable/native emulators add a fixed 1 ms delay.
7. Windows filter capture sleeps for 16 ms when its polling queue is empty.
8. The router uses a fixed 1920x1080 geometry rather than the real virtual desktop.
9. IPC reads newline JSON one byte at a time, and static BMP bytes are serialized as JSON number arrays.
10. Local input batches update the desktop root state every 8 ms, rebuilding large models and topology-derived arrays.
11. Endpoint push and 750 ms polling run concurrently, while dashboard state is assembled from multiple non-atomic requests.
12. The repository has correctness tests but no control-path, IPC, QUIC-load, event-to-paint, or video benchmarks.

The same source revision passed the existing focused baseline before isolation:

- daemon: 195 passed;
- network: 59 passed, 1 ignored manual discovery test;
- desktop frontend: 177 passed.

These tests prove functional behavior, not the proposed latency objectives.

## Goals

### Control Plane

- Wired-LAN capture-to-remote-inject p99 at or below 10 ms.
- Zero lost, duplicated, or reordered key/button/session-barrier events.
- Bounded memory and queue age under 1000 Hz pointer input.
- No stale mouse trajectory replay after congestion or reconnect.
- A slow peer, UI subscriber, bulk transfer, or media session must not block another peer's input.
- Correct continuous movement across different resolutions, DPI values, and negative-coordinate multi-monitor layouts.
- Fail-safe release of held input on every ownership, connection, backend, and process failure.

### State Display and Local IPC

- Local input event-to-paint p95 at or below 16.7 ms and p99 at or below 33 ms.
- Display topology changes visible in the UI at p95 at or below 50 ms.
- No dashboard or endpoint polling while the push stream is healthy.
- No root/topology rerender caused solely by high-frequency mouse movement.
- Explicit revision-gap detection and resynchronization.
- Framed, buffered IPC and binary delivery for image payloads.

### Video

- One selected physical or IDD virtual display.
- 1080p60 H.264 hardware pipeline on Windows 11.
- Glass-to-glass p95 at or below 65 ms on wired LAN.
- Bounded capture, encode, packet, decode, and presentation queues.
- Media load must add no more than 5 ms to control p99.
- Latest complete frame wins; expired video is dropped rather than replayed.

## Non-Goals

- Backward compatibility with the current Alpha peer protocol.
- Multi-display simultaneous video encoding in the first media release.
- 4K, HDR, HEVC, AV1, recording, public relay, or WAN optimization in the first media release.
- Audio/video synchronization in the first media release.
- Replacing the existing virtual display driver.
- Routing video frames through daemon JSON IPC or the existing general `Message` stream.
- Making USB forwarding part of the primary control or media path.

## Top-Level Architecture

```text
local capture
  -> semantic bounded ingress
  -> InputRouter actor
  -> realtime input datagrams / reliable input stream
  -> remote InputInjection actor

InputRouter + connection events + platform inventory
  -> daemon state aggregator
  -> versioned UiSnapshot + UiDelta stream
  -> slice-based desktop stores

selected physical/IDD display
  -> GPU capture
  -> hardware video encode
  -> separate media QUIC connection
  -> hardware decode
  -> native D3D11 presentation surface

audio / clipboard / USB / files / diagnostics
  -> separate control, bulk, or telemetry lanes
```

The daemon remains authoritative, but ownership is split by actor rather than one large shared state lock:

- `InputRouter` owns the active input session, pressed-state ledger, route cache, target geometry, and forwarding sequence.
- `ConnectionRegistry` owns peer lifecycle and cloneable transport handles.
- `InputInjection` owns the platform injection backend on one dedicated OS thread.
- `StateAggregator` consumes typed state changes and publishes immutable snapshots/deltas.
- `MediaSessionManager` owns capture/encode/decode/render session lifecycle.
- Low-priority telemetry workers own formatting, JSON projection, and sampled diagnostics.

## Control Data Model

### Input Session Identity

Every active control session has:

- `owner_peer_id`;
- `session_epoch`;
- selected target endpoint and display;
- reliable sequence;
- realtime sequence;
- pressed key/button ledger;
- lease deadline.

The epoch increments on enter, return, ownership transfer, reconnect, reset, or suspension. A receiver accepts input only from the current owner and current epoch.

### Realtime Frames

Realtime datagrams carry replaceable state:

```text
RealtimeInputFrame {
  protocol_version,
  session_epoch,
  sequence,
  captured_at_mono,
  kind,
  payload
}
```

Initial kinds:

- relative mouse motion;
- optional absolute position anchor;
- gamepad axes/triggers;
- cursor visual state when required.

Rules:

- accept only sequences newer than the last accepted sequence for the epoch;
- count gaps, duplicates, and out-of-order frames;
- never replay an old realtime frame through the reliable stream;
- on send congestion, replace the pending realtime value with the latest value;
- datagram failure is observable but does not enqueue stale motion.

### Reliable Input Frames

The dedicated reliable input stream carries:

- session `Enter`, `Leave`, `ReleaseAll`, and ownership barriers;
- key press/release and text commit;
- mouse button press/release;
- wheel deltas;
- gamepad connect/disconnect and button transitions;
- backend/session failure acknowledgments.

Mouse button frames include the click coordinate and the realtime motion sequence they anchor. If the corresponding motion datagram was lost, the receiver positions the pointer before applying the button event.

Reliable input uses a compact binary frame and a dedicated persistent QUIC stream. It does not share a stream with diagnostics, audio, clipboard, USB, or video.

## Semantic Bounded Input Ingress

OS hooks and driver callbacks must not block on asynchronous work.

Input ingress therefore separates:

- replaceable continuous state: latest mouse motion and gamepad axes;
- non-replaceable discrete state: keys, buttons, wheel, text, and session barriers.

Consecutive motion may be coalesced, but never across a discrete-event barrier. Each discrete mouse event snapshots the latest pointer coordinate.

All queues are bounded. If the reliable discrete queue cannot accept a frame:

1. mark the backend/control capability degraded;
2. suspend remote control;
3. emit a fail-safe `ReleaseAll` through the reserved emergency path;
4. require a new session epoch before accepting further remote input.

Silent key/button drops are forbidden.

Each ingress item records a monotonic capture timestamp so queue residence can be measured.

## Input Router

One `InputRouter` actor replaces the independent routing engines currently used by different capture paths. It receives Windows hooks/filter events, portable capture, gamepad events, and connection/layout changes in one ordered model.

The router:

- caches connected layout neighbors instead of rebuilding a `HashSet` per event;
- owns the session state machine and active pressed-state ledger;
- uses real virtual desktop bounds, display geometry, scale, and DPI;
- establishes a target-side absolute position on edge entry;
- prefers raw relative motion while remote control is active;
- produces reliable barriers before any new-epoch realtime frames are accepted;
- publishes small typed state changes to the state aggregator;
- updates lightweight metrics without constructing strings or JSON.

Formatting recent-event descriptions and projecting endpoint diagnostics happens outside the router at a sampled 10-20 Hz rate.

## Transport QoS

Each peer exposes cloneable handles for distinct traffic classes:

1. realtime input datagrams;
2. reliable input stream;
3. control/state stream;
4. bulk streams;
5. sampled telemetry;
6. a separately negotiated media connection.

Connection-table locks are held only long enough to clone a handle. No map, registry, manager, or daemon mutex may be held while awaiting socket I/O.

Broadcast snapshots the target handles first and sends independently so one slow peer cannot block another.

Reliable input has strict priority over control and bulk traffic. Realtime input bypasses the general reliable writer. Media uses a second QUIC connection with a bitrate cap and separate queue/congestion state.

## Remote Injection

The remote injection backend is owned by a dedicated ordered OS thread or actor, not by the Tokio network event task.

The injection path:

- validates peer ownership, epoch, and sequence;
- rejects late or duplicate realtime state;
- repairs pointer position from a reliable button anchor when needed;
- invokes the platform backend without an unconditional sleep;
- records dequeue, call-start, and completion timestamps;
- updates the held-state ledger only after successful injection;
- executes local `ReleaseAll` on timeout or backend failure.

The existing `event_delay` defaults to zero in realtime mode. Any backend-specific delay must be explicit, measured, and outside the async runtime.

Windows filter input must move to blocking/overlapped/event-driven reads. A shorter adaptive poll is only an interim fallback and must not remain the final low-latency path.

## UI State Stream

The daemon exposes one persistent versioned UI stream. The initial envelope is:

```text
UiSnapshot {
  boot_id,
  revision,
  status,
  devices,
  layout,
  capabilities,
  display_inventory,
  dynamic_state,
  active_sessions
}
```

Subsequent envelopes are typed deltas with a monotonic revision:

- status/capability change;
- device upsert/remove;
- layout/display topology change;
- latest pointer/gamepad state;
- key/button transition;
- session transition;
- diagnostics/latency update;
- media session update;
- `ResyncRequired`.

State is split into:

- static inventory, refreshed by OS notification, explicit request, or low-frequency TTL;
- latest dynamic values, distributed with `watch`/latest-value semantics;
- bounded non-replaceable event history.

A subscriber that lags does not silently continue. It receives `ResyncRequired` and fetches a new consistent snapshot.

## Desktop State and Rendering

The frontend uses one transport-independent `UiStateClient` and identical `UiEnvelope` behavior in packaged Tauri and browser development modes.

Frontend state is partitioned into selector-driven stores:

- topology/layout;
- local and remote input visuals;
- diagnostics/latency;
- connection/capabilities;
- media session/rendering.

Mouse and gamepad continuous state is applied once per `requestAnimationFrame`, using only the newest value. Discrete key/button transitions apply immediately. A pointer update must not rebuild the desktop topology model or replace the `externalDevices` array.

When push is healthy:

- dashboard polling is disabled;
- endpoint polling is disabled;
- only a five-second heartbeat/watchdog remains.

Disconnect triggers bounded reconnect with revision resynchronization. Polling is used only as a temporary disconnected fallback.

## Local IPC and Static Display Capture

Ordinary daemon TCP IPC moves to a framed protocol:

```text
u32 payload_length
u8 envelope_kind
payload
```

The first implementation may keep JSON payloads for low-rate commands, but reading must use buffered framed I/O rather than one-byte reads. Binary payloads never become JSON number arrays.

Static display preview remains user-triggered and separate from video:

- capture runs on a blocking/platform worker;
- thumbnails are compressed to PNG or JPEG;
- responses use binary frames, a blob, or a temporary resource URL;
- capture concurrency is bounded;
- the UI does not continuously refresh thumbnail backgrounds.

## Media Session Protocol

The trusted control connection negotiates:

```text
StartMediaSession {
  session_id,
  display_id,
  codec_preferences,
  max_width,
  max_height,
  max_fps,
  max_bitrate,
  media_endpoint,
  one_time_token
}
```

The token is short-lived, single-use, and bound to:

- the authenticated device identity;
- the active control connection;
- the selected display;
- the media session id.

The media connection reuses the existing peer certificate identity and trust decision. Invalid or expired tokens fail closed.

## Windows Video Pipeline

The initial `rshare-media` Windows implementation uses:

1. Windows Graphics Capture with D3D11 textures;
2. DXGI Desktop Duplication as a fallback;
3. GPU conversion/scaling to NV12;
4. Media Foundation hardware H.264 encoding;
5. QUIC datagram packetization on the media connection;
6. Media Foundation hardware decode to D3D11 textures;
7. a native D3D11 child presentation surface attached to the Tauri window.

Low-latency encoder settings:

- no B frames;
- no lookahead;
- shallow reference and output queues;
- regular keyframes/intra refresh;
- bitrate adaptation between approximately 8 and 20 Mbps;
- 1920x1080 at 60 fps as the initial target.

The implementation must avoid CPU readback when capture, conversion, encoder, decoder, and renderer support shared GPU textures.

## Media Packetization and Recovery

Video datagrams carry:

```text
VideoPacket {
  session_id,
  frame_id,
  packet_index,
  packet_count,
  pts,
  keyframe,
  payload
}
```

Rules:

- packet payload respects the negotiated QUIC datagram/MTU limit;
- reassembly is bounded by frame count, bytes, and deadline;
- an incomplete non-key frame is dropped at its playout deadline;
- the receiver requests a fresh keyframe rather than replaying expired frames;
- the sender queue prefers the newest frame and drops old delta frames;
- congestion first lowers bitrate, then drops old frames, then reduces resolution;
- media pressure cannot block or grow the input queues.

The initial jitter buffer holds no more than approximately one frame. Presentation selects the newest complete decodable frame.

## Cursor Presentation

Where supported, the captured video omits the source cursor. Cursor position and shape are sent as low-cost realtime/control state and rendered as a local overlay on the receiver.

This allows perceived pointer response to follow the control path rather than waiting for capture, encode, network, decode, and presentation. Reliable mouse-button anchors keep the overlay and injected click position consistent.

## Failure Handling

### Control

The receiver executes local `ReleaseAll` on:

- control connection loss;
- active-session lease timeout;
- ownership transfer;
- daemon shutdown;
- backend failure;
- lock/sleep/session change;
- reliable input overflow;
- explicit user stop.

Reconnect never restores old held state. A new `Enter` and epoch are required.

### Handshake and Connection Registry

A peer enters the canonical connection pool only after:

- protocol version validation;
- application namespace validation;
- peer identity confirmation;
- capability negotiation;
- certificate trust confirmation.

Invalid, missing, or timed-out Hello traffic is closed and cannot create a ghost Connected device.

### UI State

`boot_id` changes and revision gaps force a full resync. Transient stream errors retain the last known snapshot with a stale/disconnected marker rather than clearing the entire UI.

### Media

Capture loss, display removal, encoder/decoder failure, lock screen, token revocation, or control identity loss ends the media session and publishes a truthful reason. Media failure alone does not suspend input control.

## Security

- Only a trusted and currently authenticated peer may request media or input ownership.
- The receiver enforces one active control owner.
- Session epochs prevent packets from an earlier owner or connection from being applied.
- Media authorization is explicit, scoped to one display, and revocable.
- Display sharing stops on lock/session change unless a future privileged policy explicitly allows it.
- Diagnostics are subscription-aware and do not broadcast all local input activity to every connected peer.
- Protocol/version mismatch and capability absence fail closed.

## Observability

Control measurements record monotonic timestamps at:

1. capture callback;
2. ingress enqueue;
3. router dequeue;
4. transport enqueue/write;
5. remote receive;
6. injection dequeue/call/completion.

Two-machine wall clocks are not assumed synchronized. Local stage histograms and probe acknowledgments report remote receive-to-inject duration. True motion-to-photon/key-to-paint is validated with a high-speed camera or external loopback rig.

Published metrics include:

- p50/p95/p99 and maximum queue residence;
- realtime overwrite, sequence gap, duplicate, and out-of-order counts;
- reliable input queue depth/age and emergency suspensions;
- injection service time and backend failures;
- per-lane bytes, stalls, resets, and slow-peer isolation;
- UI stream revision lag/resync count;
- UI event-to-paint and component commit count;
- media capture, encode, packet, reassembly, decode, and presentation latency;
- video frame/packet drops, keyframe requests, bitrate, and queue age.

Telemetry aggregation is low priority and bounded. Metrics collection must not allocate or format strings per mouse event.

## Testing Strategy

All behavior changes use test-driven development.

### Deterministic Unit and Actor Tests

- 100,000 mouse moves followed by key/button barriers keep memory bounded and preserve the final pointer state.
- Motion coalescing never crosses a discrete-event barrier.
- Reliable overflow suspends the session and triggers fail-safe release.
- Epoch sequence `10, 12, 11` accepts only valid newer state.
- An old owner cannot inject after ownership transfer.
- A missing motion datagram followed by a button frame repairs the click coordinate.
- A blocked telemetry or bulk worker does not delay input scheduling.
- A slow peer does not block a fast peer or connection status reads.
- Invalid Hello never enters the connection pool.
- UI revision gaps cause resynchronization.
- Healthy UI push disables fallback polling.
- Topology selectors do not update for pointer-only deltas.
- Missing video fragments expire without unbounded reassembly.
- Media congestion drops stale frames and requests keyframes without affecting input.

### Performance Harnesses

- Microbenchmarks for routing, session transitions, input framing, codec operations, and allocation count.
- Real localhost framed-IPC sequential and concurrent benchmarks.
- Warm QUIC loopback at 125, 500, and 1000 Hz with concurrent diagnostics, status requests, audio, and bulk traffic.
- Two-peer slow/fast isolation tests.
- Windows UI event-to-paint and render-commit tests on a fixed runner.
- Windows media capture-to-present stage benchmarks.

Hosted CI uses broad regression limits. A fixed Windows nightly runner records histograms and fails when median regresses by more than 10% or p95/p99 regresses by more than 15% across five runs.

## Acceptance Criteria

### Control

- Wired mouse capture-to-inject: p50 at or below 3 ms, p95 at or below 6 ms, p99 at or below 10 ms.
- Reliable key/button path: p95 at or below 8 ms, p99 at or below 15 ms.
- Jitter `p99 - p50` at or below 4 ms.
- Zero lost or duplicated reliable input and zero stuck modifiers over ten minutes of mixed load.
- A 100 ms network stall does not replay stale pointer history and converges to the latest state within 20 ms of recovery.
- Media/bulk load adds no more than 5 ms to input p99.
- Slow-peer load adds no more than 2 ms to a fast peer's p99.

### State Display

- Input-to-paint p95 at or below 16.7 ms and p99 at or below 33 ms.
- Topology/status change-to-UI p95 at or below 50 ms and p99 at or below 100 ms.
- No periodic dashboard/endpoint requests while push is healthy.
- No topology component commit for pointer-only state.
- Stream gap recovery completes within 500 ms on local daemon reconnect.

### Video

- 1920x1080 at 60 fps on the selected physical or IDD display.
- Glass-to-glass p95 at or below 65 ms and p99 at or below 90 ms.
- No media queue grows without a configured bound.
- Incomplete expired frames are dropped rather than displayed late.
- Cursor overlay remains responsive at control-path latency.
- Thirty minutes of media plus 1000 Hz input produces no unbounded RSS growth or control-state residue.

## Delivery Phases

### Phase 1A: Measurement and Baseline

- Add monotonic stage timestamps and rolling histograms.
- Add deterministic input/IPC/QUIC load harnesses.
- Establish CI and fixed-runner reports.

Gate: the system can report capture, route, queue, transport, receive, inject, UI, and media-stage distributions.

### Phase 1B: Control Hot Path

- Introduce semantic bounded ingress and the single `InputRouter`.
- Add session epoch/ownership and fail-safe release.
- Add cloneable per-peer transport handles and QoS lanes.
- Split realtime and reliable input receive paths.
- Add the dedicated injection actor.
- Replace fixed Windows filter polling with event-driven reads.
- Use real display geometry and relative remote motion.

Gate: wired control meets the control acceptance criteria under concurrent diagnostics and bulk load.

### Phase 1C: State Display and IPC

- Add versioned `UiSnapshot`/`UiDelta` stream and revision resync.
- Split static inventory from dynamic state.
- Add selector-driven frontend stores and RAF latest-value rendering.
- Stop polling when push is healthy.
- Replace one-byte IPC reading and JSON image arrays.

Gate: UI latency, render isolation, resync, and zero-healthy-polling criteria pass.

### Phase 2A: Media Skeleton

- Add `rshare-media` contracts and session negotiation.
- Add bounded packetizer/reassembler and simulated encoded frames.
- Prove media queue/blockage isolation from input.

Gate: media stress adds no more than 5 ms to control p99.

### Phase 2B: Windows 1080p60

- Implement GPU capture and NV12 conversion.
- Implement Media Foundation H.264 hardware encode/decode.
- Add separate media QUIC connection.
- Add native D3D11 Tauri presentation and cursor overlay.

Gate: physical-display 1080p60 meets the video acceptance criteria.

### Phase 2C: IDD and Recovery

- Validate IDD virtual display as a selectable capture source.
- Add adaptive bitrate/resolution and keyframe recovery.
- Cover display removal, lock, sleep/wake, reconnect, and decoder reset.

Gate: the full thirty-minute mixed-load and recovery matrix passes.

## Migration

- Introduce a new peer protocol version and capability handshake.
- Reject older peers with a clear reason before connection registration.
- Keep local UI/CLI compatibility within the same release through the versioned local envelope.
- Land internal actors and measurement behind runtime feature switches where useful, but do not maintain two long-term network protocols.
- Enable media only after Phase 1 control gates pass.

## Deferred Work

- Wi-Fi tuning after the wired baseline is stable.
- macOS and Linux capture/injection parity.
- Multi-display video sessions.
- HEVC/AV1/HDR/4K.
- Audio synchronization and media A/V clocking.
- Public relay and WAN congestion behavior.
- Privileged lock-screen capture policy.

