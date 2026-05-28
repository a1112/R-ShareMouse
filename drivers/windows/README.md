# RShare Windows Driver Workspace

This directory contains RShare-owned Windows driver packages for driver-level local capture, injection, and virtual display experiments.

- `rshare-common/` stores the shared IOCTL ABI used by the drivers and Rust daemon client.
- `rshare-filter/` is the KMDF keyboard/mouse class filter path. It exposes a control device, a synthetic event test IOCTL, and class service callback interception for keyboard/mouse packets. The current INF still targets keyboard devices; mouse-class installation is a separate packaging step.
- `rshare-vhid/` is the VHF virtual HID path for keyboard/mouse reports. Keyboard, relative mouse move, mouse buttons, vertical wheel, and horizontal wheel/pan reports are wired through the shared IOCTL ABI. The virtual gamepad descriptor is intentionally scaffolded only.
- `rshare-vdisplay/` is the UMDF/IddCx virtual display path. It initializes an IddCx adapter, exposes a UMDF/IddCx IOCTL control callback, reports an EDID-less monitor on create, supports monitor departure on remove, exposes default/target modes, and completes swap-chain frames. The daemon still needs a Windows user-mode client for that control interface before desktop create/remove can drive the installed IDD.

Generic USB device forwarding is intentionally not in these drivers yet. It is tracked as an experimental feature and requires a separate host capture layer plus a virtual USB bus/device endpoint, not only HID filter/vhid support.

The drivers are not part of the Cargo workspace. Build them with the scripts under `scripts/driver/` from a Windows Developer Command Prompt with WDK installed. `rshare-vdisplay` additionally requires WDK IddCx headers and libraries (`iddcx.h` and `IddCxStub.lib`); a Windows SDK-only install is not enough.

Driver installation requires Windows test signing and must be reversible with `uninstall-test-driver.ps1`.

## Virtual Display Manual Validation Target

The full virtual display feature is complete only when this checklist passes on Windows:

1. Build `drivers/windows/rshare-vdisplay/rshare-vdisplay.vcxproj` with the WDK.
2. Install the test-signed IDD driver and confirm Device Manager lists `R-ShareMouse Virtual Display`.
3. Run `target\driver-tools\rshare-driver-probe.exe vdisplay status` and confirm the virtual display driver reports version, capabilities, and inactive state.
4. Run `target\driver-tools\rshare-driver-probe.exe vdisplay create 1920 1080 60000`.
5. Confirm Windows Settings > System > Display shows the new display.
6. Start `rshare-daemon` and use the desktop display settings page to create or refresh the virtual display.
7. Change the virtual display resolution or refresh rate in Windows Settings.
8. Confirm `rshare_platform::display::query_display_state()` and the desktop UI refresh with the changed mode.
9. Run `target\driver-tools\rshare-driver-probe.exe vdisplay remove`, then confirm Windows Settings no longer shows it.
