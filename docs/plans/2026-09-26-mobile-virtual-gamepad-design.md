# Mobile Virtual HID Gamepad Design

Date: 2026-09-26

## Goal and scope

Add a landscape virtual gamepad to the paired Android controller. Windows and HID/DirectInput applications shall see one R-ShareMouse gamepad with two analog sticks, a D-pad, ABXY, bumpers, triggers, Start/Select, and stick clicks. XInput-only compatibility, rumble, and multiple simultaneous virtual gamepads are outside this increment.

## Options considered

1. **Extend the existing R-ShareMouse VHF driver (selected).** The installed keyboard/mouse virtual HID driver gains a separate gamepad top-level collection and a full-state IOCTL. This reuses the project's driver, signing, and backend selection paths. It requires updating the installed test-signed driver to verify on Windows.
2. Integrate ViGEmBus. It can expose an XInput controller, but its upstream SDK and driver are retired and would add another driver dependency.
3. Integrate HIDMaestro. It targets wider input API coverage, but adds a separate virtual-device platform and deployment surface beyond the requested HID/DirectInput scope.

## Architecture and data flow

The Android shell adds a third mode. It locks gamepad and keyboard modes to landscape, retains the current pairing token and WebView, and switches the served page through a single mode setter. The gamepad page uses independent pointer capture on each stick and button so simultaneous touches work. Stick position is normalized to signed 16-bit axes, triggers to unsigned values, and the D-pad to a hat direction. The UI sends coalesced full-state snapshots on changes plus a short heartbeat while held.

The mobile gateway accepts a typed `GamepadState` endpoint payload with bounded buttons and axis values. It assigns the single virtual pad to one authenticated mobile client at a time. Sequence checks, input validation, and a short lease prevent stale or competing updates. A neutral report is sent on explicit release, mode switch, page hide, disconnect, gateway shutdown, or lease expiry. Existing keyboard and mouse ownership remains unchanged.

The daemon converts the typed endpoint payload to the existing `InputEvent::GamepadState`. The Windows virtual HID inject backend submits one packed gamepad report through a new driver IOCTL. The driver exposes a gamepad HID application collection with a distinct report ID, updates the entire report atomically, and advertises a dedicated gamepad capability flag. The daemon reports a clear unsupported result when the installed driver lacks that capability. Other platforms continue to report gamepad injection as unsupported.

## Error handling and verification

The phone shows whether the desktop can inject a gamepad and does not claim success on an unsupported backend. Transient network errors may drop a state update; the next full snapshot repairs it, and the lease eventually releases any held control. A failed driver submit does not commit a new held state in the gateway.

Tests cover endpoint decoding and bounds, lease ownership and neutralization, HID report packing, and input-backend capability handling. Build the VHF driver with the local WDK, build the Android APK, test landscape multi-touch and cancellation in the emulator, and inspect a running updated driver in `joy.cpl` when installation is available. HID/DirectInput recognition is the acceptance target; XInput-only games are not claimed as supported.
