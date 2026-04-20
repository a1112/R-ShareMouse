# Desktop Auto-Start And Real Layout Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** make `rshare-desktop` auto-start the daemon only when IPC is unavailable and convert `Layout` into a real editor of daemon-owned remembered topology with compact online-only rendering.

**Architecture:** keep the daemon as the runtime owner. Add a small startup orchestration layer in the desktop app, persist/load layout in the daemon, and replace the frontend's fake layout projection with a real `LayoutGraph` plus discovery-status merge and online-only compact rendering.

**Tech Stack:** Rust, Tokio, Tauri, React, Vite, existing `rshare-core`, `rshare-daemon`, `other/figma-ui`

---

### Task 1: Add Failing Desktop Startup Tests For IPC-Gated Auto-Start

**Files:**
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`
- Create or Modify: desktop-side Rust tests in `apps/rshare-desktop/src-tauri/src/main.rs`

**Step 1: Write the failing test**

Add tests that prove:
- successful `dashboard_state` does not trigger daemon spawn
- IPC-unavailable startup triggers exactly one spawn attempt
- non-IPC failures do not trigger daemon spawn

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-desktop`
Expected: FAIL because the desktop backend does not contain startup orchestration yet

**Step 3: Write minimal implementation**

Implement a small startup helper in `apps/rshare-desktop/src-tauri/src/main.rs` or a nearby helper module that:
- probes daemon status
- distinguishes "IPC unavailable" from other failures
- calls `daemon_client::spawn_daemon(...)` only for IPC-unavailable

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-desktop`
Expected: PASS

**Step 5: Commit**

```bash
git add apps/rshare-desktop/src-tauri/src/main.rs
git commit -m "feat: auto-start desktop daemon only when ipc is unavailable"
```

### Task 2: Add Core Layout Persistence Tests In The Daemon Path

**Files:**
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `crates/rshare-core/src/service.rs`

**Step 1: Write the failing test**

Add tests that prove:
- daemon can load a saved layout from state dir
- daemon falls back to local-only layout when no saved layout exists
- saved layout survives daemon restart semantics

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-daemon`
Expected: FAIL because layout persistence is not loaded/saved as part of daemon lifecycle

**Step 3: Write minimal implementation**

Add:
- a persisted layout path helper in `crates/rshare-core/src/service.rs`
- daemon startup load path
- daemon save path on `SetLayout`

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-daemon`
Expected: PASS

**Step 5: Commit**

```bash
git add apps/rshare-daemon/src/main.rs crates/rshare-core/src/service.rs
git commit -m "feat: persist daemon layout across restarts"
```

### Task 3: Add Failing Layout Merge Tests In Core

**Files:**
- Modify: `crates/rshare-core/src/layout.rs`
- Create or Modify: `crates/rshare-core/tests/layout_graph_contract.rs`

**Step 1: Write the failing test**

Add tests that prove:
- newly discovered devices not yet in layout are appended to the right
- existing remembered devices are not reordered when rediscovered
- offline devices remain in persisted graph

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-core layout_graph_contract`
Expected: FAIL because merge helpers do not exist yet

**Step 3: Write minimal implementation**

Add helpers in `crates/rshare-core/src/layout.rs` for:
- merging discovered peers into persisted layout
- right-side append placement
- preserving existing remembered node positions

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-core layout_graph_contract`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/rshare-core/src/layout.rs crates/rshare-core/tests/layout_graph_contract.rs
git commit -m "feat: merge discovered peers into remembered layout"
```

### Task 4: Add Compact Online Projection Tests

**Files:**
- Modify: `crates/rshare-core/src/layout.rs`
- Modify: `crates/rshare-core/tests/layout_graph_contract.rs`

**Step 1: Write the failing test**

Add tests that prove:
- offline nodes are retained in persisted layout
- offline nodes are omitted from visible projection
- visible online nodes are packed without offline gaps

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-core layout_graph_contract`
Expected: FAIL because compact online projection does not exist yet

**Step 3: Write minimal implementation**

Add a projection helper that:
- accepts the full persisted `LayoutGraph`
- accepts a set of online/discovered device ids
- produces an online-only compact topology for rendering

Do not mutate the persisted graph in this helper.

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-core layout_graph_contract`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/rshare-core/src/layout.rs crates/rshare-core/tests/layout_graph_contract.rs
git commit -m "feat: add compact online layout projection"
```

### Task 5: Expand Desktop Tauri Commands For Real Layout Startup

**Files:**
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`

