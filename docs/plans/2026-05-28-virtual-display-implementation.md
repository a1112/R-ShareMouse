# Virtual Display Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build R-ShareMouse's internal virtual display creation control path, using R-VD as a control-plane reference, and prepare the Windows IDD backend needed for OS-visible displays.

**Architecture:** Add platform-neutral virtual display IPC contracts in `rshare-core`, a daemon-owned manager that delegates to `rshare-platform`, and a frontend display settings panel. The first implementation must not fake OS displays; Windows should return a truthful driver-unavailable result until the IDD control path exists.

**Tech Stack:** Rust, serde IPC, daemon tests, Tauri command bridge, React/TypeScript frontend, Node `node:test`, Windows IDD/IddCx backend boundary.

---

### Task 1: Core Virtual Display IPC Contract

**Files:**
- Modify: `crates/rshare-core/src/local_controls.rs`
- Modify: `crates/rshare-core/src/ipc.rs`
- Modify: `crates/rshare-core/src/daemon_client.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Test: `crates/rshare-core/tests/ipc_contract.rs`

**Step 1: Write the failing contract test**

Add a test that serializes and deserializes:

```rust
let request = DaemonRequest::CreateVirtualDisplay(VirtualDisplayCreateRequest {
    id: Some("vd-1".to_string()),
    width: 1920,
    height: 1080,
    refresh_rate_millihz: Some(60_000),
    name: Some("R-ShareMouse Virtual Display".to_string()),
});
```

Assert that the decoded request preserves width, height, refresh, id, and name. Add response coverage for `DaemonResponse::VirtualDisplays(Vec<VirtualDisplaySnapshot>)` and `DaemonResponse::VirtualDisplayOperation(VirtualDisplayOperationResult)`.

**Step 2: Run test to verify it fails**

Run:

```powershell
cargo test -p rshare-core virtual_display --test ipc_contract
```

Expected: FAIL because the request/response types and enum variants do not exist.

**Step 3: Implement minimal core types**

Add:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VirtualDisplayStatus {
    Pending,
    Active,
    Removed,
    DriverUnavailable,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VirtualDisplayOperationStatus {
    Created,
    Removed,
    AlreadyExists,
    InvalidMode,
    DriverUnavailable,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VirtualDisplayCreateRequest {
    #[serde(default)]
    pub id: Option<String>,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub refresh_rate_millihz: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VirtualDisplayRemoveRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VirtualDisplaySnapshot {
    pub id: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub refresh_rate_millihz: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
    pub status: VirtualDisplayStatus,
    #[serde(default)]
    pub display_id: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VirtualDisplayOperationResult {
    pub status: VirtualDisplayOperationStatus,
    #[serde(default)]
    pub display: Option<VirtualDisplaySnapshot>,
    #[serde(default)]
    pub message: Option<String>,
}
```

Wire IPC variants:

```rust
ListVirtualDisplays,
CreateVirtualDisplay(VirtualDisplayCreateRequest),
RemoveVirtualDisplay(VirtualDisplayRemoveRequest),
VirtualDisplays(Vec<VirtualDisplaySnapshot>),
VirtualDisplayOperation(VirtualDisplayOperationResult),
```

Add daemon client helpers for list/create/remove.

**Step 4: Run test to verify it passes**

Run:

```powershell
cargo test -p rshare-core virtual_display --test ipc_contract
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add crates/rshare-core/src/local_controls.rs crates/rshare-core/src/ipc.rs crates/rshare-core/src/daemon_client.rs crates/rshare-core/src/lib.rs crates/rshare-core/tests/ipc_contract.rs
git commit -m "feat: add virtual display ipc contract"
```

### Task 2: Platform Virtual Display Backend Boundary

**Files:**
- Create: `crates/rshare-platform/src/virtual_display.rs`
- Modify: `crates/rshare-platform/src/lib.rs`
- Test: `crates/rshare-platform/src/virtual_display.rs`

**Step 1: Write the failing tests**

Add tests for:

```rust
#[test]
fn rejects_zero_sized_virtual_display() { ... }

#[test]
fn reports_driver_unavailable_without_platform_driver() { ... }
```

The first should expect `InvalidMode`. The second should expect `DriverUnavailable` on Windows test builds and `Unsupported` elsewhere.

**Step 2: Run test to verify it fails**

Run:

```powershell
cargo test -p rshare-platform virtual_display
```

Expected: FAIL because the module does not exist.

**Step 3: Implement minimal backend boundary**

Create public functions:

```rust
pub fn list_virtual_displays() -> Result<Vec<VirtualDisplaySnapshot>>;
pub fn create_virtual_display(request: &VirtualDisplayCreateRequest) -> Result<VirtualDisplayOperationResult>;
pub fn remove_virtual_display(request: &VirtualDisplayRemoveRequest) -> Result<VirtualDisplayOperationResult>;
```

