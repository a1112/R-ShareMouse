# Device event monitor through UI state

## Problem

The Devices page stops its local-control event subscription whenever the UI-state stream is healthy. UI state currently contains pointer position and pressed state, but no local event history or counters. The daemon still captures events, so the page appears frozen. Remote input history also depends on a separate endpoint-event stream.

## Chosen design

The daemon owns one bounded monitor projection containing its local-control snapshot and recent remote endpoint input events. The initial UI-state snapshot includes that projection. A reliable `device_monitor` delta replaces it when input or device diagnostics change, sampled at about 100 ms. The desktop reads the monitor projection from the UI-state store and renders local and remote activity through the existing device-page models. Pointer and gamepad motion retain their existing low-latency UI-state deltas.

The daemon requests remote endpoint diagnostics when a peer connects. Remote diagnostics already enter the endpoint store; the monitor projection selects recent keyboard, mouse, and gamepad events per connected peer. The desktop no longer opens local-controls or endpoint-events subscriptions for normal monitoring. The existing IPC APIs remain available to other clients and as manual diagnostics.

The projection keeps the existing 64-event local history and caps remote history per peer and in total. It publishes only changed projections, limiting serialization and rendering work. A UI-state reconnect receives the current bounded history in its snapshot. If UI state is unavailable, the desktop uses its existing local-controls request as a fallback, with an explicit stale/error state.

## Verification

- Core contract: snapshot and delta apply the monitor projection and preserve bounded data across resync.
- Daemon: local and remote event updates publish a monitor delta; unchanged samples do not publish; peer connection requests diagnostics.
- Frontend: a healthy UI-state stream updates local counts/history and remote event history without separate event subscriptions; snapshots recover after reconnect.
- Runtime: confirm daemon snapshots and deltas change under local input, then build the frontend and run focused tests.
