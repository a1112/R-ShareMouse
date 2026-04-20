# Desktop Background Acceptance Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `rshare-desktop` ready for dual-machine acceptance by exposing daemon-owned background/tray state and adding a clear UI acceptance checklist.

**Architecture:** The daemon remains the owner of background service state and future tray semantics. The Tauri desktop shell only probes IPC, auto-starts the daemon when IPC is unavailable, reads one dashboard snapshot, and renders acceptance readiness from that snapshot.

**Tech Stack:** Rust/Tokio daemon, `rshare-core` IPC DTOs, Tauri 2 commands, React/Vite Figma UI, Node test runner.

---

### Task 1: IPC Background Contract

**Files:**
- Modify: `crates/rshare-core/src/runtime.rs`
- Modify: `crates/rshare-core/src/ipc.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Test: `crates/rshare-core/tests/ipc_contract.rs`

**Step 1: Write failing tests**

Add a contract test asserting `ServiceStatusSnapshot::new(...)` exposes a daemon-owned background state with:
- background owner `Daemon`
- background mode `BackgroundProcess`
- tray owner `Daemon`
- tray state `Unavailable`
- autostarted by desktop `false`

**Step 2: Verify red**

Run: `cargo test -p rshare-core --test ipc_contract background`

Expected: FAIL because the new fields/types do not exist.

**Step 3: Implement minimal model**

Add serializable enums for `BackgroundProcessOwner`, `BackgroundRunMode`, and `TrayRuntimeState`. Add optional/default-compatible fields to `ServiceStatusSnapshot`.

**Step 4: Verify green**

Run: `cargo test -p rshare-core --test ipc_contract background`

Expected: PASS.

### Task 2: Desktop Dashboard Acceptance Payload

**Files:**
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`

**Step 1: Write failing tests**

Add tests proving:
- `dashboard_state_with` propagates desktop auto-start into `status.started_by_desktop`.
- successful probe keeps `started_by_desktop` false.
- dashboard payload returns a compact `acceptance` object derived from service/layout/device/input readiness.

**Step 2: Verify red**

Run: `cargo test -p rshare-desktop dashboard_state`

Expected: FAIL because the acceptance payload and status mutation do not exist.

**Step 3: Implement minimal code**

Add `DesktopAcceptancePayload` and compute it from the existing status/devices/layout snapshot. Mutate only the returned snapshot to annotate desktop autostart; do not change daemon runtime state.

**Step 4: Verify green**

Run: `cargo test -p rshare-desktop dashboard_state`

Expected: PASS.

### Task 3: UI Acceptance Checklist

**Files:**
- Modify: `other/figma-ui/src/app/desktop-model.mjs`
- Modify: `other/figma-ui/src/app/desktop-model.test.mjs`
- Modify: `other/figma-ui/src/app/App.tsx`

**Step 1: Write failing tests**

Add model tests proving:
- acceptance readiness is displayed from payload when present.
- offline daemon shows background/tray and main-link checks as blocked.
- online daemon with layout and discovered device shows dual-machine validation checks.

**Step 2: Verify red**

Run: `npm.cmd test` in `other/figma-ui`

Expected: FAIL because model acceptance fields do not exist.

**Step 3: Implement minimal UI**

Add a Settings page section named `实机验收` with concise checks: daemon/background, tray owner/state, local endpoint, discovery, layout, input backend, and dual-machine next step. Keep Layout/Devices pages unchanged.

**Step 4: Verify green**

Run: `npm.cmd test` in `other/figma-ui`

Expected: PASS.

### Task 4: Full Verification

**Files:**
- No required source changes unless tests expose regressions.

**Step 1: Rust tests**

Run: `cargo test --workspace`

Expected: PASS.

**Step 2: Frontend build**

Run: `npm.cmd run build` in `other/figma-ui`

Expected: PASS.

**Step 3: Desktop build check**

Run: `cargo check -p rshare-desktop`

Expected: PASS.

