# Alpha-2 Input Loop Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Move R-ShareMouse from Alpha-1 control-plane completeness to Alpha-2 by closing the first real Windows-to-Windows input sharing loop.

**Architecture:** Keep the existing daemon, IPC, and Tauri shell, but make backend status reflect real operational capability and then replace the capture-side stubs with a genuine Windows capture path. Validate the loop incrementally: backend truthfulness first, capture implementation second, integration wiring third, and dual-machine validation last.

**Tech Stack:** Rust, Tokio, Tauri, Win32 hooks/SendInput, existing `rshare-core`, `rshare-input`, `rshare-platform`, `rshare-net`

---

### Task 1: Freeze Alpha-2 Expectations In Docs And Tests

**Files:**
- Modify: `docs/roadmap.md`
- Modify: `apps/rshare-daemon/src/main.rs`
- Test: `apps/rshare-daemon/src/main.rs`

**Step 1: Write the failing test**

Add daemon tests that prove:
- a backend is not considered available if capture or inject is unavailable
- selected mode and degraded state match actual healthy candidates

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-daemon`
Expected: new backend-selection assertions fail before implementation exists

**Step 3: Write minimal implementation**

Add helper functions that:
- derive a backend candidate from capture + inject health
- resolve selected mode, available backend list, and degraded state from the candidate set

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-daemon`
Expected: backend-selection tests pass

**Step 5: Commit**

```bash
git add docs/roadmap.md apps/rshare-daemon/src/main.rs
git commit -m "test: codify alpha-2 backend selection rules"
```

### Task 2: Lock The Windows SendInput ABI With Tests

**Files:**
- Modify: `crates/rshare-platform/src/windows.rs`
- Test: `crates/rshare-platform/src/windows.rs`

**Step 1: Write the failing test**

Add a Windows-only unit test asserting:
- `MouseInput` size is `32`
- `KeyboardInput` size is `24`
- `InputPayload` size is `32`
- `Input` size is `40`
- `Input.payload` offset is `8`

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-platform windows::windows_impl::tests::send_input_layout_matches_windows_64bit_abi`
Expected: failure if the ABI structs are wrong or regress

**Step 3: Write minimal implementation**

Use `#[repr(C)]` native structs and unions only; do not hand-pack byte arrays or hard-code field offsets.

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-platform windows::windows_impl::tests::send_input_layout_matches_windows_64bit_abi`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/rshare-platform/src/windows.rs
git commit -m "test: protect windows sendinput abi layout"
```

### Task 3: Make Windows Native Inject Availability Truthful

**Files:**
- Modify: `crates/rshare-input/src/emulator.rs`
- Modify: `crates/rshare-input/src/backend.rs`
- Test: `crates/rshare-input/src/emulator.rs`
- Test: `crates/rshare-input/src/backend.rs`

**Step 1: Write the failing test**

Add tests that prove:
- `WindowsNativeInputEmulator::new()` is active only if the inner emulator is actually activated
- `WindowsNativeInjectBackend::inject()` returns an error when inactive
- `PortableInjectBackend::inject()` is not a no-op and fails when inactive

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-input`
Expected: at least one of the new activation/inject contract tests fails

**Step 3: Write minimal implementation**

Ensure:
- backend constructors activate the underlying emulator
- inject paths return explicit errors when inactive
- health reflects actual activation failure instead of optimistic defaults

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-input`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/rshare-input/src/emulator.rs crates/rshare-input/src/backend.rs
git commit -m "fix: make inject backend activation truthful"
```

### Task 4: Replace Portable Capture Stub With A Real Adapter Wrapper

**Files:**
- Modify: `crates/rshare-input/src/backend.rs`
- Modify: `crates/rshare-input/src/listener.rs`
- Test: `crates/rshare-input/src/backend.rs`

**Step 1: Write the failing test**

Add tests that prove:
- portable capture start creates a real running listener state
- stop transitions it back to not running
- health does not remain degraded when the listener is active

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-input portable_capture`
Expected: failure because start currently returns not implemented

**Step 3: Write minimal implementation**

Wrap `RDevInputListener` inside `PortableCaptureBackend` so the adapter:
- owns the listener
- starts and stops it
- reports running/health from the real listener state

Do not add routing logic yet; just make the adapter truthful and operational.

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-input portable_capture`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/rshare-input/src/backend.rs crates/rshare-input/src/listener.rs
git commit -m "feat: wire portable capture backend to real listener"
```

### Task 5: Implement The First Windows Capture Backend

**Files:**
- Modify: `crates/rshare-platform/src/windows.rs`
- Modify: `crates/rshare-input/src/backend.rs`
- Modify: `crates/rshare-input/src/listener.rs`
- Test: `crates/rshare-input/src/backend.rs`
- Test: `crates/rshare-platform/src/windows.rs`

**Step 1: Write the failing test**

