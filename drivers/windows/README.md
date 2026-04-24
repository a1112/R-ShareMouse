# RShare Windows Driver Workspace

This directory contains RShare-owned Windows driver scaffolding for driver-level local capture and injection.

- `rshare-common/` stores the shared IOCTL ABI used by the drivers and Rust daemon client.
- `rshare-filter/` is the KMDF keyboard/mouse class filter path. The first implementation exposes a control device and synthetic event test IOCTL; keyboard/mouse packet interception is the next WDK step.
- `rshare-vhid/` is the VHF virtual HID path for keyboard/mouse reports. The virtual gamepad descriptor is intentionally scaffolded only.

The drivers are not part of the Cargo workspace. Build them with the scripts under `scripts/driver/` from a Windows Developer Command Prompt with WDK installed.

Driver installation requires Windows test signing and must be reversible with `uninstall-test-driver.ps1`.
