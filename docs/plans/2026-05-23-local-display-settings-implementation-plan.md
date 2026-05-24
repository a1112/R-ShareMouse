# Local Display Settings Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Windows-first local display management surface with real monitor state, screenshots, identify overlays, safe display setting writes, and a direct Windows Display Settings launcher.

**Architecture:** Keep the daemon as the source of truth. Add display operation contracts in `rshare-core`, platform implementations in `rshare-platform`, daemon IPC handlers in `apps/rshare-daemon`, and desktop commands/UI in `apps/rshare-desktop`. Windows implements full read/write where supported; scale writes return `RequiresSystemSettings`.

**Tech Stack:** Rust workspace, serde IPC contracts, Windows Win32 APIs through the `windows` crate, Tauri commands, vanilla desktop UI JavaScript/CSS tests with `node:test`.

---

### Task 1: Core Display Contracts

**Files:**
- Modify: `crates/rshare-core/src/local_controls.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Modify: `crates/rshare-core/tests/ipc_contract.rs`
- Test: `crates/rshare-core/tests/display_settings_contract.rs`

**Step 1: Write the failing contract tests**

Create `crates/rshare-core/tests/display_settings_contract.rs`:

```rust
use rshare_core::{
    DisplayCaptureRequest, DisplayIdentifyRequest, DisplayOperationStatus,
    DisplayOrientation, DisplaySettingsUpdateRequest, LocalDisplayInfo,
};

#[test]
fn local_display_info_deserializes_older_snapshots() {
    let json = r#"{
        "display_id":"primary",
        "x":0,
        "y":0,
        "width":1920,
        "height":1080,
        "primary":true
    }"#;

    let display: LocalDisplayInfo = serde_json::from_str(json).unwrap();

    assert_eq!(display.display_id, "primary");
    assert_eq!(display.width, 1920);
    assert_eq!(display.orientation, DisplayOrientation::Landscape);
    assert_eq!(display.write_capabilities.scale, false);
}

#[test]
fn display_update_request_round_trips() {
    let request = DisplaySettingsUpdateRequest {
        display_id: "display-1".to_string(),
        width: Some(2560),
        height: Some(1440),
        refresh_rate_millihz: Some(144_000),
        orientation: Some(DisplayOrientation::Landscape),
        primary: Some(true),
        x: Some(0),
        y: Some(0),
        scale_percent: Some(150),
    };

    let json = serde_json::to_string(&request).unwrap();
    let decoded: DisplaySettingsUpdateRequest = serde_json::from_str(&json).unwrap();

    assert_eq!(decoded.display_id, "display-1");
    assert_eq!(decoded.scale_percent, Some(150));
}

#[test]
fn display_operation_status_serializes_stable_names() {
    assert_eq!(
        serde_json::to_string(&DisplayOperationStatus::RequiresSystemSettings).unwrap(),
        r#""RequiresSystemSettings""#
    );
}

#[test]
fn capture_and_identify_requests_round_trip() {
    let capture = DisplayCaptureRequest {
        display_id: "primary".to_string(),
        max_width: Some(640),
    };
    let identify = DisplayIdentifyRequest {
        duration_ms: Some(2500),
    };

    serde_json::from_str::<DisplayCaptureRequest>(&serde_json::to_string(&capture).unwrap())
        .unwrap();
    serde_json::from_str::<DisplayIdentifyRequest>(&serde_json::to_string(&identify).unwrap())
        .unwrap();
}
```

**Step 2: Run the failing test**

Run:

```bash
cargo test -p rshare-core display_settings_contract
```

Expected: fails because the display request/result types and extended fields do not exist.

**Step 3: Add the core types**

In `crates/rshare-core/src/local_controls.rs`, add:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayOrientation {
    #[default]
    Landscape,
    Portrait,
    LandscapeFlipped,
    PortraitFlipped,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayWriteCapabilities {
    #[serde(default)]
    pub resolution: bool,
    #[serde(default)]
    pub refresh_rate: bool,
    #[serde(default)]
    pub orientation: bool,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub position: bool,
    #[serde(default)]
    pub scale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayModeInfo {
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub refresh_rate_millihz: Option<u32>,
    #[serde(default)]
    pub orientation: DisplayOrientation,
    #[serde(default)]
    pub bits_per_pixel: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayOperationStatus {
    Success,
    Unsupported,
    PermissionDenied,
    InvalidDisplay,
    InvalidMode,
    RequiresSystemSettings,
    ApplyFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayCaptureRequest {
    pub display_id: String,
    #[serde(default)]
    pub max_width: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayCaptureResult {
    pub status: DisplayOperationStatus,
    #[serde(default)]
    pub display_id: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub bytes: Vec<u8>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayIdentifyRequest {
    #[serde(default)]
    pub duration_ms: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayIdentifyResult {
    pub status: DisplayOperationStatus,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplaySettingsUpdateRequest {
    pub display_id: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub refresh_rate_millihz: Option<u32>,
    #[serde(default)]
    pub orientation: Option<DisplayOrientation>,
    #[serde(default)]
    pub primary: Option<bool>,
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
    #[serde(default)]
    pub scale_percent: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplaySettingsUpdateResult {
    pub status: DisplayOperationStatus,
    #[serde(default)]
    pub message: Option<String>,
}
```