Add tests that prove:
- `WindowsNativeCaptureBackend` no longer reports permanent unavailability once initialized
- adapter start/stop transitions are driven by the real Windows listener implementation

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-input windows_native_capture`
Expected: failure because backend start is still stubbed

**Step 3: Write minimal implementation**

Implement the first usable Windows capture path:
- initialize low-level hooks through `WindowsInputListener`
- wire start/stop into `WindowsNativeCaptureBackend`
- expose health based on actual hook initialization outcome

Keep this first implementation narrow:
- unlocked desktop only
- no special secure desktop handling yet

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-input windows_native_capture`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/rshare-platform/src/windows.rs crates/rshare-input/src/backend.rs crates/rshare-input/src/listener.rs
git commit -m "feat: implement initial windows capture backend"
```

### Task 6: Route Real Backend State Into Daemon Status

**Files:**
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `crates/rshare-input/src/selection.rs`
- Test: `apps/rshare-daemon/src/main.rs`

**Step 1: Write the failing test**

Add daemon tests that prove:
- `available_backends` only contains backends with healthy capture and inject sides
- degraded state is reported when falling back from Windows native to portable
- no backend means `input_mode = None`

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-daemon`
Expected: failure if daemon still builds candidate sets optimistically

**Step 3: Write minimal implementation**

Connect daemon selection to the real adapter health returned by input backends. Do not infer health from constructor success alone.

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-daemon`
Expected: PASS

**Step 5: Commit**

```bash
git add apps/rshare-daemon/src/main.rs crates/rshare-input/src/selection.rs
git commit -m "fix: derive daemon backend state from real adapter health"
```

### Task 7: Expose Real Backend Truth To CLI And Desktop UI

**Files:**
- Modify: `apps/rshare-cli/src/commands/status.rs`
- Modify: `apps/rshare-cli/src/commands/devices.rs`
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`
- Modify: `other/figma-ui/src/app/...`
- Test: desktop-model tests under `other/figma-ui/src/app/*.test.mjs`

**Step 1: Write the failing test**

Add tests that prove:
- CLI no longer prints unavailable backends as available
- desktop view model renders `no backend`, `degraded`, and `portable fallback` distinctly

**Step 2: Run test to verify it fails**

Run:
- `cargo test -p rshare-cli`
- `npm.cmd test` in `other/figma-ui`

Expected: state rendering tests fail before the UI/CLI logic is updated

**Step 3: Write minimal implementation**

Update CLI/Tauri/UI mapping so the operator sees:
- selected mode
- available backends
- degraded fallback reason
- “no usable backend” state

**Step 4: Run test to verify it passes**

Run:
- `cargo test -p rshare-cli`
- `npm.cmd test`

Expected: PASS

**Step 5: Commit**

```bash
git add apps/rshare-cli/src/commands/status.rs apps/rshare-cli/src/commands/devices.rs apps/rshare-desktop/src-tauri/src/main.rs other/figma-ui/src
git commit -m "feat: surface backend truth in cli and desktop ui"
```

### Task 8: Validate End-To-End On Two Windows Machines

**Files:**
- Modify as needed based on findings in:
  - `apps/rshare-daemon/src/main.rs`
  - `crates/rshare-input/src/backend.rs`
  - `crates/rshare-platform/src/windows.rs`
  - `crates/rshare-net/src/network_manager.rs`

**Step 1: Prepare the validation script**

Document a fixed manual runbook:
- start daemon on both machines
- verify discovery
- connect from machine A to machine B
- verify mouse move
- verify left/right click
- verify wheel
- verify keyboard press/release
- disconnect and reconnect

**Step 2: Run the dual-machine validation**

Expected:
- no false healthy state when capture is unavailable
- input events reach the remote machine on the normal desktop

**Step 3: Fix only the first blocking root cause**

Do not batch speculative fixes. If validation fails, capture logs and fix one root cause at a time.

**Step 4: Re-run the validation**

Expected:
- end-to-end loop passes on the validated scenarios

**Step 5: Commit**

```bash
git add <files changed by the first real dual-machine fix>
git commit -m "fix: close first dual-machine input loop"
```

### Task 9: Run The Alpha-2 Package Verification Set

**Files:**
- No code changes unless verification exposes a regression

**Step 1: Run Rust package tests**

Run:
- `cargo test -p rshare-core`
- `cargo test -p rshare-platform`
- `cargo test -p rshare-input`
- `cargo test -p rshare-daemon`

Expected: all pass

**Step 2: Run desktop frontend tests**

Run in `other/figma-ui`:
- `npm.cmd test`
- `npm run build`

Expected: all pass

**Step 3: Run desktop build**

Run:
- `cargo build -p rshare-desktop`

Expected: PASS

**Step 4: Record residual risks**

Document anything still missing from Alpha-2:
- macOS path
- secure desktop/UAC/login behavior
- clipboard sync

**Step 5: Commit**

```bash
git add .
git commit -m "chore: verify alpha-2 input loop baseline"
```

### Task 10: Declare Alpha-2 Or Re-Scope Honestly

**Files:**
- Modify: `docs/roadmap.md`
- Modify: `docs/plans/2026-04-19-alpha-2-input-loop.md`

**Step 1: Compare actual outcomes against Alpha-2 exit criteria**

Check:
- real Windows capture exists
- real dual-machine path works
- status plane matches reality

**Step 2: Update roadmap truthfully**

If all pass:
- move project status to Alpha-2

If not:
- leave project at Alpha-1 and record the remaining blockers precisely

**Step 3: Re-run doc sanity check**

No command required beyond reading the updated docs and ensuring they match actual behavior.

**Step 4: Commit**

```bash
git add docs/roadmap.md docs/plans/2026-04-19-alpha-2-input-loop.md
git commit -m "docs: update alpha stage after validation"
```
