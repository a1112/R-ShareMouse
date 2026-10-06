# Android Mobile Controller Design

## Goal

Install an APK on the connected Xiaomi 14 Pro and use it to control the Windows host through the existing experimental mobile gateway.

## Approach

Add a small Android application under `apps/rshare-mobile-android`. Its native connection screen accepts the desktop gateway URL and scans the QR code shown in desktop settings. A WebView displays the daemon-served `/mobile` page, so pointer, keyboard, text, and input-release behavior remain owned by the existing mobile implementation.

The app validates the URL before loading it: HTTP, a private LAN IPv4 address, port 27437, `/mobile` path, and a nonempty `t` token. It stores the last successful URL only in app-private storage. No token is compiled into the APK. The app enables cleartext HTTP solely because the current gateway is an explicitly experimental LAN service. The connection screen explains that the same trusted Wi-Fi is required and that the link is unencrypted.

## Alternatives considered

- A fully native controller would improve Android integration but duplicate gesture and input-state logic that the daemon page already implements.
- Reusing the Tauri/React desktop frontend on Android would require mobile packaging and adapting its daemon IPC path. The gateway already serves a standalone phone page, so this adds more work for the first installable build.

## Verification

Build an installable debug APK, install it through ADB on the Xiaomi 14 Pro, launch it, open the current gateway page, and verify page loading plus the connection-screen recovery path. Run focused URL validation tests and the existing mobile-controller tests. Real input control requires a user-visible device interaction and is reported separately from automated installation checks.
