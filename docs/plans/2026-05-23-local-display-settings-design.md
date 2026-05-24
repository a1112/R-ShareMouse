# Local Display Settings Design

Date: 2026-05-23

## Goal

Add a local display management surface that matches the practical capabilities users expect from Windows Display Settings:

- Show real local monitor state.
- Show monitor size, position, orientation, scale, DPI, refresh rate, primary status, and supported modes.
- Provide monitor screenshots or thumbnails for recognition.
- Identify monitors with temporary on-screen labels.
- Allow safe display setting changes where Windows provides stable APIs.
- Keep a direct entry point to the operating system display settings page.

This is Windows-first. macOS and Linux keep the same public contract but may return read-only state or `Unsupported` for setting changes until platform-specific implementations are designed.

## Current Context

The daemon already owns local runtime truth and exposes it through local-control IPC. `LocalControlDeviceSnapshot` already contains `LocalDisplayState`, and the daemon already enriches layout and capability state from local displays. The Windows platform layer can enumerate monitor rectangles and open `ms-settings:display`, but it does not yet expose monitor names, orientation, scale, screenshot previews, supported modes, or write operations.

The desktop app already follows the correct ownership model: it reads daemon state instead of deriving runtime truth in the UI. The new display page should preserve that model.

## Architecture

Add a platform display-management module under `rshare-platform` and keep the daemon as the authority for display state and display operations.

The platform module exposes a cross-platform trait-like API:

- `query_display_state() -> LocalDisplayState`
- `capture_display(display_id) -> DisplayCapture`
- `identify_displays(request) -> DisplayIdentifyResult`
- `apply_display_settings(request) -> DisplaySettingsResult`
- `open_display_settings()`

Windows implements the full first version. Other platforms return best-effort read state where cheap and `Unsupported` for write/capture/identify operations as needed.

The daemon refreshes display state through this module, stores it in `LocalControlDeviceSnapshot.display`, and updates layout/capability details from the same source. Desktop and CLI clients call daemon IPC commands for display actions.

## Data Model

Extend `LocalDisplayInfo` with stable and user-facing fields:

- `display_id`: stable app identifier for the monitor path.
- `adapter_id` and `target_id`: Windows display path identifiers when available.
- `device_name`: Windows device name such as `\\.\DISPLAY1`.
- `friendly_name`: monitor or adapter name for UI.
- `x`, `y`, `width`, `height`: current desktop rectangle.
- `work_x`, `work_y`, `work_width`, `work_height`: current work area.
- `primary`: true when the display is the primary monitor.
- `orientation`: landscape, portrait, landscape flipped, or portrait flipped.
- `scale_percent`: effective display scale when available.
- `dpi_x`, `dpi_y`: effective DPI.
- `raw_dpi_x`, `raw_dpi_y`: physical DPI where available.
- `refresh_rate_millihz` or `refresh_rate_hz`.
- `bits_per_pixel`.
- `active`: whether the display is active in the desktop topology.
- `modes`: supported display modes, deduplicated for resolution, refresh, and orientation.
- `write_capabilities`: per-monitor flags for resolution, refresh rate, orientation, primary display, position, and scale.

Keep existing fields backward-compatible with serde defaults.

Add request/result types in `rshare-core` for IPC:

- `DisplayCaptureRequest`
- `DisplayCaptureResult`
- `DisplayIdentifyRequest`
- `DisplaySettingsUpdateRequest`
- `DisplaySettingsUpdateResult`
- `DisplayOperationStatus`

Results include `Unsupported`, `PermissionDenied`, `InvalidMode`, `ApplyFailed`, and `RequiresSystemSettings`.

## Windows Implementation

Read state with a layered strategy:

1. Use `EnumDisplayMonitors` and `GetMonitorInfoW` for desktop rectangles, work areas, and primary status.
2. Use `EnumDisplayDevicesW` and `EnumDisplaySettingsW` for device names, current modes, supported modes, orientation, refresh rate, and color depth.
3. Use `QueryDisplayConfig` where available to improve stable monitor identity and path details.
4. Use `GetDpiForMonitor` and `GetScaleFactorForMonitor` for effective DPI and scale.

Write state:

- Resolution, refresh rate, orientation, and color depth use the `DEVMODE` returned by `EnumDisplaySettingsW`, modified and applied through `ChangeDisplaySettingsExW`.
- Primary display and monitor position use `ChangeDisplaySettingsExW` with coordinated `CDS_NORESET` updates followed by a reset/apply call.
- Invalid or unsupported modes are rejected before calling Windows APIs.
- Scaling is read-only in the first implementation. The result should report `RequiresSystemSettings` and include the display settings launcher. Windows provides stable public APIs for reading per-monitor scale, but not a normal supported per-monitor scale write API equivalent to the Settings app.

The existing `open_display_settings()` entry point remains and is exposed from both tray and display page.

## Screenshot And Identify

Screenshot capture is low frequency and user-triggered.

First implementation can use GDI desktop capture against the monitor rectangle because the product only needs a preview thumbnail for recognition and layout. If that path fails or captures protected/blank content, return a clear status and keep the display state usable. Windows Graphics Capture can be evaluated later for higher fidelity and HDR-aware capture.

Identify is implemented as temporary borderless always-on-top windows placed on each monitor, showing a large display number and friendly name. This mirrors Windows Display Settings without changing OS configuration. The overlay auto-closes after a short duration and can be cancelled by a later identify request.

## Desktop UI

Add a local display section in the device surface:

- Topology view with per-monitor rectangles and current primary marker.
- Monitor detail panel with screenshot thumbnail, name, resolution, refresh rate, orientation, scale, DPI, and coordinates.
- Controls for identify, refresh screenshot, set primary, resolution, refresh rate, and orientation.
- Disabled scale control with direct action to open Windows Display Settings when scale cannot be written safely.
- Direct "Open Windows Display Settings" action.

All write actions require confirmation, apply through daemon IPC, and refresh local display state after completion.

## Error Handling

Display operations must not degrade input sharing. Failures are reported as display operation results and recent local diagnostic events, not daemon fatal errors.

Unsupported operations should be explicit. The UI should show that scale write is delegated to Windows settings rather than silently failing.

When an apply operation fails, the daemon refreshes display state to avoid stale UI. If Windows reports that a restart or re-login is needed, surface that in the result message.

## Testing

Core tests:

- serde defaults preserve backward compatibility for `LocalDisplayInfo`.
- display operation request/result contracts round-trip over IPC.
- invalid display setting requests are rejected before platform apply.

Daemon tests:

- display state refresh updates local controls and layout displays from one canonical source.
- unsupported display operation returns a nonfatal result.
- successful operation triggers state refresh.

Windows platform tests:

- pure conversion tests for `DEVMODE` orientation, refresh rate, and mode normalization.
- stable display ID generation is deterministic.
- mode validation rejects unsupported resolution/refresh/orientation combinations.

Desktop tests:

- display view renders state, screenshot placeholder, unsupported scale write state, and operation errors.
- write controls call the display operation command and refresh state.

Manual validation:

- Single monitor.
- Two monitors with different resolution and scale.
- Portrait monitor.
- Setting primary display.
- Changing refresh rate and reverting.
- Opening Windows Display Settings.
- Identify overlays appear on the correct monitors.
- Screenshot preview matches each monitor.

## Non-Goals

- Implementing cross-platform display writes in the first pass.
- Bypassing Windows Settings for per-monitor scale writes through registry hacks or undocumented APIs.
- Streaming remote display video.
- Changing display topology automatically during input sharing.

