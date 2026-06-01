# RShare Windows Driver Workspace

This directory contains RShare-owned Windows driver packages for driver-level local capture, injection, and virtual display experiments.

- `rshare-common/` stores the shared IOCTL ABI used by the drivers and Rust daemon client.
- `rshare-filter/` is the KMDF keyboard/mouse class filter path. It exposes a control device, a synthetic event test IOCTL, and class service callback interception for keyboard/mouse packets. The current INF still targets keyboard devices; mouse-class installation is a separate packaging step.
- `rshare-vhid/` is the VHF virtual HID path for keyboard/mouse reports. Keyboard, relative mouse move, mouse buttons, vertical wheel, and horizontal wheel/pan reports are wired through the shared IOCTL ABI. The virtual gamepad descriptor is intentionally scaffolded only.
- `rshare-vdisplay/` is the UMDF/IddCx virtual display path. It initializes an IddCx adapter, exposes a UMDF/IddCx IOCTL control callback, reports an EDID-backed monitor on create, tracks pending IddCx arrival before the monitor becomes active, supports monitor departure on remove, exposes default/target modes, syncs committed Windows display modes back to driver state, and completes swap-chain frames. The daemon and desktop call the Windows control interface through `rshare-platform` when the installed IDD is present.

Generic USB device forwarding is intentionally not in these drivers yet. It is tracked as an experimental feature and requires a separate host capture layer plus a virtual USB bus/device endpoint, not only HID filter/vhid support.

The drivers are not part of the Cargo workspace. Build them with the scripts under `scripts/driver/` from a Windows Developer Command Prompt with WDK installed. `rshare-vdisplay` additionally requires WDK IddCx headers and libraries (`iddcx.h` and `IddCxStub.lib`); a Windows SDK-only install is not enough.

Before building, run `scripts\driver\check-wdk.ps1`. If it reports missing IddCx components, install the matching WDK package, for example `winget install --id Microsoft.WindowsWDK.10.0.26100 --exact --source winget`, then reopen the shell and run the check again. To check the full virtual-display readiness chain without changing system state, run `scripts\driver\preflight-vdisplay.ps1`, or run `scripts\driver\start-vdisplay-preflight.ps1` from a normal shell to open the same preflight in an elevated PowerShell with a transcript.

Driver installation requires Windows test signing and must be reversible with `uninstall-test-driver.ps1`. On a new test machine, run `scripts\driver\start-vdisplay-validation.ps1 -EnableTestSigning` or `scripts\driver\validate-vdisplay.ps1 -EnableTestSigning` from an elevated PowerShell once; after it enables test signing, reboot Windows and re-run the validation command without that first-run setup step. If the script reports that Secure Boot is enabled, Disable Secure Boot in firmware settings first because Windows blocks `bcdedit /set testsigning on` while Secure Boot policy is active.

For keyboard/mouse driver validation, run `scripts\driver\start-hid-validation.ps1 -EnableInputClassFilters` from a normal shell, or run `scripts\driver\validate-hid.ps1 -EnableInputClassFilters` from an elevated PowerShell. The HID flow builds and installs `rshare-filter` plus `rshare-vhid`, enables the keyboard and mouse class `UpperFilters` entry only when `-EnableInputClassFilters` is supplied, checks `rshare-driver-probe filter status`, checks `rshare-driver-probe vhid status`, runs `rshare-driver-probe vhid inject-smoke`, runs `rshare-driver-probe filter test`, then waits for a real keyboard or mouse hardware event with `rshare-driver-probe filter watch`. Enabling class filters requires a Windows restart or reboot before real hardware capture can be trusted; after reboot, re-run `scripts\driver\validate-hid.ps1 -SkipBuild -SkipInstall` to collect the capture evidence. From the starter script, transcripts are written under `target\driver-validation`.

After WDK and test signing are ready, run `scripts\driver\validate-vdisplay.ps1 -VerifyDaemonDisplayTopology -WaitForManualModeChange` from an elevated PowerShell to build, install, create the virtual display, start the daemon for topology verification, confirm the daemon sees it in the Windows display topology, open Windows display settings, and wait for a manual mode change to be reflected through the driver probe.

From a normal shell, run `scripts\driver\start-vdisplay-validation.ps1` to open the same validation flow in an elevated PowerShell. It writes a transcript under `target\driver-validation` so the Windows Settings and driver-probe evidence can be kept with the build artifacts.

## Virtual Display Manual Validation Target

The full virtual display feature is complete only when this checklist passes on Windows:

1. Build `drivers/windows/rshare-vdisplay/rshare-vdisplay.vcxproj` with the WDK.
2. Install the test-signed IDD driver and confirm Device Manager lists `R-ShareMouse Virtual Display`.
3. Run `target\driver-tools\rshare-driver-probe.exe vdisplay status` and confirm the virtual display driver reports version, capabilities, and inactive state.
4. Run `target\driver-tools\rshare-driver-probe.exe vdisplay create 1920 1080 60000` and wait for the state to move from `Pending` to `Active` if IddCx arrival is still in flight.
5. Confirm Windows Settings > System > Display shows the new `R-SHAREMOUSE` display.
6. Start `rshare-daemon` and use the desktop display settings page to create or refresh the virtual display.
7. Change the virtual display resolution or refresh rate in Windows Settings.
8. Confirm `rshare_platform::display::query_display_state()` and the desktop UI refresh with the changed mode.
9. Run `target\driver-tools\rshare-driver-probe.exe vdisplay remove`, confirm the probe reports `Removed`, then confirm Windows Settings no longer shows it.
