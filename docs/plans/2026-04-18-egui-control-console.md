# Egui Control Console Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Turn the current `rshare-gui` dashboard into a stable, product-like control console without changing the underlying daemon-driven state model.

**Architecture:** Keep `RShareApp` as the source of truth for daemon snapshots and navigation. Refactor `MainView` into a dashboard renderer with explicit layout helpers and action return values, then apply a stronger `egui` theme from `main.rs` so the shell and dashboard feel cohesive.

**Tech Stack:** Rust, `egui`, `eframe`, existing daemon IPC in `rshare-core`

---

### Task 1: Add failing tests for dashboard layout decisions

**Files:**
- Modify: `G:/Project/R-ShareMouse/apps/rshare-gui/src/ui/main_view.rs`

**Step 1: Write the failing test**

Add tests that describe:
- narrow widths stack dashboard content into one column
- wide widths split dashboard content into two columns
- hero action label is `Start Service` when the daemon is offline and `Stop Service` when it is online

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-gui main_view`
Expected: FAIL because the new layout helper and action label helper do not exist yet.

**Step 3: Write minimal implementation**

Add deterministic helper functions for:
- content column selection based on available width
- hero action label based on `DashboardSummary`

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-gui main_view`
Expected: PASS

**Step 5: Commit**

Skip commit in this shared dirty workspace unless the user explicitly asks for one.

### Task 2: Refactor the dashboard into a control console

**Files:**
- Modify: `G:/Project/R-ShareMouse/apps/rshare-gui/src/ui/main_view.rs`
- Modify: `G:/Project/R-ShareMouse/apps/rshare-gui/src/app.rs`

**Step 1: Write the failing test**

Extend tests to prove quick actions return navigation intents instead of acting as inert buttons.

**Step 2: Run test to verify it fails**

Run: `cargo test -p rshare-gui main_view`
Expected: FAIL because the action enum and routing helpers do not exist yet.

**Step 3: Write minimal implementation**

Refactor `MainView::show` to:
- render a hero panel
- render stable metric cards with minimum sizes
- render recent activity and device overview panels
- return dashboard actions for navigation and service control

Update `RShareApp` to handle those actions and route to tabs or trigger service changes.

**Step 4: Run test to verify it passes**

Run: `cargo test -p rshare-gui main_view`
Expected: PASS

**Step 5: Commit**

Skip commit in this shared dirty workspace unless the user explicitly asks for one.

### Task 3: Apply a cohesive desktop theme

**Files:**
- Modify: `G:/Project/R-ShareMouse/apps/rshare-gui/src/main.rs`

**Step 1: Write the failing test**

No automated theme fidelity test is practical here. Reuse the existing layout tests and rely on build verification for this task.

**Step 2: Run test to verify it fails**

Not applicable for pure style constants; keep behavior tests green before changing theme code.

**Step 3: Write minimal implementation**

Increase viewport size, tighten panel visuals, raise corner radius and spacing, and use a coherent dark console palette.

**Step 4: Run test to verify it passes**

Run:
- `cargo test -p rshare-gui`
- `cargo build -p rshare-gui`

Expected: PASS

**Step 5: Commit**

Skip commit in this shared dirty workspace unless the user explicitly asks for one.

### Task 4: Run acceptance verification

**Files:**
- None

**Step 1: Start the updated GUI**

Run: `cargo build -p rshare-gui`

**Step 2: Launch the executable**

Run: `target\\debug\\rshare-gui.exe`

**Step 3: Verify the dashboard**

Check:
- cards no longer collapse into vertical text
- hero state and primary action are obvious
- quick actions navigate to tabs
- recent activity and device summary read cleanly

**Step 4: Report residual gaps**

Call out any remaining areas that still look placeholder-grade.

**Step 5: Commit**

Skip commit in this shared dirty workspace unless the user explicitly asks for one.
