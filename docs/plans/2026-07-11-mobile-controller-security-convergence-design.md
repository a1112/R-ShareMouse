# Mobile Controller Security Convergence Design

**Date:** 2026-07-11
**Status:** Approved

## Context

The `codex/mobile-controller` branch adds a LAN-served mobile keyboard and pointer controller, automatic peer connection, text commit support, and desktop access controls. Review found that the branch is not safe to merge as written: unpaired discovery can establish trusted QUIC sessions, a malformed trust store fails open, the mobile credential travels over plaintext HTTP, stateful input requests can be reordered, closed connections cannot reconnect, and several platform and lifecycle paths can leave input stuck or unusable.

The approved outcome is a safe convergence merge. The branch will retain the mobile controller as an explicit experimental capability, but unsafe LAN exposure and unpaired automatic connection will not be enabled by default.

## Goals

- Preserve explicit user safety choices across upgrades.
- Require an intentional peer connection before first-seen QUIC trust is persisted.
- Fail closed when persisted trust data is corrupt.
- Make disconnect/reconnect state canonical and race-safe.
- Keep stateful mobile input ordered and releasable after page or network failure.
- Bound unauthenticated gateway resource use.
- Avoid retaining committed text in diagnostic history.
- Correct multi-display pointer behavior and platform capability reporting.
- Make the automated suite deterministic without mutating a real virtual display driver by default.
- Merge the reviewed branch into local `main` without losing the user's existing uncommitted formatting changes.

## Non-goals

- Claiming that plaintext LAN HTTP is secure.
- Shipping an installable PWA or Wake Lock support over an insecure private-network origin.
- Building a public certificate service, relay, or native mobile application in this change.
- Automatically trusting every discovered peer.

## Security Defaults

`FeatureConfig` gains an explicit `mobile_gateway_enabled` flag that defaults to `false`. Missing fields in existing configuration also resolve to `false`. When disabled, the daemon does not bind the LAN mobile port and the desktop reports that the experimental gateway is disabled without exposing a token.

When explicitly enabled, the desktop clearly labels the gateway as experimental plaintext LAN control. The HTTP version does not advertise PWA installation or Wake Lock. The access token remains ephemeral so restarting the daemon invalidates old QR links; users must rescan after restart. The Windows TCP firewall rule is installed only when the gateway is explicitly enabled and is included in firewall status and cleanup.

`automatic_input_forwarding=false` is always preserved when it is explicitly present. New configurations may still default forwarding to enabled, but value-based migration heuristics will not reinterpret a saved `false` as an old default.

## Peer Trust and Connection Lifecycle

Discovery remains observational. `NetworkManagerConfig::auto_connect` defaults to `false`; a newly discovered device cannot trigger a connection or layout mutation until an explicit connect request is made. Future trusted-peer auto-reconnect can be added only from an allowlist, not from discovery alone.

Outbound QUIC trust is split into two phases:

1. Immediately after TLS, compare the presented certificate with any existing pin. A mismatch closes the connection.
2. For a first-seen certificate, complete the application Hello/HelloBack exchange and require the returned device ID to equal the requested ID. Only then persist the first-seen pin. A missing or mismatched identity closes the connection without changing trust state.

Trust-store parsing is fail-closed for every existing empty or malformed file. Writes use a same-directory temporary file and atomic replacement under a process lock so a partial write cannot silently erase all pins.

Before connecting, the connection manager checks the live pool. A canonical entry whose transport is closed is removed and may be replaced. Disconnect notifications are checked against the live connection generation so a late reader from an old connection cannot mark a newer connection disconnected.

## Mobile Gateway and Input Safety

The gateway has a fixed upper bound on concurrent unauthenticated sockets. Header/body reads and response writes have deadlines. Accept failures degrade the optional gateway with logged backoff rather than terminating the daemon.

Each browser session receives a random client ID and monotonically increasing sequence. Mobile inject requests use an envelope containing the client ID, sequence, and existing endpoint-inject request. The server serializes check-and-inject for each client and rejects stale sequences. This guarantees that a late `Pressed` cannot be applied after a newer `Released`, and that stale absolute pointer moves cannot overwrite newer positions. The React/Tauri mirror uses a single ordered request queue for the same stateful semantics.

The server tracks the keys and mouse buttons held by each client. Authorized polling acts as the client heartbeat. Page hide sends the complete tracked release set with keepalive; if heartbeats stop while inputs remain held, a bounded lease expires and the daemon injects releases. Release operations are idempotent.

Committed text is used only for transient injection. Diagnostic records contain event type and character count, never the original text.

## Client and Platform Correctness

- Two-finger scrolling accumulates sub-threshold movement instead of resetting its baseline on every event.
- Status polling is single-flight and cannot overwrite pointer coordinates while a gesture is active or apply an older response after a newer one.
- Held controls expose keyboard/click semantics and `aria-pressed` state without double-firing pointer input.
- Windows absolute input uses virtual-screen origin and dimensions plus `MOUSEEVENTF_VIRTUALDESK`, so negative and secondary-monitor coordinates remain valid.
- Text commit is exposed only when the selected daemon backend reports support. macOS native injection either implements Unicode commit or reports the capability unavailable and disables the control.

## Testing Strategy

Every behavior change follows red-green-refactor:

- config tests preserve an explicit forwarding opt-out and default the mobile gateway off;
- trust tests reject corrupt stores, defer first-seen persistence, and reject Hello identity mismatch;
- connection tests reproduce closed-transport reconnect and stale-reader ordering;
- gateway tests cover timeouts, connection bounds, stale sequences, held-input lease release, and diagnostic redaction;
- JavaScript tests use delayed promises to prove press/release ordering, scroll accumulation, and polling freshness;
- Windows unit tests cover virtual desktop normalization including negative origins;
- platform-mutating virtual-display tests use injected fakes by default, with real driver validation kept as an explicit manual test.

The final gate is `cargo fmt --all -- --check`, targeted Rust and JavaScript tests after each task, `cargo test --workspace`, frontend tests and production build, and a final independent diff review. Tests are rerun after merging into `main`.

## Integration

Implementation stays on the existing isolated `codex/mobile-controller` worktree. After all checks pass, the current `main` worktree's two known formatting edits are temporarily preserved, the reviewed branch is merged locally, the edits are restored, and the merged result is tested again. The feature worktree and merged branch are removed only after successful verification.
