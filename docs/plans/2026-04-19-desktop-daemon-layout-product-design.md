# Desktop Auto-Start And Real Layout Product Design

## Summary

**Goal:** make `rshare-desktop` behave like a real product console instead of a shell demo by:

- auto-starting the local daemon only when local IPC is unavailable
- treating the daemon as the owner of tray, service lifecycle, discovery, and persisted layout
- turning the `Layout` page into a real editor of daemon-owned `LayoutGraph`
- auto-merging newly discovered LAN peers into the real layout while preserving remembered placement

This design closes the gap between the current UI and the runtime model already introduced for Alpha-2. The desktop app remains an operator surface. The daemon remains the canonical runtime authority.

## Product Goals

- Opening `rshare-desktop` should normally lead to an operational control console without requiring a manual "start service" click.
- The desktop app must not spawn duplicate daemons when one is already alive.
- The system tray belongs to the daemon, not to the desktop shell.
- The `Layout` page must render and edit the real persisted layout, not a frontend-only projection.
- Newly discovered LAN devices should appear automatically in the layout using a deterministic default placement rule.
- User-adjusted placement must be remembered across desktop restarts and daemon restarts.
- Offline devices must keep remembered topology but disappear from the visible layout canvas.
- Visible devices must remain tightly packed with no offline gaps in the current canvas.

## Non-Goals

- No centralized discovery service.
- No desktop-owned tray implementation.
- No full-blown topology solver in this phase.
- No destructive removal of remembered offline devices from persisted layout.
- No background daemon restart loop for arbitrary errors; auto-start is only for "IPC unavailable".

## Canonical Ownership Model

### Daemon Owns

- discovery
- peer directory
- connect/disconnect state
- runtime `LayoutGraph`
- layout persistence
- local service lifecycle
- tray and background presence

### Desktop Owns

- IPC probe on startup
- one-shot daemon bootstrap when IPC is unavailable
- layout visualization
- layout editing
- operator-triggered commands

The desktop must not become a second source of truth for topology or service state.

## Startup Flow

### Desired Behavior

When `rshare-desktop` launches:

1. the frontend requests `dashboard_state`
2. if the request succeeds, the daemon is considered online
3. if the request fails because the desktop cannot connect to local IPC, the desktop calls `start_service`
4. once `start_service` returns ready, the frontend fetches the real runtime state again
5. if the request fails for a different reason, the desktop shows an actionable error state and does not loop-restart the daemon

### Why This Shape

- it preserves daemon primacy
- it avoids duplicate service processes
- it makes double-click launch feel product-grade
- it prevents masking real daemon failures as "just restart it"

### Tray Boundary

The tray is explicitly a daemon feature. Closing `rshare-desktop` must not imply stopping the daemon or removing tray presence.

## Real Layout Model

### Canonical Source

The daemon-owned `LayoutGraph` is the only real topology source.

The desktop must:

- read `get_layout` during initialization
- render the visible layout from that graph
- write changes back through `set_layout`

The desktop must not synthesize the primary layout topology from `dashboard_state.devices`.

### Discovery Merge Model

Discovery still matters, but only as input into layout reconciliation.

At runtime the desktop combines:

- persisted layout from `get_layout`
- current peer directory from `dashboard_state` / `devices`

The merge logic is:

- if a discovered device already exists in `LayoutGraph.nodes`, preserve its remembered topology
- if a discovered device does not exist in `LayoutGraph.nodes`, append it using the default placement rule
- if a persisted layout node is not currently discovered, retain it in persisted layout but mark it offline for rendering purposes

## Default Placement Rule

Newly discovered devices that are not already part of the layout are appended:

- to the right side of the current visible chain
- in deterministic order
- aligned vertically with the local primary display baseline

The initial rule is intentionally simple:

- find the visible device with the greatest right boundary
- place the new device immediately to its right
- no overlap
- no empty spacing

This creates an understandable first-run experience while still allowing later drag adjustment.

## Remembered Placement Semantics

### Persisted Memory

Once a device has been inserted into the layout and saved:

- its node remains in persisted layout even when the device goes offline
- its historical adjacency and coordinates remain remembered
- daemon restart and desktop restart must restore that remembered topology

### Visible Canvas Semantics

Offline devices are not shown on the `Layout` canvas by default.

That means the product has two layers:

- **real topology layer:** full persisted layout, including offline remembered devices
- **visible canvas layer:** only online/discovered devices plus the local device