Implement validation in shared code. Return truthful unavailable/unsupported results without creating fake displays.

**Step 4: Run test to verify it passes**

Run:

```powershell
cargo test -p rshare-platform virtual_display
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add crates/rshare-platform/src/virtual_display.rs crates/rshare-platform/src/lib.rs
git commit -m "feat: add virtual display platform boundary"
```

### Task 3: Daemon Virtual Display Manager

**Files:**
- Modify: `apps/rshare-daemon/src/main.rs`
- Test: `apps/rshare-daemon/src/main.rs`

**Step 1: Write the failing daemon tests**

Add tests that:

- create a virtual display request with valid dimensions,
- assert the daemon returns `VirtualDisplayOperation`,
- assert list returns the requested snapshot with a truthful unavailable/unsupported status,
- assert duplicate ids return `AlreadyExists`,
- assert invalid dimensions return `InvalidMode`.

**Step 2: Run test to verify it fails**

Run:

```powershell
cargo test -p rshare-daemon virtual_display
```

Expected: FAIL because daemon has no request handling or manager state.

**Step 3: Implement minimal daemon manager**

Add a `VirtualDisplayManager` struct near daemon state helpers. It should own a `BTreeMap<String, VirtualDisplaySnapshot>`, validate ids and modes, call `rshare_platform::virtual_display`, and store the returned snapshot.

Wire IPC handling for:

- `ListVirtualDisplays`
- `CreateVirtualDisplay`
- `RemoveVirtualDisplay`

After create/remove, call `rshare_platform::display::query_display_state()` and refresh daemon local controls if successful.

**Step 4: Run test to verify it passes**

Run:

```powershell
cargo test -p rshare-daemon virtual_display
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add apps/rshare-daemon/src/main.rs
git commit -m "feat: manage virtual display requests in daemon"
```

### Task 4: Tauri Bridge And Frontend Model

**Files:**
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`
- Modify: `apps/rshare-desktop-frontend/src/app/App.tsx`
- Modify: `apps/rshare-desktop-frontend/src/app/desktop-model.mjs`
- Test: `apps/rshare-desktop-frontend/src/app/desktop-model.test.mjs`

**Step 1: Write the failing frontend model test**

Add a test that passes virtual display snapshots into the display settings view model and asserts:

- unavailable driver status is visible,
- a valid default create form mode is offered,
- no fake display is added to the system display list.

**Step 2: Run test to verify it fails**

Run:

```powershell
npm test -- src/app/desktop-model.test.mjs
```

Expected: FAIL because the model has no virtual display view.

**Step 3: Implement bridge and model**

Add Tauri commands:

- `list_virtual_displays`
- `create_virtual_display`
- `remove_virtual_display`

Add network command mapping in `App.tsx`.

Extend the display settings panel with create/remove controls and status rows.

**Step 4: Run tests**

Run:

```powershell
npm test -- src/app/desktop-model.test.mjs
npm run build
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add apps/rshare-desktop/src-tauri/src/main.rs apps/rshare-desktop-frontend/src/app/App.tsx apps/rshare-desktop-frontend/src/app/desktop-model.mjs apps/rshare-desktop-frontend/src/app/desktop-model.test.mjs
git commit -m "feat: expose virtual display controls in desktop"
```

### Task 5: Windows IDD Driver Scaffold

**Files:**
- Create: `drivers/windows/rshare-vdisplay/rshare-vdisplay.inf`
- Create: `drivers/windows/rshare-vdisplay/rshare-vdisplay.vcxproj`
- Create: `drivers/windows/rshare-vdisplay/driver.cpp`
- Modify: `drivers/windows/README.md`

**Step 1: Add scaffold docs and build project**

Create a minimal driver project layout documenting that it is an IDD/IddCx backend target. This task does not claim production driver functionality until an actual WDK build and install path is implemented.

**Step 2: Add README validation checklist**

Document manual verification:

- build driver with WDK,
- install test-signed driver,
- create virtual display through daemon,
- confirm Windows Settings shows the display,
- change mode in Windows Settings,
- confirm daemon display snapshot updates.

**Step 3: Commit**

```powershell
git add drivers/windows/rshare-vdisplay drivers/windows/README.md
git commit -m "docs: scaffold windows virtual display driver"
```

### Task 6: Full Verification

Run:

```powershell
cargo test -p rshare-core virtual_display --test ipc_contract
cargo test -p rshare-platform virtual_display
cargo test -p rshare-daemon virtual_display
cargo check -p rshare-daemon
npm test -- src/app/desktop-model.test.mjs
npm run build
```

Expected: all pass.

Manual Windows IDD verification remains incomplete until the driver can be built, installed, and observed in Windows Settings. Do not mark the full goal complete until that manual evidence exists.
