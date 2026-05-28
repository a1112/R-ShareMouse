# RShare Windows Driver Workspace

This directory contains RShare-owned Windows driver scaffolding for driver-level local capture and injection.

- `rshare-common/` stores the shared IOCTL ABI used by the drivers and Rust daemon client.
- `rshare-filter/` is the KMDF keyboard/mouse class filter path. It exposes a control device, a synthetic event test IOCTL, and class service callback interception for keyboard/mouse packets. The current INF still targets keyboard devices; mouse-class installation is a separate packaging step.
- `rshare-vhid/` is the VHF virtual HID path for keyboard/mouse reports. Keyboard, relative mouse move, mouse buttons, vertical wheel, and horizontal wheel/pan reports are wired through the shared IOCTL ABI. The virtual gamepad descriptor is intentionally scaffolded only.
- `rshare-vdisplay/` is the planned UMDF/IddCx virtual display path. It is currently a scaffold only; it does not yet register an IDD adapter or create a system-visible monitor.

Generic USB device forwarding is intentionally not in these drivers yet. It is tracked as an experimental feature and requires a separate host capture layer plus a virtual USB bus/device endpoint, not only HID filter/vhid support.

The drivers are not part of the Cargo workspace. Build them with the scripts under `scripts/driver/` from a Windows Developer Command Prompt with WDK installed.

Driver installation requires Windows test signing and must be reversible with `uninstall-test-driver.ps1`.

## Virtual Display Manual Validation Target

The full virtual display feature is complete only when this checklist passes on Windows:

1. Build `drivers/windows/rshare-vdisplay/rshare-vdisplay.vcxproj` with the WDK.
2. Install the test-signed IDD driver and confirm Device Manager lists `R-ShareMouse Virtual Display`.
3. Start `rshare-daemon`.
4. Use the desktop display settings page to create a virtual display.
5. Confirm Windows Settings > System > Display shows the new display.
6. Change the virtual display resolution or refresh rate in Windows Settings.
7. Confirm `rshare_platform::display::query_display_state()` and the desktop UI refresh with the changed mode.
8. Remove the virtual display from the desktop UI and confirm Windows Settings no longer shows it.
