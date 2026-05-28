# Remote Device Visibility And Monitoring Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Keep discovered remote computers visible in layout/devices before connection and expose remote hardware state in the device console.

**Architecture:** Preserve daemon ownership of discovery, layout, and capability truth. Fix the frontend model so discovery visibility is separate from control connection, then synthesize remote monitor snapshots from capability/layout metadata plus endpoint events.

**Tech Stack:** Rust daemon/Tauri IPC contracts, React/TypeScript desktop frontend, Node `node:test` model tests.

---

### Task 1: Keep Discovered Remotes Visible

**Files:**
- Modify: `apps/rshare-desktop-frontend/src/app/desktop-model.test.mjs`
- Modify: `apps/rshare-desktop-frontend/src/app/desktop-model.mjs`

**Step 1: Write the failing test**

Add a test that passes one discovered-but-not-connected remote device and a visible layout containing that remote. Assert:
- `model.devices.length === 1`
- `model.devices[0].online === true`
- `model.devices[0].connected === false`
- `model.layout.devices` contains the remote device
- the remote gallery item says `已发现`

**Step 2: Run test to verify it fails**

Run: `npm test -- src/app/desktop-model.test.mjs`
Expected: FAIL because the remote is filtered from `model.devices`.

**Step 3: Write minimal implementation**

Change `buildRemoteDevice()` so `online` means discovered/recently seen, not connected. Remove the `discoveredRemoteDevices.filter((device) => device.online)` path or keep it equivalent to all discovered devices.

**Step 4: Run test to verify it passes**

Run: `npm test -- src/app/desktop-model.test.mjs`
Expected: PASS.

### Task 2: Build Remote Hardware Monitor Snapshot

**Files:**
- Modify: `apps/rshare-desktop-frontend/src/app/desktop-model.test.mjs`
- Modify: `apps/rshare-desktop-frontend/src/app/desktop-model.mjs`
- Modify: `apps/rshare-desktop-frontend/src/app/App.tsx`

**Step 1: Write the failing test**

Add a model test for a remote device with:
- capability registry entries for input, gamepad, audio, display topology
- visible layout display metadata for the remote
- one remote endpoint event

Assert the generated remote snapshot exposes keyboard/mouse capability rows, display count, and recent remote events without using local device state.

**Step 2: Run test to verify it fails**

Run: `npm test -- src/app/desktop-model.test.mjs`
Expected: FAIL because no exported remote snapshot builder exists and App currently builds a sparse snapshot from events only.

**Step 3: Write minimal implementation**

Export a `buildRemoteControlSnapshot()` helper from `desktop-model.mjs`. It should:
- start from a blank snapshot
- add remote layout displays from `visible_layout`
- set keyboard/mouse detected from the remote Input capability
- set gamepad/audio/display availability from capability details
- replay matching recent endpoint events

Update `App.tsx` to call this helper when a remote device is selected.

**Step 4: Run tests**

Run: `npm test -- src/app/desktop-model.test.mjs`
Expected: PASS.

### Task 3: Verify Workspace

**Files:**
- Existing frontend and Rust files only.

**Step 1: Run focused frontend tests**

Run: `npm test -- src/app/desktop-model.test.mjs`

**Step 2: Run frontend build**

Run: `npm run build`

**Step 3: Run focused Rust layout/daemon tests if frontend changes reveal contract risk**

Run: `cargo test -p rshare-core layout_graph_contract`
Run: `cargo test -p rshare-daemon discovered_device_updates_in_memory_layout_without_desktop_roundtrip`
