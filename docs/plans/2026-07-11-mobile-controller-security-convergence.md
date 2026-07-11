# Mobile Controller Security Convergence Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `codex/mobile-controller` safe-by-default, deterministic under failure, fully verified, and ready to merge into local `main`.

**Architecture:** Discovery remains read-only until an explicit connect; QUIC first-seen trust is committed only after strict application identity validation. The plaintext mobile gateway becomes an opt-in experimental service with bounded resources, ordered client sequences, held-input recovery, and redacted diagnostics. Platform-specific behavior is capability-driven and hardware-mutating tests are isolated from the default suite.

**Tech Stack:** Rust 2021, Tokio, Quinn/rustls, serde/TOML, Tauri, React 18, Node test runner, Windows SendInput/CoreGraphics.

---

### Task 1: Preserve safety configuration and disable the gateway by default

**Files:**
- Modify: `crates/rshare-core/src/config.rs`
- Modify: `crates/rshare-core/tests/ipc_contract.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Test: `crates/rshare-core/src/config.rs`
- Test: `apps/rshare-daemon/src/main.rs`

**Step 1: Write failing configuration tests**

Add tests proving that the gateway is opt-in and a saved forwarding opt-out survives loading:

```rust
#[test]
fn mobile_gateway_is_disabled_by_default() {
    assert!(!Config::default().features.mobile_gateway_enabled);
}

