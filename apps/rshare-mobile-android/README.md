# Android mobile controller

This APK wraps the daemon's experimental LAN mobile page. It does not run the R-ShareMouse daemon on Android.

## Build

Use JDK 17 and an Android SDK with API 36 installed. Set `ANDROID_HOME` to the SDK directory, then run from this folder:

```powershell
.\gradlew.bat :app:testDebugUnitTest :app:assembleDebug
```

The installable debug APK is `app/build/outputs/apk/debug/app-debug.apk`.

The 0.3.0 UI uses the approved `docs/design/mobile-discovery-keyboard-concept.png` design: a discovery card with desktop confirmation, a compact manual connection disclosure, and a landscape keyboard.

With USB debugging enabled, install it using:

```powershell
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

## Connect

1. On the Windows host, add `[features]` with `mobile_gateway_enabled = true` to `%APPDATA%\rshare\config.toml`, then restart the daemon.
2. Install the APK on the phone and connect it to a trusted LAN that can reach the computer. The app scans the local /24 first. On 192.168 networks it also tries the common routed home subnets 192.168.1/24, 192.168.0/24, and 192.168.10/24.
3. Select the computer and tap **请求配对**. In desktop settings → **移动端控制**, confirm the pending phone. The controller opens automatically after approval.
4. Tap **独立键盘** for landscape QWERTY mode, or **模拟手柄** for a landscape HID/DirectInput gamepad. Tap **触控板** to return.

The gamepad exposes dual sticks, D-pad, ABXY, bumpers, digital triggers, Start/Select, Home, and stick clicks. It requires the updated R-ShareMouse Windows VHF driver with the virtual gamepad capability; replacing a running driver may require a Windows reboot. Windows game controllers and HID/DirectInput games can see it; XInput-only games cannot. Held controls return to neutral when the screen closes, the connection drops, or the mobile lease expires.

The app can also scan the desktop QR code, receive a shared link, or accept the URL manually. It accepts only the daemon's `http://<private IPv4>:27437/mobile?t=...` address. The current gateway sends the access token over unencrypted HTTP, so use it only on a trusted LAN. The token changes after a daemon restart; use desktop confirmation again to reconnect.

## Local emulator

An API 36 x86_64 emulator can reach the computer gateway through ADB reverse mapping:

```powershell
adb -s emulator-5554 reverse tcp:27437 tcp:27437
adb -s emulator-5554 install -r app/build/outputs/apk/debug/app-debug.apk
adb -s emulator-5554 install -r app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
adb -s emulator-5554 shell am instrument -w org.rsharemouse.mobile.test/androidx.test.runner.AndroidJUnitRunner
```

Start the desktop daemon before opening the app. Discovery will find the gateway on the emulator's own virtual subnet address, and pairing still requires approval through the desktop app.

On Xiaomi devices, ADB installation can require **USB 安装** in Developer options and a confirmation on the phone. If ADB installation is blocked, copy the APK to the phone's Download folder and open it with the system file manager.