Extend `LocalDisplayInfo` with serde-default fields from the design document.

In `crates/rshare-core/src/lib.rs`, re-export the new types.

**Step 4: Run the test**

Run:

```bash
cargo test -p rshare-core display_settings_contract
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/rshare-core/src/local_controls.rs crates/rshare-core/src/lib.rs crates/rshare-core/tests/display_settings_contract.rs
git commit -m "Model local display operations"
```

### Task 2: IPC Commands For Display Operations

**Files:**
- Modify: `crates/rshare-core/src/ipc.rs`
- Modify: `crates/rshare-core/src/daemon_client.rs`
- Modify: `crates/rshare-core/tests/ipc_contract.rs`

**Step 1: Write failing IPC tests**

Add tests to `crates/rshare-core/tests/ipc_contract.rs` that serialize and deserialize:

```rust
use rshare_core::{
    DaemonRequest, DaemonResponse, DisplayCaptureRequest, DisplayOperationStatus,
    DisplaySettingsUpdateRequest, DisplaySettingsUpdateResult,
};

#[test]
fn display_capture_request_round_trips_over_ipc() {
    let request = DaemonRequest::CaptureDisplay(DisplayCaptureRequest {
        display_id: "primary".to_string(),
        max_width: Some(480),
    });

    let json = serde_json::to_string(&request).unwrap();
    let decoded: DaemonRequest = serde_json::from_str(&json).unwrap();

    assert!(matches!(decoded, DaemonRequest::CaptureDisplay(_)));
}

#[test]
fn display_settings_response_round_trips_over_ipc() {
    let response = DaemonResponse::DisplaySettingsUpdated(DisplaySettingsUpdateResult {
        status: DisplayOperationStatus::RequiresSystemSettings,
        message: Some("Open Windows Display Settings to change scale.".to_string()),
    });

    let json = serde_json::to_string(&response).unwrap();
    let decoded: DaemonResponse = serde_json::from_str(&json).unwrap();

    assert!(matches!(decoded, DaemonResponse::DisplaySettingsUpdated(_)));
}
```

**Step 2: Run the failing test**

Run:

```bash
cargo test -p rshare-core ipc_contract
```

Expected: fails because the IPC variants do not exist.

**Step 3: Add IPC variants and client helpers**

In `crates/rshare-core/src/ipc.rs`, add daemon request variants:

- `CaptureDisplay(DisplayCaptureRequest)`
- `IdentifyDisplays(DisplayIdentifyRequest)`
- `UpdateDisplaySettings(DisplaySettingsUpdateRequest)`
- `OpenDisplaySettings`

Add response variants:

- `DisplayCapture(DisplayCaptureResult)`
- `DisplayIdentify(DisplayIdentifyResult)`
- `DisplaySettingsUpdated(DisplaySettingsUpdateResult)`

In `crates/rshare-core/src/daemon_client.rs`, add helpers:

- `request_display_capture(request)`
- `request_identify_displays(request)`
- `request_update_display_settings(request)`
- `request_open_display_settings()`

Follow the existing helper style for request/response matching.

**Step 4: Run IPC tests**

Run:

```bash
cargo test -p rshare-core ipc_contract display_settings_contract
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/rshare-core/src/ipc.rs crates/rshare-core/src/daemon_client.rs crates/rshare-core/tests/ipc_contract.rs
git commit -m "Add display operation IPC"
```

### Task 3: Platform Display Module And Fallbacks

**Files:**
- Create: `crates/rshare-platform/src/display.rs`
- Modify: `crates/rshare-platform/src/lib.rs`
- Test: `crates/rshare-platform/src/display.rs`

**Step 1: Write failing fallback tests**

In `crates/rshare-platform/src/display.rs`, include unit tests for pure helpers:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rshare_core::DisplayOperationStatus;

    #[test]
    fn unsupported_capture_result_names_display() {
        let result = unsupported_capture("display-1", "not implemented");
        assert_eq!(result.status, DisplayOperationStatus::Unsupported);
        assert_eq!(result.display_id, "display-1");
        assert!(result.bytes.is_empty());
    }

    #[test]
    fn scale_update_requires_system_settings() {
        let result = scale_requires_system_settings();
        assert_eq!(result.status, DisplayOperationStatus::RequiresSystemSettings);
    }
}
```

**Step 2: Run the failing test**

Run:

```bash
cargo test -p rshare-platform display
```

Expected: fails because `display.rs` does not exist.

**Step 3: Add the fallback module**

Create `crates/rshare-platform/src/display.rs` with cross-platform public functions:

```rust
use anyhow::Result;
use rshare_core::{
    DisplayCaptureRequest, DisplayCaptureResult, DisplayIdentifyRequest, DisplayIdentifyResult,
    DisplayOperationStatus, DisplaySettingsUpdateRequest, DisplaySettingsUpdateResult,
    LocalDisplayState,
};

pub fn query_display_state() -> Result<LocalDisplayState> {
    platform_query_display_state()
}

pub fn capture_display(request: &DisplayCaptureRequest) -> Result<DisplayCaptureResult> {
    platform_capture_display(request)
}

pub fn identify_displays(request: &DisplayIdentifyRequest) -> Result<DisplayIdentifyResult> {
    platform_identify_displays(request)
}

pub fn update_display_settings(
    request: &DisplaySettingsUpdateRequest,
) -> Result<DisplaySettingsUpdateResult> {
    platform_update_display_settings(request)
}

pub fn open_display_settings() -> Result<()> {
    platform_open_display_settings()
}
```

Use `#[cfg(windows)]` to call Windows functions to be added later. Non-Windows fallback returns `Unsupported` for capture, identify, and update. Move the existing OS-specific `open_display_settings` functions into this module or call them from here.

Modify `crates/rshare-platform/src/lib.rs` to expose `pub mod display;` and remove the old inline `display` modules after behavior is preserved.

**Step 4: Run platform tests**

Run:

```bash
cargo test -p rshare-platform display
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/rshare-platform/src/display.rs crates/rshare-platform/src/lib.rs
git commit -m "Introduce platform display module"
```

### Task 4: Windows Display Enumeration

**Files:**
- Modify: `crates/rshare-platform/Cargo.toml`
- Modify: `crates/rshare-platform/src/windows.rs`
- Modify: `crates/rshare-platform/src/display.rs`
- Modify: `apps/rshare-daemon/src/main.rs`

**Step 1: Write pure conversion tests**

In the Windows module tests, add tests for:

```rust
#[test]
fn windows_orientation_mapping_is_stable() {
    assert_eq!(display_orientation_from_devmode(0), DisplayOrientation::Landscape);
    assert_eq!(display_orientation_from_devmode(1), DisplayOrientation::Portrait);
    assert_eq!(display_orientation_from_devmode(2), DisplayOrientation::LandscapeFlipped);
    assert_eq!(display_orientation_from_devmode(3), DisplayOrientation::PortraitFlipped);
}

#[test]
fn refresh_rate_converts_to_millihz() {
    assert_eq!(refresh_rate_to_millihz(60), Some(60_000));
    assert_eq!(refresh_rate_to_millihz(0), None);
}
```

