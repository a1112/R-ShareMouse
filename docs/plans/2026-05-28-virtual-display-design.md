# Virtual Display Integration Design

## Goal

Add an internal virtual display creation path to R-ShareMouse that can create a real OS-visible display and let users change that display through the system display settings surface.

## Current Context

`E:\project\R-VD` is useful as a control-plane reference, but it does not create an operating system display. Its README states that `screen create` is backed by a fake adapter and does not register a real OS display. The reusable ideas are the manager boundary, display mode model, lifecycle events, and snapshot shape.

R-ShareMouse already has a daemon-owned display control path:

- Frontend commands call `capture_display`, `identify_displays`, `update_display_settings`, and `open_display_settings`.
- The Tauri shell forwards those commands to the daemon when available.
- The daemon owns the runtime snapshot and calls `rshare_platform::display`.
- Windows display enumeration and mode writes already use real system APIs, including `ChangeDisplaySettingsExW`.

This means virtual displays should enter the same display state pipeline as physical displays. A fake in-app screen is not enough, because the requirement is that the display appears in the system display settings and can be changed there.

## Architecture

Use a split architecture:

```text
Desktop UI
  -> Tauri command bridge
  -> daemon IPC
  -> VirtualDisplayManager
  -> rshare-platform virtual_display backend
  -> Windows IDD / IddCx control path
  -> OS display settings and existing display enumeration
```

The Rust control plane should be implemented first and should mirror the useful parts of R-VD:

- create/remove/list virtual displays,
- validate display modes,
- track requested lifecycle state,
- return structured status and errors,
- refresh ordinary display enumeration after changes.

The Windows backend must be the layer that makes the display real. On Windows, that means a true IDD/IddCx driver or a control device for an installed IDD driver. The Rust side should not pretend that a virtual display exists unless the platform backend reports that creation was accepted or the subsequent OS display enumeration observes it.

## Component Boundaries

### Core IPC Types

Add serializable request/response types in `rshare-core`:

- `VirtualDisplayCreateRequest`
- `VirtualDisplayRemoveRequest`
- `VirtualDisplaySnapshot`
- `VirtualDisplayStatus`
- `VirtualDisplayOperationResult`

Add daemon IPC variants:

- `ListVirtualDisplays`
- `CreateVirtualDisplay(VirtualDisplayCreateRequest)`
- `RemoveVirtualDisplay(VirtualDisplayRemoveRequest)`

These types are platform-neutral and should not mention IDD directly.

### Daemon Manager

Add a daemon-owned manager that stores requested virtual displays and delegates creation/removal to `rshare_platform::virtual_display`.

Responsibilities:

- validate request dimensions and refresh rate,
- assign stable ids,
- prevent duplicate active ids,
- keep the last platform status,
- refresh local controls/display state after successful platform operations,
- emit a local-control display event if display topology changed.

### Platform Backend

Add `crates/rshare-platform/src/virtual_display.rs`.

Initial behavior:

- Windows exposes a backend boundary with a clear `DriverUnavailable` result if no IDD control path is present.
- Non-Windows returns `Unsupported`.
- The API shape is real and testable, so the later IDD control implementation can replace the unavailable branch without changing daemon/frontend contracts.

Later Windows IDD work:

- add `drivers/windows/rshare-vdisplay`,
- expose a control device or service API,
- accept create/remove/mode requests,
- report monitor hotplug to IddCx,
- rely on Windows display settings for user-visible mode changes.

### UI

Extend the display settings page with a virtual display control panel:

- list virtual displays and their status,
- create a virtual display from width/height/refresh input,
- remove a virtual display,
- show driver unavailable/unsupported messages,
- keep the existing system settings button as the way to change OS-visible display settings.

The UI should not draw a virtual monitor unless it is present in the daemon's display snapshot or listed as pending/unavailable in the virtual display manager.

## Error Handling

Operation statuses should be explicit:

- `Created`
- `Removed`
- `AlreadyExists`
- `InvalidMode`
- `DriverUnavailable`
- `Unsupported`
- `Failed`

The user-facing copy should distinguish "control plane accepted request" from "OS display exists". The completion criterion for the full feature is OS enumeration showing the virtual display after create.

## Testing

Use TDD for each layer:

- core serialization and contract tests,
- daemon manager tests for create/remove/list and platform status mapping,
- platform tests for invalid modes and unsupported/unavailable results,
- frontend model tests for the virtual display panel state.

Real IDD validation must be manual or integration-level on Windows:

- install/start the IDD driver,
- create a virtual display,
- confirm `query_display_state()` reports it,
- confirm Windows system display settings shows it,
- change mode in system settings and confirm daemon snapshot refreshes.

## Non-Goals For The First Implementation Slice

- No fake display injection into `LocalDisplayState`.
- No claim that a virtual monitor exists without OS/platform evidence.
- No frame streaming or encoding path.
- No driver signing automation in the first slice.
