# RShare Windows Driver Workspace

This directory contains RShare-owned Windows driver scaffolding for driver-level local capture and injection.

- `rshare-common/` stores the shared IOCTL ABI used by the drivers and Rust daemon client.
- `rshare-filter/` is the KMDF keyboard/mouse class filter path. It exposes a control device, a synthetic event test IOCTL, and class service callback interception for keyboard/mouse packets. The current INF still targets keyboard devices; mouse-class installation is a separate packaging step.
- `rshare-vhid/` is the VHF virtual HID path for keyboard/mouse reports. Keyboard, relative mouse move, mouse buttons, vertical wheel, and horizontal wheel/pan reports are wired through the shared IOCTL ABI. The virtual gamepad descriptor is intentionally scaffolded only.

Generic USB device forwarding is intentionally not in these drivers yet. It is tracked as an experimental feature and requires a separate host capture layer plus a virtual USB bus/device endpoint, not only HID filter/vhid support.

The drivers are not part of the Cargo workspace. Build them with the scripts under `scripts/driver/` from a Windows Developer Command Prompt with WDK installed.

Driver installation requires Windows test signing and must be reversible with `uninstall-test-driver.ps1`.
