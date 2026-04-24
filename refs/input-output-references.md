# Input and Output Driver References

This directory stores research notes only. Do not vendor third-party source here.

## Adopted Direction

- Use a Windows KMDF class upper-filter path for driver-level keyboard and mouse capture.
- Use a Windows VHF source-driver path for virtual keyboard and mouse injection.
- Keep the existing user-mode hook, Raw Input enumeration, and SendInput path as fallback when the test driver is not installed.
- Keep gamepad capture through `gilrs`; virtual gamepad support remains behind the VHF/ViGEm research path until a real Windows game-controller validation passes.

## References

| Project or doc | Link | Reference value | Decision |
| --- | --- | --- | --- |
| Deskflow | https://github.com/deskflow/deskflow | Mature keyboard/mouse sharing product shape and cross-platform separation. | Borrow layering and fallback discipline, not source. |
| Barrier | https://github.com/debauchee/barrier | Earlier open source sharing architecture and UI flows. | Use as historical comparison only. |
| Kanata | https://github.com/jtroo/kanata | Keyboard remapping, macro style, platform backends. | Useful for future macro model and config constraints. |
| KMonad | https://github.com/kmonad/kmonad | Keyboard transformation and device-grab concepts. | Useful for macro/remap vocabulary, not a direct Windows driver model. |
| AutoHotInterception | https://github.com/evilC/AutoHotInterception | Per-device keyboard/mouse control through Interception. | Reference for per-device UX and risks of third-party driver dependency. |
| RawInputViewer | https://github.com/EsportToys/RawInputViewer | Raw Input device/event inspection. | Reference for user-mode enumeration and diagnostic tooling. |
| ViGEmBus | https://github.com/nefarius/ViGEmBus | Virtual gamepad bus approach. | Research path for gamepad only; not enabled by default. |
| Microsoft Kbfiltr | https://learn.microsoft.com/en-us/samples/microsoft/windows-driver-samples/keyboard-input-wdf-filter-driver-kbfiltr/ | KMDF keyboard class filter sample. | Primary pattern for `rshare-filter`. |
| Microsoft Moufiltr | https://learn.microsoft.com/en-us/samples/microsoft/windows-driver-samples/mouse-input-wdf-filter-driver-moufiltr/ | KMDF mouse class filter sample. | Primary pattern for mouse packet interception. |
| Microsoft VHF | https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/virtual-hid-framework--vhf- | Virtual HID source driver API. | Primary pattern for `rshare-vhid`. |
| Microsoft Raw Input | https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawinput | User-mode per-device input messages. | Fallback diagnostics and enumeration. |
| Microsoft SendInput | https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput | User-mode injection API. | Fallback injection and comparison path. |
| Microsoft test signing | https://learn.microsoft.com/en-us/windows-hardware/drivers/install/test-signing | Test-signed driver deployment. | Required for local driver validation. |

## Engineering Notes

- The daemon stays authoritative. UI never infers hardware truth from browser APIs.
- The driver ABI is RShare-owned and versioned; Rust and WDK code share the same IOCTL and packet layout.
- Driver install is opt-in and test-signing only for now.
- Driver unavailability must never break existing local or remote device pages.