**Step 1: Write the failing test**

Add tests that prove the desktop startup payload can provide:
- status
- devices
- layout
- whether daemon was auto-started during this session

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-desktop`
Expected: FAIL because the current `dashboard_state` only returns status + devices

**Step 3: Write minimal implementation**

Extend the Tauri backend to:
- fetch `get_layout`
- return layout in the startup/dashboard payload or via a dedicated initialization command
- include enough state for the frontend to distinguish "daemon offline", "daemon auto-started", and "layout unavailable"

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-desktop`
Expected: PASS

**Step 5: Commit**

```bash
git add apps/rshare-desktop/src-tauri/src/main.rs
git commit -m "feat: expose real layout in desktop startup payload"
```

### Task 6: Write Failing Frontend Model Tests For Real Layout Consumption

**Files:**
- Modify: `other/figma-ui/src/app/desktop-model.test.mjs`
- Modify: `other/figma-ui/src/app/desktop-model.mjs`

**Step 1: Write the failing test**

Add tests that prove:
- the model uses daemon-provided layout instead of synthesizing all monitors from devices
- new discovered devices are appended to layout memory only once
- offline remembered devices are hidden from visible layout but preserved in remembered data
- visible layout remains tightly packed

**Step 2: Run test to verify it fails**

Run: `node --test other/figma-ui/src/app/desktop-model.test.mjs`
Expected: FAIL because the model still builds layout from `payload.devices`

**Step 3: Write minimal implementation**

Refactor `desktop-model.mjs` so it:
- consumes real layout payload
- merges discovery status into remembered layout
- emits:
  - remembered topology data
  - compact visible layout data
  - device cards with online/connected state

**Step 4: Run test to verify it passes**

Run: `node --test other/figma-ui/src/app/desktop-model.test.mjs`
Expected: PASS

**Step 5: Commit**

```bash
git add other/figma-ui/src/app/desktop-model.mjs other/figma-ui/src/app/desktop-model.test.mjs
git commit -m "feat: bind desktop model to remembered daemon layout"
```

### Task 7: Add Failing App Tests For Desktop Auto-Start And Real Layout Init

**Files:**
- Modify: `other/figma-ui/src/app/App.tsx`
- Create or Modify: frontend tests near `other/figma-ui/src/app/desktop-shell.test.mjs` or a new test file

**Step 1: Write the failing test**

Add tests that prove:
- app startup triggers `start_service` only after IPC-unavailable status probe
- app loads real layout after daemon is available
- app does not call `start_service` when status already succeeds

**Step 2: Run test to verify it fails**

Run: the relevant frontend test command from `other/figma-ui/package.json`
Expected: FAIL because startup logic is still manual-button only

**Step 3: Write minimal implementation**

Update `App.tsx` startup orchestration so it:
- probes startup state
- auto-starts daemon only on IPC unavailable
- loads real layout and devices after readiness

**Step 4: Run test to verify it passes**

Run: the same frontend test command
Expected: PASS

**Step 5: Commit**

```bash
git add other/figma-ui/src/app/App.tsx
git commit -m "feat: auto-start desktop daemon on ipc unavailable"
```

### Task 8: Add Real Layout Save-Back From The Canvas

**Files:**
- Modify: `other/figma-ui/src/app/App.tsx`
- Modify: `other/figma-ui/src/app/components/MonitorManager.tsx`
- Modify: frontend tests for layout interaction

**Step 1: Write the failing test**

Add tests that prove:
- drag updates are translated into a `set_layout` call
- save failures are surfaced as unsaved/error state
- successful save updates remembered layout

**Step 2: Run test to verify it fails**

Run: frontend test command
Expected: FAIL because the canvas currently says "拖拽只影响当前界面展示"

**Step 3: Write minimal implementation**

Modify the layout flow so:
- `MonitorManager` emits drag/end geometry changes upward
- `App.tsx` converts the visible move back into remembered layout updates
- `set_layout` is called on commit
- save failure is shown clearly in the UI

**Step 4: Run test to verify it passes**

