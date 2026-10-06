# Mobile Virtual HID Gamepad Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the paired Android controller expose a real Windows HID/DirectInput gamepad with safe full-state release.

**Architecture:** A landscape WebView controller sends typed full-state snapshots over the authenticated mobile gateway. The daemon validates and leases the single gamepad, then the Windows virtual HID backend submits one atomic report to the existing VHF driver. The driver advertises a dedicated capability so an old installation cannot appear to support gamepads.

**Tech Stack:** Rust core/daemon/input/platform, Windows KMDF VHF C driver and WDK, Android Java/WebView HTML/CSS/JS, Gradle, Android emulator.

---

### Task 1: Define the driver gamepad report

**Files:**
- Modify: `drivers/windows/rshare-common/rshare_ioctls.h`
- Modify: `drivers/windows/rshare-vhid/driver.c`
- Modify: `drivers/windows/tools/rshare-driver-probe.c`
- Modify: `drivers/windows/README.md`

**Steps:**
1. Add a distinct gamepad capability bit and full-state IOCTL payload (16 buttons, four signed axes, two unsigned triggers, one 8-way hat). Keep the old ABI intact; old drivers must fail the new call cleanly.
2. Extend the VHF descriptor with report ID 3 and a Gamepad top-level application collection. Map X/Y/Rx/Ry, button usages, two trigger usages, and a hat with neutral state.
3. Add one atomic report-submission branch with strict size/range checking and a neutral initial state. The driver must emit a neutral report during cleanup where possible.
4. Add a probe command that sends a recognizable press/release and prints capability status. Build only `rshare-vhid.vcxproj`; confirm no WDK errors.

### Task 2: Connect the Rust Windows backend

**Files:**
- Modify: `crates/rshare-platform/src/windows.rs`
- Modify: `crates/rshare-input/src/backend.rs`
- Test: co-located Windows platform/backend tests

**Steps:**
1. Write failing tests for HID packing: button bit mapping, D-pad diagonal/neutral hat, signed stick endpoints, trigger conversion, and old-driver capability rejection.
2. Add a Rust mirror of the fixed IOCTL payload with size assertions, serialize `GamepadState`, and call the new IOCTL only when `virtual_gamepad` is advertised.
3. Handle `InputEvent::GamepadState` in `VirtualHidInjectBackend`; make `GamepadDisconnected` send neutral. Route gamepad events from Windows-native and portable inject backends to the same VHF driver because keyboard/mouse backend selection may not choose VirtualHid. Preserve existing keyboard/mouse behavior.
4. Run `cargo test -p rshare-platform` and `cargo test -p rshare-input`.

### Task 3: Accept gamepad state from the mobile gateway

**Files:**
- Modify: `crates/rshare-core/src/endpoint_events.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `apps/rshare-daemon/src/mobile_gateway.rs`
- Test: co-located daemon gateway and core tests

**Steps:**
1. Write failing tests for typed `GamepadState` decoding, bounded buttons and axes, one-client ownership, neutral release, and stale-client lease recovery.
2. Convert the typed payload to the existing input event. Validate gamepad ID and supported buttons before dispatch.
3. Add one active gamepad client to mobile session ownership. Set a short lease, reject another client while active, and neutralize on explicit release and lease cleanup. Failed injections must not claim or advance ownership.
4. Include gamepad neutralization in page-hide release handling and on gateway shutdown. Run focused daemon tests, then `cargo test -p rshare-daemon`.

### Task 4: Implement the mobile landscape controls

**Files:**
- Modify: `apps/rshare-daemon/src/mobile_gateway.rs` (served HTML/CSS/JS)
- Modify: `apps/rshare-mobile-android/app/src/main/java/org/rsharemouse/mobile/MainActivity.java`
- Modify: `apps/rshare-mobile-android/app/src/main/res/layout/activity_main.xml`
- Test: rendered-page tests and Android build

**Steps:**
1. Add failing rendered-page tests for a third mode, two independent stick pointers, button hold/release, a neutral release event, and lifecycle cancellation.
2. Add a responsive landscape gamepad surface with dual sticks, D-pad, ABXY, LB/RB, LT/RT, Start/Select, L3/R3. Send complete snapshots on changes, coalesce stick moves, and send a brief held-state heartbeat.
3. Replace the boolean Android keyboard mode with an explicit touch/keyboard/gamepad mode. Both keyboard and gamepad lock landscape; switching away releases gamepad state before changing UI.
4. Build with `apps/rshare-mobile-android/gradlew.bat assembleDebug`; install to `emulator-5554` and test multi-touch, orientation, cancellation, and connection status.

### Task 5: Verify the Windows device and package results

**Files:**
- Modify: `apps/rshare-mobile-android/README.md`
- Modify: `docs/plans/2026-09-26-mobile-virtual-gamepad-design.md` only if implementation decisions change

**Steps:**
1. Run `cargo fmt --all -- --check`, `cargo test -p rshare-core -p rshare-platform -p rshare-input -p rshare-daemon`, the driver build, and Android build.
2. If the local driver can be updated, install the new test-signed VHF driver through the repository script, check the new capability with the probe, and inspect the controller in `joy.cpl` or equivalent HID/DirectInput enumerator. Record whether a restart is required.
3. Test mobile input against the local gateway using the emulator and verify axis/button transitions plus disconnect neutralization. Copy the debug APK into `apps/rshare-mobile-android/dist/` with the project versioned name.
4. Report the exact verified compatibility and any deployment step still required for the Xiaomi 14 Pro or Windows driver.