**Step 2: Run the failing test**

Run:

```bash
cargo test -p rshare-platform windows_orientation_mapping_is_stable refresh_rate_converts_to_millihz
```

Expected: fails because helper functions do not exist.

**Step 3: Implement Windows state query**

Add required `windows` crate features in `crates/rshare-platform/Cargo.toml` if needed:

- `Win32_Graphics_Gdi`
- `Win32_UI_HiDpi`
- `Win32_System_LibraryLoader`

In `crates/rshare-platform/src/windows.rs`, add Windows display state functions that:

- enumerate monitors with `EnumDisplayMonitors`;
- read `MONITORINFOEXW`;
- enumerate devices with `EnumDisplayDevicesW`;
- read current and supported modes with `EnumDisplaySettingsW`;
- read DPI/scale with `GetDpiForMonitor` and `GetScaleFactorForMonitor`;
- build `LocalDisplayState` and extended `LocalDisplayInfo`.

Keep the existing `get_all_screens()` behavior by deriving it from the richer state or leaving it as a compatibility wrapper.

In `apps/rshare-daemon/src/main.rs`, replace direct `rshare_platform::windows::get_all_screens()` usage with `rshare_platform::display::query_display_state()` where possible. Preserve fallback behavior for errors.

**Step 4: Run tests**

Run:

```bash
cargo test -p rshare-platform display
cargo test -p rshare-daemon display
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/rshare-platform/Cargo.toml crates/rshare-platform/src/windows.rs crates/rshare-platform/src/display.rs apps/rshare-daemon/src/main.rs
git commit -m "Read detailed Windows display state"
```

### Task 5: Display Setting Writes

**Files:**
- Modify: `crates/rshare-platform/src/windows.rs`
- Modify: `crates/rshare-platform/src/display.rs`
- Test: `crates/rshare-platform/src/windows.rs`

**Step 1: Write validation tests**

Add pure tests:

```rust
#[test]
fn update_with_only_scale_requires_settings() {
    let request = DisplaySettingsUpdateRequest {
        display_id: "primary".to_string(),
        scale_percent: Some(150),
        width: None,
        height: None,
        refresh_rate_millihz: None,
        orientation: None,
        primary: None,
        x: None,
        y: None,
    };

    let result = validate_display_update_for_test(&request, &[]);
    assert_eq!(result.status, DisplayOperationStatus::RequiresSystemSettings);
}

#[test]
fn unsupported_mode_is_rejected_before_apply() {
    let request = DisplaySettingsUpdateRequest {
        display_id: "primary".to_string(),
        width: Some(9999),
        height: Some(9999),
        refresh_rate_millihz: Some(60_000),
        orientation: Some(DisplayOrientation::Landscape),
        primary: None,
        x: None,
        y: None,
        scale_percent: None,
    };

    let modes = vec![DisplayModeInfo {
        width: 1920,
        height: 1080,
        refresh_rate_millihz: Some(60_000),
        orientation: DisplayOrientation::Landscape,
        bits_per_pixel: Some(32),
    }];

    let result = validate_display_update_for_test(&request, &modes);
    assert_eq!(result.status, DisplayOperationStatus::InvalidMode);
}
```

**Step 2: Run the failing tests**

Run:

```bash
cargo test -p rshare-platform display_update
```

Expected: fails because validation and write functions do not exist.

**Step 3: Implement validation and apply**

Implement:

- mode validation against `LocalDisplayInfo.modes`;
- scale-only or scale-included update returns `RequiresSystemSettings`;
- resolution/refresh/orientation apply through `ChangeDisplaySettingsExW`;
- primary/position apply through coordinated `ChangeDisplaySettingsExW` calls.

Always return `DisplaySettingsUpdateResult` instead of panicking. Include raw Windows result codes in the message for failed applies.

**Step 4: Run tests**

Run:

```bash
cargo test -p rshare-platform display_update
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/rshare-platform/src/windows.rs crates/rshare-platform/src/display.rs
git commit -m "Apply safe Windows display settings"
```

