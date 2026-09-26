# Device Event Monitor UI State Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Restore live Devices-page monitoring for local and remote input through one UI-state stream.

**Architecture:** Add a bounded daemon-owned monitor projection to UI snapshots and reliable deltas. Sample changed monitor state, subscribe to peer diagnostics on connection, and render the projection from the frontend UI-state store. Keep the existing local-controls request for offline fallback.

**Tech Stack:** Rust, Tokio, serde, React, JavaScript UI store, Node tests.

---

### Task 1: UI-state monitor contract

**Files:** `crates/rshare-core/src/ui_state.rs`, `apps/rshare-daemon/src/state_aggregator.rs`

1. Add a failing core test for snapshot and `device_monitor` delta application.
2. Run the focused core test and confirm the expected failure.
3. Add a bounded monitor value to `UiDynamicState`, a `UiChange` variant, and aggregator forwarding.
4. Run the focused core and aggregator tests.

### Task 2: Daemon monitor publisher

**Files:** `apps/rshare-daemon/src/main.rs`

1. Add failing tests for remote-event selection and changed-only monitor sampling.
2. Run those tests and confirm failure.
3. Project local and recent remote events into the initial snapshot, publish changed samples on a 100 ms timer, and request peer diagnostics on connection.
4. Run focused daemon tests and check the daemon build.

### Task 3: Frontend consumption

**Files:** `apps/rshare-desktop-frontend/src/app/ui-store.mjs`, `apps/rshare-desktop-frontend/src/app/desktop-model.mjs`, `apps/rshare-desktop-frontend/src/app/App.tsx`, matching tests.

1. Add failing store/model tests proving monitor snapshot and delta update event history and counters.
2. Run focused Node tests and confirm failure.
3. Feed the UI monitor projection to the Devices page, remove its normal secondary event subscriptions, and retain a request-based fallback when UI state is unavailable.
4. Run all frontend tests and production build.

### Task 4: End-to-end verification

1. Run focused Rust tests and frontend tests again after integration.
2. Restart only the daemon and GUI if needed to verify their new binaries, preserving the current mobile/driver work.
3. Observe a local input event in a UI-state snapshot or delta and confirm Devices-page counters/history update.
4. Review `git diff` and report verification results and remaining limits.
