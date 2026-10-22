# iOS validation — 2026-09-29

## 2026-09-30 native Mac display update

Native USB and TLS-encrypted LAN / Wi-Fi receivers now share VideoToolbox decoding
and fullscreen presentation. Mac virtual-screen creation, actual H.264 capture,
removal and recreation passed real-system integration tests. The device app was
built and installed. Current acceptance and remaining physical USB/LAN checks are
recorded in `docs/testing/mac-display-2026-09-30.md`; the older entries below describe
the browser path at their original date.

## Mobile secondary display follow-up

- Implemented direct display-link entry, native immersive fullscreen and rotation,
  playback idle prevention, bounded reconnect and background/foreground recovery.
- Dedicated iPhone 17 and iPad A16 simulators both passed real Chrome-to-WKWebView
  video decode, fullscreen/exit, landscape layout, manual reconnect and foreground
  recovery. Screenshots additionally verify fitted test-video coverage.
- Six unit tests and two connection/offline UI tests passed. Pairing lifecycle
  unit tests now explicitly disable persistent credentials.
- The generated video and signaling fixture do not validate a Windows virtual
  monitor or physical phone/iPad input. No new physical-device install was made.
- Full commands, current results and evidence paths:
  `docs/testing/mobile-display-2026-09-29.md`.

## Mouse smoothness follow-up

- Replaced per-event HTTP connections with an authenticated, same-origin
  WebSocket. Motion can pipeline up to four unacknowledged events; pending motion
  still coalesces and button/key state waits for the actual backend result.
- Idle heartbeats retain the connection. Suspension cancels unsent events and
  keeps the existing release beacon/sequence fencing. Ambiguous delivery is
  never replayed through HTTP. Older hosts retain the HTTP fallback.
- Fractional touch deltas accumulate instead of being lost to per-event rounding.
  Successful motion no longer rewrites the native status label on every event.
- 80 mobile-gateway Rust tests and 280 frontend tests passed. The live iOS
  simulator connection/keyboard/display-navigation test passed against the
  updated Mac service (`target/mobile-smoothness/iOSTests.xcresult`).
- 40 real acknowledged injections over one browser WebSocket completed without
  page errors. Same-Mac median was approximately 6.8 ms; this is not a phone Wi-Fi
  latency or a measured physical smoothness improvement. The earlier HTTP probe
  used a different client and is not a controlled performance comparison.
- The iPhone app was relaunched with refreshed pairing, and a persistent LAN
  connection was observed. Physical gesture smoothness still needs user feedback.

## Verified

- Development-signed Debug app built with Xcode 27 and installed/launched on an
  iPhone 16 Pro Max running iOS 27.0.1. The real phone displayed `已连接` to the
  live Mac gateway. Latest install: `target/ios-validation/install-final.log`.
- Updated Mac daemon built in release mode, retained the existing Apple signing
  identity and passed strict signature verification. Live IPC reported
  `Healthy`, `LocalReady`, input monitoring/accessibility/ready all true.
- Authenticated `/mobile` and `/display` both returned HTTP 200.
- Four pairing unit tests and two connection/offline UI tests passed on iPhone
  simulator: `target/ios-validation/SimulatorTestsFinal.xcresult`.
- A simulator connected to the real gateway, showed `已连接`, scrolled to the
  keyboard/send controls, opened the display receiver, then returned to a
  connected touchpad: `target/ios-validation/LiveKeyboardTestsFinal.xcresult`.
  The test runner was reinstalled before this final run to avoid stale simulator
  test code. Screenshots are in `target/ios-validation/final-attachments/`.
- Frontend: 275 tests passed, including a regression exercising both sensitivity
  implementations; frontend build and release daemon build passed.
- First-run sensitivity is 1.35 rather than accidentally clamping absent storage
  to 0.50. Explicit user-selected values remain intact.
- WindowBase checkout was read for native containment, safe areas and appearance
  reference, and was not modified. This repository still has only `main` locally
  and `origin/main` remotely.

## Remaining acceptance

- The physical-device XCTest runner could not install because the device already
  had the maximum free-development-profile apps. The app itself installed and
  connected. No other phone apps were removed.
- The live UI test checks connection/navigation and keyboard visibility. Physical
  gesture injection, Chinese IME/text entry, background held-key release,
  iPad hardware/orientation and sustained latency still need device acceptance.
- The display receiver correctly waits for a host stream. Windows virtual-driver
  installation, actual extended-screen video and touch injection were not tested
  in that earlier Mac-only run. The native Mac display implementation was added
  on 2026-09-30; see the newer validation record above.
- The earlier full Rust workspace run had one pre-existing macOS virtual-display
  package contract failure; this iOS work does not implement that missing driver
  or claim a fully green workspace.

Artifacts under `target/` are local evidence, not committed source. Do not publish
pairing URLs, private test-run environment files, or credentials from local logs.
