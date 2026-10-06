# Android Mobile Controller Implementation Plan

**Goal:** Build and install an Android APK that opens the existing LAN mobile controller on a Xiaomi 14 Pro.

**Architecture:** A native Android connection screen scans or accepts a gateway URL. A WebView loads the daemon-served controller page. The daemon retains all input and session state.

**Tech Stack:** Java, Android SDK, Gradle, ZXing Android Embedded, existing Rust mobile gateway.

---

### Task 1: Project and connection contract

**Files:** `apps/rshare-mobile-android/settings.gradle`, `apps/rshare-mobile-android/build.gradle`, `apps/rshare-mobile-android/app/build.gradle`, `apps/rshare-mobile-android/app/src/main/java/org/rsharemouse/mobile/MobileUrl.java`, `apps/rshare-mobile-android/app/src/test/java/org/rsharemouse/mobile/MobileUrlTest.java`.

1. Add failing tests for valid private-LAN gateway URLs and rejection of public hosts, wrong paths, wrong ports, missing tokens, and non-HTTP schemes.
2. Run the focused unit test and confirm the expected failure.
3. Add the minimal Android project and `MobileUrl` validator.
4. Run the unit test and confirm it passes.

### Task 2: Connection screen and controller WebView

**Files:** `apps/rshare-mobile-android/app/src/main/AndroidManifest.xml`, `apps/rshare-mobile-android/app/src/main/java/org/rsharemouse/mobile/MainActivity.java`, Android resources.

1. Add an instrumented smoke test that launches the connection screen.
2. Build and run the test to confirm the missing screen fails.
3. Implement URL entry, QR scanning, WebView loading, back navigation, error display, and page lifecycle input release.
4. Run the screen smoke test and debug APK build.

### Task 3: Real-device deployment

**Files:** `apps/rshare-mobile-android/README.md`, generated debug APK.

1. Confirm the daemon binds port 27437 and serves `/mobile` with an authorized token.
2. Install the APK on the connected Xiaomi 14 Pro with ADB.
3. Launch the app and verify the mobile page loads over the LAN or ADB reverse connection.
4. Record the APK location and any hardware interaction still requiring the user's confirmation.
