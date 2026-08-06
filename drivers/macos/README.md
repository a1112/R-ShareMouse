# RShare macOS Driver Workspace

This directory contains RShare-owned macOS virtual display driver contracts.

macOS does not expose a public CoreGraphics API that creates an OS-visible virtual display from a normal user-mode process, and the SDK available here does not expose a public DisplayDriverKit family. The RShare daemon therefore treats virtual display support as driver-level capability: the driver must publish an IOKit service, expose an `IOUserClient`, and create or remove the display from the driver side.

- `rshare-vdisplay/` stores the shared ABI for the macOS virtual display user client.
- The service class published to IOKit is `RShareMacVirtualDisplay`.
- User-mode control opens the service with `IOServiceOpen` and calls fixed selectors through `IOConnectCallStructMethod`.
- The Rust daemon never fakes a display when this service is missing. Creation reports `DriverUnavailable`, and list queries preserve the service-open error so an existing Active or Pending display is degraded to `DriverUnavailable` instead of being misreported as removed.

The full macOS validation target is complete only when a signed driver implementing this ABI is installed, approved by macOS security policy, and a created display is visible through the macOS display topology.

For the current source package:

- Run `scripts/driver/check-macos-vdisplay.sh` to verify the IOGraphics/IOUserClient source against the installed macOS SDK.
- Run `scripts/driver/build-macos-vdisplay.sh` to build `target/macos-vdisplay/RShareMacVirtualDisplay.kext`.
- Run `cargo run -p rshare-cli -- display virtual driver-status` to query the platform driver/user-client directly.
- Run `scripts/driver/validate-macos-vdisplay.sh` for non-privileged preflight checks, including `kmutil print-diagnostics -z` dependency diagnostics. User-owned bundle warnings are expected before installation; `sudo scripts/driver/load-macos-vdisplay.sh` installs the signed bundle under `/Library/Extensions`, normalizes that copy to `root:wheel`, and submits the Auxiliary Kernel Collection update with `kmutil load`. The script reports macOS approval-required and reboot-required transitions explicitly.
- Add `--load --verify-daemon-display-topology` after signing. A first installation on macOS 11 or later can require approval and restart; after restart, use `--skip-build --verify-daemon-display-topology` to exercise the full daemon create/verify/remove loop.
- Run `sudo scripts/driver/unload-macos-vdisplay.sh` to remove the installed bundle and submit the required `kmutil rebuild`; restart after approval to boot without the old Auxiliary Kernel Collection.
