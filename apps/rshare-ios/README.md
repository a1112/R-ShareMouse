# R-ShareMouse iOS

Native SwiftUI connection/settings/navigation with a WKWebView control surface
served by the authoritative desktop daemon. Supports iPhone and iPad on iOS 17+.
The desktop Rust workspace is not embedded in the phone app.

## Interface and capabilities

- Connection page: paste the desktop's authenticated mobile URL; reconnect to the
  last paired computer. Credentials are stored in the device-only Keychain,
  never in UserDefaults, screenshot labels, analytics, or committed configuration.
- Remote: the existing daemon touchpad, mouse buttons, scrolling, shortcuts,
  modifiers and text input. Actual backend status remains daemon-owned.
  Current hosts use an authenticated persistent WebSocket with up to four
  unacknowledged events and coalesced pending pointer positions. Button/key
  confirmation still waits for actual backend results; old hosts fall back to
  HTTP. Fractional pointer deltas are retained for slow, precise movement.
- Mac extended display: select “连接 Mac 附屏”, then USB or LAN / Wi-Fi.
  USB exposes a loopback-only native H.264 receiver to Apple's usbmuxd and uses a
  six-digit pairing code. LAN exposes a TLS-PSK receiver on port 27439 and generates
  a fresh 256-bit key in a user-copied pairing link. Neither needs personal hotspot.
  A single native decoder presents the Mac's independent virtual screen, supports
  fullscreen/rotation, stays awake while receiving or waiting, and stops on background.
  See `docs/extended-display.md` for desktop setup and acceptance limitations.
- Browser extended display: loads `/display` as a receiver. Requires a Windows host
  with the virtual display driver and a host browser sharing that screen.
  Pasting a display URL opens this tab directly. Playback supports native immersive
  fullscreen, device rotation, idle prevention, bounded reconnect and manual retry.
  Leaving playback releases the idle timer; returning from the background reconnects
  without retaining touch consent. The host must enable touch again.
- Settings: system/light/dark appearance, connection status, disconnect and forget.
- Backgrounding/tab changes dispatch the page's input-release lifecycle events.
  Extended-display sessions are reloaded after suspension because their pagehide
  handler closes the signaling socket. The daemon's held-input expiry remains
  the final release boundary if the OS kills the app before it can send.

WindowBase references: `apps/tauri/src/styles.css` (mobile safe areas and adaptive
shell), `src-tauri/Info.ios.plist` (iOS container configuration). This implementation
uses SwiftUI native navigation/material and WKWebView rather than importing the
desktop-only window/tray implementation. The WindowBase checkout is unchanged.

## Pair

Enable `features.mobile_gateway_enabled = true` in the computer's R-ShareMouse
configuration and restart its service. From desktop Settings → Mobile control,
copy its complete `/mobile?t=...` link to the iOS connection page. Both devices
must be on the same LAN. The experimental desktop gateway currently uses HTTP;
use it only on a trusted LAN. Public hosts, malformed links, changed origins and
host-mode display navigation are rejected by the app.

The app requests no camera or microphone access. Copy/paste uses the system
Paste button. The local HTTP exception is limited to web content/local networking.

## Build and install

Requirements: Xcode, an iOS runtime/device, XcodeGen, and an Apple signing team
configured in Xcode. The team is supplied at build time, not stored in the project.

```sh
xcodegen generate --spec apps/rshare-ios/project.yml
RSHARE_IOS_TEAM=YOUR_TEAM scripts/install-ios.sh YOUR_DEVICE_UDID
```

The script builds a development-signed app, installs it on the selected device,
and launches it. It does not publish to TestFlight or the App Store.

For tests, choose an installed simulator UUID from `xcrun simctl list devices`:

```sh
xcodebuild -project apps/rshare-ios/RShareMouse.xcodeproj -scheme RShareMouse \
  -destination 'platform=iOS Simulator,id=SIMULATOR_UUID' \
  -derivedDataPath target/ios-simulator CODE_SIGNING_ALLOWED=NO test
```

`LiveClientUITests` is an opt-in LAN check. On a physical device it uses an already
paired phone. On a simulator it requires `RSHARE_LIVE_PAIRING_URL` in the test
runner's environment and uses nonpersistent test pairing. Never commit that
credential. It verifies connection and receiver-page navigation; it does not
claim physical touch injection or video from an installed Windows display driver.

For real WebRTC decode and mobile lifecycle tests with a generated video source:

```sh
RSHARE_IOS_SIMULATOR=SIMULATOR_UUID \
  node apps/rshare-desktop-frontend/tests/mobile-display-ios.mjs
```

This requires Chrome and the frontend's installed Playwright dependency. The
runner starts loopback-only test signaling, exercises native fullscreen/rotation,
reconnect and foreground recovery, verifies screenshot coverage, and retains
XCTest evidence under `target/mobile-display-validation/`.

Native video receiver regression (generated Mac VideoToolbox frames; simulator):

```sh
RSHARE_IOS_SIMULATOR=SIMULATOR_UUID python3 scripts/test-ios-usb-receiver.py
RSHARE_IOS_SIMULATOR=SIMULATOR_UUID RSHARE_DISPLAY_TEST_TRANSPORT=lan \
  python3 scripts/test-ios-usb-receiver.py
```

LAN tests additionally require Python 3.13+ with TLS-PSK support. These tests check
real decoding, authentication failures and UI lifecycle, but do not prove physical
USB or Wi-Fi transport. Current evidence: `docs/testing/mac-display-2026-09-30.md`.