Run: frontend test command
Expected: PASS

**Step 5: Commit**

```bash
git add other/figma-ui/src/app/App.tsx other/figma-ui/src/app/components/MonitorManager.tsx
git commit -m "feat: save real layout edits back to daemon"
```

### Task 9: Keep Devices And Layout Semantics Consistent

**Files:**
- Modify: `other/figma-ui/src/app/App.tsx`
- Modify: `other/figma-ui/src/app/desktop-model.mjs`
- Modify: frontend tests

**Step 1: Write the failing test**

Add tests that prove:
- `Devices` page and `Layout` page use the same underlying discovery/connection source
- offline devices remain in remembered topology but are absent from visible layout
- connected/online labels match across pages

**Step 2: Run test to verify it fails**

Run: frontend test command
Expected: FAIL until model unification is complete

**Step 3: Write minimal implementation**

Remove remaining fake-layout assumptions so both pages are driven from:
- daemon status
- daemon devices
- daemon layout
- frontend merge/projection helpers

**Step 4: Run test to verify it passes**

Run: frontend test command
Expected: PASS

**Step 5: Commit**

```bash
git add other/figma-ui/src/app/App.tsx other/figma-ui/src/app/desktop-model.mjs
git commit -m "fix: unify desktop device and layout state"
```

### Task 10: Run Product-Level Verification And Lock Docs

**Files:**
- Modify: `docs/plans/2026-04-19-desktop-daemon-layout-product-design.md`
- Modify: `docs/plans/2026-04-19-desktop-daemon-layout-implementation-plan.md`

**Step 1: Add the validation checklist**

Record checks for:
- desktop startup with daemon already online
- desktop startup with daemon offline
- discovery-driven right-side append
- drag/save persistence
- daemon restart memory restore
- offline hide + remembered restore
- compact visible packing

**Step 2: Run verification**

Run:
- `cargo check --workspace`
- `cargo test --workspace`
- frontend test command for `other/figma-ui`

Expected: PASS, or capture the failures blocking completion

**Step 3: Update docs with evidence**

Document:
- what passed
- what remains open
- any manual verification still required

**Step 4: Re-run verification**

Run the same commands again.
Expected: PASS

**Step 5: Commit**

```bash
git add docs/plans/2026-04-19-desktop-daemon-layout-product-design.md docs/plans/2026-04-19-desktop-daemon-layout-implementation-plan.md
git commit -m "docs: lock desktop daemon layout product plan"
```

---

## Execution Notes

- Keep daemon ownership strict. Do not move tray or persisted topology into the desktop app.
- Treat online compact layout as a pure projection over remembered topology.
- Do not silently overwrite remembered positions just because discovery order changed.
- Prefer adding testable helpers in `rshare-core/src/layout.rs` over burying merge logic inside React components.
- Keep commits small and preserve TDD at each task boundary.

## Expected Exit Condition

When this plan is complete:

- opening `rshare-desktop` reliably boots the product console without a manual start button when daemon IPC is unavailable
- `Layout` is backed by real daemon layout state
- newly discovered LAN devices join the real remembered topology
- offline devices keep memory but disappear from the visible canvas
- visible devices stay tightly packed
- daemon remains the owner of tray and background service lifecycle

## Implementation Evidence

Implemented in the `codex/figma-titlebar-tighten` branch through:

- `c8106dc` - desktop IPC-miss daemon auto-start
- `ffadfd1` and `c707539` - daemon layout persistence and corruption-safe recovery
- `c0fbf80` - core remembered-layout merge and online display projection helpers
- `4418c78` - desktop dashboard binding to daemon layout, discovery merge, and visible projection
- `aa0b500` and `cdf0acf` - visible layout edit save-back with remembered-coordinate preservation

Verification completed on 2026-04-20:

- `cargo check --workspace` passed with no warnings after the final daemon test-only helper cleanup.
- `cargo test --workspace` passed.
- `npm.cmd test` in `other/figma-ui` passed.
- `npm.cmd run build` in `other/figma-ui` passed.

Manual verification still required:

- real two-machine LAN discovery and right-side append
- daemon offline -> desktop auto-start in the packaged Tauri shell
- drag/save placement persistence across desktop restart and daemon restart
- offline remembered device hidden while online devices remain compact
