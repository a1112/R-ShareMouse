# Extended Display Implementation Plan

> Execute sequentially using the executing-plans and test-driven-development skills.

**Goal:** Deliver an integrated experimental Windows extended display with browser
receivers, a low-latency WebRTC path and native Windows multi-touch over LAN or an
available USB network link, with explicit direct-iPad-USB limitations.

**Architecture:** Daemon-owned single session and OS display; authenticated bounded
WebSocket signaling/input; loopback browser capture; direct WebRTC media.

**Tech Stack:** Rust/Tokio, existing IDD backend, Windows touch injection, browser
WebRTC/Pointer Events, Node test runner.

### Task 1: Touch contract and Windows backend

Create `crates/rshare-platform/src/remote_touch.rs`, export from `src/lib.rs`, add
Windows pointer feature to Cargo.toml. Add pure state-machine tests first: reject
invalid contacts, map negative origins, maintain all active contacts, up uses last
injected location, cancel all, no state commit on backend failure. Run
`cargo test -p rshare-platform remote_touch`; implement and repeat.

### Task 2: Browser receiver/sender

Create `apps/rshare-daemon/src/extended-display/{client.mjs,page.html}` and
`apps/rshare-desktop-frontend/src/app/extended-display.test.mjs`. Test aspect-fit,
pointer snapshots and codec/transport policy with `node --test` before implementation.
Add browser capture consent, H264 preference, bounded input, inline full-screen
receiver, statistics, errors and disconnect handling. Do not claim hardware
acceleration or USB routing from requested settings alone.

### Task 3: Daemon-owned session and gateway

Create `apps/rshare-daemon/src/extended_display.rs`; wire `main.rs` and
`mobile_gateway.rs`. Add tests for role validation, message bounds, signaling and
cleanup; run `cargo test -p rshare-daemon extended_display`. Add a loopback-only
sender, one authenticated receiver, virtual display create/remove with truthful
enumeration, periodic geometry/lease checks and touch cancellation. Integrate
entry links in the mobile page and desktop mobile settings.

### Task 4: Verify and document

Run targeted tests then `cargo check -p rshare-daemon`, frontend tests and
`npm run build --prefix apps/rshare-desktop-frontend`. Add usage/acceptance docs with
USB prerequisites and manual Windows/iPad checks. Review diff against existing
uncommitted work. Do not present unperformed physical acceptance as passing.

## Execution record

- Implemented all four code tasks in the existing checkout on
  `codex/extended-display-touch`, preserving pre-existing uncommitted work.
- Platform touch/lease tests: 4 passed; existing virtual-display tests: 5 passed
  (real driver test remains opt-in and did not run a physical create/remove cycle).
- Extended display daemon tests: 7 passed, including a real TCP/WebSocket relay,
  authentication/role rejection, generation revocation and shutdown cleanup.
- Mobile gateway regression suite: 81 passed. Frontend suite: 281 passed.
- `cargo check -p rshare-daemon --locked` and frontend production build passed.
  Existing unused-code warnings and Vite chunk-size warning remain.
- Edge integration passed actual H.264 encode/decode, receiver playback, full
  contact snapshots, disconnect and cancelled-recapture touch revocation. Capture
  source and daemon signaling were simulated; physical IDD/iPad were not tested.
- Static review identified recapture retaining touch consent; reproduced with a
  failing browser test, then fixed and verified.
- Open acceptance/development work: iPad/device-specific validation, measured
  glass-to-glass latency and native cable-only iPad USB receiver/bridge.