### Task 6: Screenshot And Identify Operations

**Files:**
- Modify: `crates/rshare-platform/Cargo.toml`
- Modify: `crates/rshare-platform/src/windows.rs`
- Modify: `crates/rshare-platform/src/display.rs`

**Step 1: Write pure tests**

Add tests for screenshot scaling math and identify duration clamping:

```rust
#[test]
fn thumbnail_size_preserves_aspect_ratio() {
    assert_eq!(fit_thumbnail_size(3840, 2160, 640), (640, 360));
    assert_eq!(fit_thumbnail_size(1080, 1920, 480), (270, 480));
}

#[test]
fn identify_duration_is_clamped() {
    assert_eq!(clamp_identify_duration_ms(None), 2500);
    assert_eq!(clamp_identify_duration_ms(Some(100)), 500);
    assert_eq!(clamp_identify_duration_ms(Some(30_000)), 10_000);
}
```

**Step 2: Run failing tests**

Run:

```bash
cargo test -p rshare-platform thumbnail_size identify_duration
```

Expected: fails because helper functions do not exist.

**Step 3: Implement capture and identify**

For capture:

- find display by `display_id`;
- capture the monitor rectangle with GDI BitBlt into a bitmap;
- encode PNG bytes if a PNG encoder dependency is already acceptable, otherwise return BMP bytes with `image/bmp` for first pass;
- resize to `max_width` when provided.

For identify:

- create temporary borderless topmost windows on each monitor;
- show display index and friendly name;
- auto-close after clamped duration;
- return `Success` once overlays are created.

If capture encoding requires a new dependency, use the smallest established Rust image crate and keep it in `rshare-platform` only.

**Step 4: Run tests**

Run:

```bash
cargo test -p rshare-platform thumbnail_size identify_duration
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/rshare-platform/Cargo.toml crates/rshare-platform/src/windows.rs crates/rshare-platform/src/display.rs
git commit -m "Capture and identify local displays"
```

### Task 7: Daemon Display Operation Handlers

**Files:**
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `crates/rshare-core/tests/ipc_contract.rs`

**Step 1: Write daemon handler tests**

Add tests near existing daemon request handling tests:

```rust
#[tokio::test]
async fn display_scale_update_returns_nonfatal_result() {
    let request = DisplaySettingsUpdateRequest {
        display_id: "primary".to_string(),
        scale_percent: Some(150),
        width: None,
        height: None,
        refresh_rate_millihz: None,
        orientation: None,
        primary: None,
        x: None,
        y: None,
    };

    let result = handle_display_settings_update_for_test(request).await;

    assert!(matches!(
        result.status,
        DisplayOperationStatus::RequiresSystemSettings | DisplayOperationStatus::Unsupported
    ));
}
```

Adapt helper names to the existing test style in `apps/rshare-daemon/src/main.rs`.

**Step 2: Run failing daemon tests**

Run:

```bash
cargo test -p rshare-daemon display
```

Expected: fails because daemon handlers do not exist.

**Step 3: Add daemon request handling**

Handle IPC requests:

- `CaptureDisplay` -> `rshare_platform::display::capture_display`
- `IdentifyDisplays` -> `rshare_platform::display::identify_displays`
- `UpdateDisplaySettings` -> apply, refresh local display state, update layout displays, emit display event
- `OpenDisplaySettings` -> platform launcher

Do not fail the daemon on operation errors. Convert errors to operation result responses.

**Step 4: Run daemon tests**

Run:

```bash
cargo test -p rshare-daemon display
```

Expected: passes.

**Step 5: Commit**

```bash
git add apps/rshare-daemon/src/main.rs
git commit -m "Handle local display operations in daemon"
```

### Task 8: Desktop Commands And UI

**Files:**
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`
- Modify: `apps/rshare-desktop/ui/app.js`
- Modify: `apps/rshare-desktop/ui/index.html`
- Modify: `apps/rshare-desktop/ui/styles.css`
- Create: `apps/rshare-desktop/ui/display.test.mjs`
- Create: `apps/rshare-desktop/ui/display.mjs`

**Step 1: Write UI builder tests**

Create `apps/rshare-desktop/ui/display.test.mjs`:

```js
import test from 'node:test';
import assert from 'node:assert/strict';