#[test]
fn load_preserves_explicit_automatic_forwarding_opt_out() {
    let path = temp_config_path("explicit-forwarding-opt-out");
    let mut config = Config::default();
    config.features.automatic_input_forwarding = false;
    config.save_to_path(&path).unwrap();
    assert!(!Config::load_from_path(&path)
        .unwrap()
        .features
        .automatic_input_forwarding);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
```

**Step 2: Verify RED**

Run: `cargo test -p rshare-core config::tests:: -- --nocapture`

Expected: FAIL because `mobile_gateway_enabled` is missing and the migration rewrites the explicit `false`.

**Step 3: Implement the minimal configuration change**

- Add `#[serde(default)] pub mobile_gateway_enabled: bool` to `FeatureConfig`.
- Set it to `false` in `Default`.
- Remove `apply_legacy_default_migrations` and the value-based legacy heuristic.
- Add the value to `RuntimeFeatureConfig` and construct `MobileGatewayAccess::disabled(...)` when false.
- Do not bind port 27437 when disabled and do not expose a token.

**Step 4: Verify GREEN**

Run: `cargo test -p rshare-core config::tests -- --nocapture`

Run: `cargo test -p rshare-daemon mobile_gateway -- --nocapture`

Expected: all selected tests pass.

**Step 5: Commit**

```bash
git add crates/rshare-core/src/config.rs crates/rshare-core/tests/ipc_contract.rs apps/rshare-daemon/src/main.rs
git commit -m "fix: preserve safe mobile configuration"
```

### Task 2: Make experimental gateway exposure truthful and bounded at the OS boundary

**Files:**
- Modify: `apps/rshare-daemon/src/mobile_gateway.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `crates/rshare-platform/src/firewall.rs`
- Modify: `apps/rshare-desktop-frontend/index.html`
- Modify: `apps/rshare-desktop-frontend/src/app/App.tsx`
- Modify: `apps/rshare-desktop-frontend/src/app/desktop-model.mjs`
- Modify: `apps/rshare-desktop-frontend/src/app/desktop-model.test.mjs`
- Delete: `apps/rshare-desktop-frontend/public/mobile.webmanifest`
- Delete: `apps/rshare-desktop-frontend/public/mobile-icon.svg`
- Modify: `apps/rshare-desktop-frontend/src/app/desktop-shell.test.mjs`

**Step 1: Write failing tests**

- Add a firewall unit test whose expected rules include TCP 27437 only when `mobile_gateway_enabled` is true.
- Add daemon tests asserting a disabled access snapshot contains no URL/token.
- Update frontend tests to require an explicit plaintext experimental warning and to reject PWA manifest/Wake Lock metadata.

**Step 2: Verify RED**

Run: `cargo test -p rshare-platform firewall -- --nocapture`

Run: `npm.cmd test -- desktop-model.test.mjs desktop-shell.test.mjs`

Expected: FAIL because mobile TCP firewall state and truthful UI copy are absent.

**Step 3: Implement minimal exposure behavior**

- Load config before Windows firewall configuration.
- Change firewall configuration/check APIs to accept whether mobile TCP is required; removal always removes any old mobile rule.
- Add the manual `netsh` instruction for `protocol=TCP localport=27437` only when enabled.
- Remove manifest/icon routes, manifest tags, install assets, Wake Lock code, and their source-string tests.
- Label enabled URLs as `实验性明文局域网控制`; disabled mode explains how to opt in.

**Step 4: Verify GREEN**

Run: `cargo test -p rshare-platform firewall -- --nocapture`

Run: `cargo test -p rshare-daemon mobile_gateway -- --nocapture`

Run: `npm.cmd test` from `apps/rshare-desktop-frontend`.

**Step 5: Commit**

```bash
git add apps/rshare-daemon crates/rshare-platform/src/firewall.rs apps/rshare-desktop-frontend
git commit -m "fix: make mobile gateway explicitly experimental"
```

### Task 3: Fail closed and defer first-seen QUIC trust

**Files:**
- Modify: `crates/rshare-net/src/encryption.rs`
- Modify: `crates/rshare-net/src/transport.rs`
- Modify: `crates/rshare-net/src/connection.rs`
- Modify: `crates/rshare-net/src/network_manager.rs`
- Test: the same four modules

**Step 1: Write failing trust tests**

Replace the fail-open assertion and add a non-mutating check/commit contract:

```rust
#[test]
fn malformed_trust_store_is_rejected() {
    let dir = temp_dir("quic-trust-malformed");
    let path = dir.join("trust.json");
    fs::create_dir_all(&dir).unwrap();
    fs::write(&path, b"{ not valid json").unwrap();
    assert!(QuicTrustStore::load(&path).is_err());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn checking_first_seen_does_not_mutate_store() {
    let id = DeviceId::new_v4();
    let fingerprint = PeerCertificateFingerprint::from_der(b"cert-a");
    let store = QuicTrustStore::default();
    assert_eq!(store.check(id, &fingerprint), QuicTrustDecision::FirstSeen);
    assert!(store.fingerprint_for(&id).is_none());
}
```

Add an async connection test where HelloBack returns a different ID and assert the connection fails without persisting the expected ID.

**Step 2: Verify RED**

Run: `cargo test -p rshare-net malformed_trust_store_is_rejected checking_first_seen_does_not_mutate_store -- --nocapture`

Expected: FAIL with the current empty-store fallback and mutating `verify_or_trust` API.

**Step 3: Implement trust phases**

- Empty or malformed existing trust files return contextual errors.
- Add `check(device_id, &fingerprint)` and `trust_first_seen(device_id, fingerprint)`.
- Inspect existing pins immediately after TLS; return a pending first-seen fingerprint without saving it.
- Require HelloBack ID equality. Mismatch or unavailable identity closes the connection.
- Commit pending first-seen trust only after equality succeeds.
- Set `NetworkManagerConfig::auto_connect` default to `false` and remove discovery-triggered connection spawning.
- Write trust data through a same-directory temporary file and atomic replacement under a process lock; add a round-trip/replacement test.

**Step 4: Verify GREEN**

Run: `cargo test -p rshare-net -- --nocapture`

Expected: all network tests pass; first-seen mismatch leaves no pin.

**Step 5: Commit**

```bash
git add crates/rshare-net
git commit -m "fix: defer and harden peer trust"
```

### Task 4: Recover canonical connections after transport loss

**Files:**
- Modify: `crates/rshare-net/src/connection.rs`
- Modify: `crates/rshare-net/src/network_manager.rs`
- Test: `crates/rshare-net/src/connection.rs`

**Step 1: Write the reconnect regression test**

Create two local managers, connect them, close the first transport, wait for `Disconnected`, restart the remote endpoint, then reconnect using the same device ID. Assert the second connection succeeds and a late disconnect from the old reader does not hide it.

**Step 2: Verify RED**

Run: `cargo test -p rshare-net reconnects_after_live_transport_closes -- --nocapture`

Expected: FAIL with `Already connected to device ...`.

**Step 3: Implement generation-aware replacement**

- Give each connection a monotonically increasing generation.
- When `connect` finds a canonical entry whose pool connection is closed, remove/close it before inserting the next generation.
- Carry generation on reader disconnect events or suppress disconnects that do not match the live generation.
- Keep `connection_infos`, `is_connected`, and `connected_count` consistent with the canonical state.

**Step 4: Verify GREEN**

Run: `cargo test -p rshare-net connection -- --nocapture`

**Step 5: Commit**

```bash
git add crates/rshare-net/src/connection.rs crates/rshare-net/src/network_manager.rs
git commit -m "fix: recover closed peer connections"
```

### Task 5: Order mobile injections, bound sockets, and recover held input

**Files:**
- Modify: `apps/rshare-daemon/src/mobile_gateway.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `crates/rshare-core/src/ipc.rs`
- Test: `apps/rshare-daemon/src/mobile_gateway.rs`
- Test: `apps/rshare-daemon/src/main.rs`

**Step 1: Write failing protocol and resource tests**

Add tests for:

- a partial HTTP header timing out;
- a configured connection limit refusing an additional socket;
- client sequence 2 (`Released`) preventing late sequence 1 (`Pressed`) from injecting;
- an expired client lease releasing every dynamically tracked key/button;
- TextCommit diagnostics containing `char_count` but no `text` value.

Use a mobile-only envelope:

```rust
#[derive(Debug, Deserialize)]
struct MobileInjectEnvelope {
    client_id: String,
    sequence: u64,
    request: DaemonRequest,
}
```

**Step 2: Verify RED**

Run: `cargo test -p rshare-daemon mobile_gateway -- --nocapture`

Expected: new timeout, ordering, lease, and redaction tests fail.

**Step 3: Implement the server safety layer**

- Limit active client tasks with a Tokio semaphore.
- Apply bounded read/write timeouts and validate `Content-Length` strictly.
- Treat listener accept errors as nonfatal with capped backoff.
- Generate a browser client ID and sequence every inject request.
- Serialize sequence-check and injection per client; reject stale sequence numbers before injection.
- Track dynamically pressed keyboard and mouse controls per client.
- Refresh the held lease from authorized status polling; release tracked input when the lease expires.
- Make pagehide release payloads contain the complete held set.
- Remove raw committed text from every diagnostic payload.

**Step 4: Verify GREEN**

Run: `cargo test -p rshare-daemon mobile_gateway -- --nocapture`

Run: `cargo test -p rshare-daemon inject_endpoint_event_accepts_unicode_text_commit -- --nocapture`

**Step 5: Commit**

```bash
git add apps/rshare-daemon/src/mobile_gateway.rs apps/rshare-daemon/src/main.rs crates/rshare-core/src/ipc.rs
git commit -m "fix: harden mobile input sessions"
```

### Task 6: Fix mobile gesture state, ordering, and accessibility

**Files:**
- Modify: `apps/rshare-desktop-frontend/src/app/mobile-controller.mjs`
- Modify: `apps/rshare-desktop-frontend/src/app/mobile-controller.test.mjs`
- Modify: `apps/rshare-desktop-frontend/src/app/MobileController.tsx`
- Modify: `apps/rshare-daemon/src/mobile_gateway.rs`

**Step 1: Write failing JavaScript behavior tests**

Add real behavior tests rather than source-only assertions:

```javascript
test("ordered request queue never lets release overtake press", async () => {
  const resolvers = [];
  const seen = [];
  const queue = createOrderedMobileRequestQueue(async (value) => {
    seen.push(value);
    await new Promise((resolve) => resolvers.push(resolve));
  });
  const press = queue.enqueue("Pressed");
  const release = queue.enqueue("Released");
  assert.deepEqual(seen, ["Pressed"]);
  resolvers.shift()();
  await Promise.resolve();
  assert.deepEqual(seen, ["Pressed", "Released"]);
  resolvers.shift()();
  await Promise.all([press, release]);
});
```

Also test repeated 2px two-finger moves accumulate into a wheel event and a late poll cannot overwrite an active gesture.

**Step 2: Verify RED**

Run: `node --test src/app/mobile-controller.test.mjs`

Expected: FAIL because no ordered queue/residual/poll freshness helpers exist.

**Step 3: Implement minimal client state fixes**

- Add a single ordered stateful request queue for the React/Tauri mirror.
- Keep only the latest pending pointer move while another move is in flight.
- Accumulate sub-threshold two-finger movement.
- Make status refresh single-flight and ignore stale coordinate updates during active gestures.
- Add keyboard/click activation and `aria-pressed` to held controls while preventing pointer double-fire.
- Mirror the same residual, poll, held-set, client-ID, and sequence rules in the embedded production page.

**Step 4: Verify GREEN**

Run: `npm.cmd test` from `apps/rshare-desktop-frontend`.

Run: `npm.cmd run build`.

**Step 5: Commit**

```bash
git add apps/rshare-desktop-frontend apps/rshare-daemon/src/mobile_gateway.rs
git commit -m "fix: stabilize mobile controller interactions"
```

### Task 7: Correct platform coordinate and text capabilities

**Files:**
- Modify: `crates/rshare-platform/src/windows.rs`
- Modify: `crates/rshare-platform/src/macos.rs`
- Modify: `crates/rshare-input/src/emulator.rs`
- Modify: `crates/rshare-core/src/local_controls.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `apps/rshare-desktop-frontend/src/app/mobile-controller.mjs`
- Test: corresponding Rust and JavaScript modules

**Step 1: Write failing Windows normalization tests**

Extract a pure helper and test negative origins:

```rust
#[test]
fn virtual_desktop_coordinates_preserve_negative_monitor_origins() {
    assert_eq!(
        normalize_virtual_desktop_point(-1920, 120, -1920, 0, 3840, 1080),
        (0, 7288)
    );
}
```

Add a capability serialization test proving unsupported text commit defaults safely for older snapshots.

**Step 2: Verify RED**

Run: `cargo test -p rshare-platform virtual_desktop_coordinates -- --nocapture`

Run: `cargo test -p rshare-core text_commit -- --nocapture`

Expected: FAIL because virtual-screen metrics and capability fields are absent.

**Step 3: Implement platform behavior**

- Read `SM_XVIRTUALSCREEN`, `SM_YVIRTUALSCREEN`, `SM_CXVIRTUALSCREEN`, and `SM_CYVIRTUALSCREEN`.
- Normalize after subtracting the virtual origin and set `MOUSEEVENTF_VIRTUALDESK`.
- Implement macOS Unicode commit using CoreGraphics when available; otherwise report `text_commit_supported=false` rather than accepting then failing.
- Surface the selected backend capability in local controls and disable the mobile text control with an explanation when unsupported.

**Step 4: Verify GREEN**

Run: `cargo test -p rshare-platform --lib -- --nocapture`

Run: `cargo test -p rshare-input --lib -- --nocapture`

Run: `cargo test -p rshare-core -- --nocapture`

Run: `npm.cmd test` from the frontend.

**Step 5: Commit**

```bash
git add crates/rshare-platform crates/rshare-input crates/rshare-core/src/local_controls.rs apps/rshare-daemon/src/main.rs apps/rshare-desktop-frontend
git commit -m "fix: align mobile input with platform capabilities"
```

### Task 8: Isolate virtual-display hardware tests and normalize formatting

**Files:**
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `crates/rshare-platform/tests/virtual_display.rs`
- Modify: `crates/rshare-platform/src/display.rs`
- Modify: `apps/rshare-cli/src/commands/mod.rs`

**Step 1: Add a deterministic failing manager test**

Introduce a fake create callback returning `Created`, call it twice, and assert the manager returns `AlreadyExists` without touching the installed driver. Add an explicit environment-gated real-driver test for manual validation.

**Step 2: Verify the existing failure and RED test**

Run: `cargo test -p rshare-daemon virtual_display_manager_handles_retry_after_platform_create_result -- --nocapture`

Run the new fake test before implementation; expected FAIL until the manager accepts an injected platform operation.

**Step 3: Implement test isolation**

- Route manager unit tests through injected create/remove operations.
- Do not mutate the Windows singleton virtual display from the default automated suite.
- Keep real driver creation/removal behind `RSHARE_RUN_VDISPLAY_DRIVER_TESTS=1` with cleanup on every path.
- Apply `cargo fmt --all` so the two known baseline formatting failures are resolved on the feature branch.

**Step 4: Verify GREEN**

Run: `cargo fmt --all -- --check`

Run: `cargo test -p rshare-daemon -- --nocapture`

Run: `cargo test -p rshare-platform --test virtual_display -- --nocapture`

**Step 5: Commit**

```bash
git add apps/rshare-daemon/src/main.rs crates/rshare-platform/tests/virtual_display.rs crates/rshare-platform/src/display.rs apps/rshare-cli/src/commands/mod.rs
git commit -m "test: isolate virtual display driver state"
```

### Task 9: Full verification, final review, and local merge

**Files:**
- Review: all files changed from `80702af7028ada48d95ee3c81e56337c5cd047a2`
- Preserve: current `main` worktree changes in `crates/rshare-platform/src/display.rs` and `crates/rshare-platform/tests/virtual_display.rs`

**Step 1: Run the complete feature-branch gate**

Run:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets
npm.cmd test
npm.cmd run build
git diff --check 80702af7028ada48d95ee3c81e56337c5cd047a2
```

Expected: tests/build/diff-check exit 0. Clippy may retain documented baseline warnings, but must introduce no new warnings in changed lines.

**Step 2: Request independent review**

Review the full base-to-head diff for security, correctness, platform behavior, test quality, and scope. Fix every Critical/Important issue through a failing regression test and re-review.

**Step 3: Preserve and merge**

- Record the exact dirty `main` diff and status.
- Stash only the two tracked formatting edits; leave the untracked build directory untouched.
- Merge `codex/mobile-controller` into `main` locally without force operations.
- Restore the tracked edits and resolve only by preserving their formatting intent on the merged files.

**Step 4: Verify the merged result**

Run `cargo fmt --all -- --check`, `cargo test --workspace`, frontend tests, and frontend build from the merged `main` worktree.

**Step 5: Clean up**

After verification succeeds, remove the mobile-controller worktree and delete the merged local branch. Do not delete the remote branch unless explicitly requested.