## Tight-Packed Visible Rendering

### Rule

Visible devices must remain mutually adjacent with no offline holes.

If a remembered device goes offline:

- its persisted node is retained
- it is hidden from the current canvas
- the visible online subset is reprojected into a compact layout with monitors staying edge-adjacent

### Important Constraint

This compact rendering is a display projection, not a destructive rewrite of persisted topology.

Persisted memory remains intact. The tight packing only affects how the online subset is presented and edited in the current desktop session.

## Reappearance Rule

When a remembered device comes back online:

- it should re-enter the visible topology according to its persisted relationship
- the online projection must be recomputed from the persisted graph
- discovery order must not override previously remembered placement

This preserves the product promise that layout is remembered by position, not by "who was seen first today".

## Layout Editing Rules

### User Interaction

Dragging in the `Layout` page modifies the real working `LayoutGraph`.

After drag completion:

- the desktop computes the updated graph
- the desktop calls `set_layout`
- the daemon persists the graph immediately

### Save Failure

If `set_layout` fails:

- the UI must not claim success
- the edited state should remain visible locally as unsaved work
- the user should see a clear "save failed / not persisted" signal

## Persistence Rules

The daemon must persist:

- stable local `device-id`
- `LayoutGraph`

The daemon startup path must:

- load the persisted layout if it exists
- canonicalize it to the current persisted local `device-id`
- fall back to a local-only layout if no persisted layout exists

## Failure Handling

### Desktop Startup

- auto-start daemon only on IPC unavailable
- do not auto-restart for arbitrary runtime errors

### Layout Read Failure

- if status succeeds but layout read fails, the product must show "service online, layout unavailable"
- device discovery should still work independently

### Layout Merge Failure

- do not drop existing remembered nodes
- log and surface merge failure
- keep the last known good visible projection

## Acceptance Criteria

### Startup

- opening `rshare-desktop` with daemon already online does not spawn a second daemon
- opening `rshare-desktop` with IPC unavailable auto-starts the daemon and lands in a usable state

### Layout

- local device is always present
- newly discovered LAN devices are automatically added to real layout
- new devices default to right-side append
- dragging and saving persists placement
- desktop restart preserves placement
- daemon restart preserves placement

### Offline Memory

- offline devices disappear from the visible layout
- offline devices remain in persisted layout memory
- visible devices stay tightly packed
- returning devices recover their remembered relationship

### Boundary

- tray behavior remains daemon-owned
- closing the desktop window does not terminate daemon ownership semantics

## Code Areas

- `apps/rshare-desktop/src-tauri/src/main.rs`
  - auto-start gating
  - richer layout-oriented Tauri commands if needed
- `crates/rshare-core/src/daemon_client.rs`
  - IPC-unavailable detection helpers
- `apps/rshare-daemon/src/main.rs`
  - layout load/save lifecycle
- `crates/rshare-core/src/layout.rs`
  - helpers for merge, projection, and remembered topology
- `other/figma-ui/src/app/App.tsx`
  - startup orchestration
  - layout initialization
  - layout save path
- `other/figma-ui/src/app/desktop-model.mjs`
  - map real layout + discovery status into visible layout view

## Design Decisions

- Auto-start only when IPC is unavailable.
- Tray belongs to daemon.
- Persisted topology and visible topology are intentionally distinct layers.
- Offline devices keep memory but are hidden from the canvas.
- Visible devices are always reprojected as a tightly packed online subset.

## Current Implementation Status

As of 2026-04-20, the product design is implemented for the desktop control path:

- `rshare-desktop` probes daemon IPC and only auto-starts daemon on IPC-unavailable failures.
- daemon-owned `LayoutGraph` is persisted to state and loaded on daemon startup.
- invalid persisted layout files no longer brick daemon startup; the daemon falls back to local-only layout and preserves the invalid file for inspection.
- `dashboard_state` returns remembered layout, online-only visible layout, layout errors, and whether daemon was auto-started.
- newly discovered devices are merged into remembered layout on the right and saved back to daemon.
- offline remembered nodes stay persisted but are omitted from the visible layout.
- the visible layout is display-only; drag save-back applies visual deltas to remembered coordinates instead of persisting compact projection coordinates.
- frontend layout rendering consumes daemon `visible_layout` rather than synthesizing all discovered devices into fake monitors.

Known remaining validation gap:

- This has automated coverage and build verification, but still needs packaged desktop and real LAN multi-machine acceptance testing.