import { buildDisplayRows, buildScaleControlState } from './display.mjs';

test('buildDisplayRows formats monitor details', () => {
  const rows = buildDisplayRows({
    displays: [
      {
        display_id: 'primary',
        friendly_name: 'Studio Display',
        width: 3840,
        height: 2160,
        refresh_rate_millihz: 60000,
        scale_percent: 150,
        orientation: 'Landscape',
        primary: true,
      },
    ],
  });

  assert.equal(rows[0].title, 'Studio Display');
  assert.equal(rows[0].resolution, '3840 x 2160');
  assert.equal(rows[0].refreshRate, '60 Hz');
  assert.equal(rows[0].scale, '150%');
});

test('scale write directs user to system settings when unsupported', () => {
  const state = buildScaleControlState({
    write_capabilities: { scale: false },
    scale_percent: 125,
  });

  assert.equal(state.enabled, false);
  assert.equal(state.action, 'open-system-settings');
});
```

**Step 2: Run failing UI tests**

Run:

```bash
node --test apps/rshare-desktop/ui/display.test.mjs
```

Expected: fails because `display.mjs` does not exist.

**Step 3: Add desktop Tauri commands**

In `apps/rshare-desktop/src-tauri/src/main.rs`, add commands:

- `capture_display(display_id, max_width)`
- `identify_displays(duration_ms)`
- `update_display_settings(request)`
- `open_display_settings()`

These call daemon client helpers. If daemon is offline, `open_display_settings()` may call `rshare_platform::display::open_display_settings()` directly as a fallback.

Register commands in the Tauri `invoke_handler`.

**Step 4: Add UI builders and render surface**

Create `apps/rshare-desktop/ui/display.mjs` with formatting/build helpers.

Update `apps/rshare-desktop/ui/app.js` to:

- load local controls state or reuse streamed state when available;
- render the display section;
- call identify, capture, update, and open settings commands;
- show operation status messages.

Update `index.html` and `styles.css` with a display section that uses compact operational UI. Keep cards only for individual monitors.

**Step 5: Run UI tests**

Run:

```bash
node --test apps/rshare-desktop/ui/display.test.mjs apps/rshare-desktop/ui/layout.test.mjs
```

Expected: passes.

**Step 6: Commit**

```bash
git add apps/rshare-desktop/src-tauri/src/main.rs apps/rshare-desktop/ui/app.js apps/rshare-desktop/ui/index.html apps/rshare-desktop/ui/styles.css apps/rshare-desktop/ui/display.mjs apps/rshare-desktop/ui/display.test.mjs
git commit -m "Show and manage local displays in desktop"
```

### Task 9: End-To-End Verification

**Files:**
- Modify only if verification finds issues.

**Step 1: Run core and platform tests**

Run:

```bash
cargo test -p rshare-core display_settings_contract ipc_contract
cargo test -p rshare-platform display
```

Expected: all selected tests pass.

**Step 2: Run daemon and desktop tests**

Run:

```bash
cargo test -p rshare-daemon display
node --test apps/rshare-desktop/ui/display.test.mjs apps/rshare-desktop/ui/layout.test.mjs
```

Expected: all selected tests pass.

**Step 3: Run workspace test if time allows**

Run:

```bash
cargo test --workspace
```

Expected: all workspace tests pass. If Windows-only APIs break non-Windows builds, gate imports and functions with `#[cfg(windows)]`.

**Step 4: Manual Windows validation**

Run:

```bash
cargo run -p rshare-daemon
cargo run -p rshare-desktop
```

Validate:

- local display list matches Windows Settings;
- monitor thumbnails match the correct physical display;
- identify overlays appear on every active monitor;
- Windows Display Settings opens from tray and display page;
- scale write shows `RequiresSystemSettings`;
- resolution/orientation/refresh changes reject unsupported modes;
- a safe supported mode can be applied and reverted.

**Step 5: Commit any fixes**

```bash
git add <fixed-files>
git commit -m "Fix display settings validation issues"
```

