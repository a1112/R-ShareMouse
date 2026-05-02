# HID driver loop and experimental USB forwarding

## Priority

Stabilize the HID loop first:

1. KMDF filter captures hardware keyboard/mouse packets from class service callbacks.
2. The Rust driver client reads normalized driver events from the control device.
3. The daemon routes only hardware events and ignores injected loopback.
4. The VHF driver injects keyboard, relative mouse, button, and wheel reports on the remote side.

The generic USB forwarding path is experimental. It should not block the HID loop.

## HID closure

Current Windows low-level path:

```text
hardware keyboard/mouse
  -> rshare-filter class callback
  -> IOCTL_RSHARE_READ_EVENT
  -> VirtualHidCaptureBackend
  -> daemon routing
  -> QUIC Message::Key/Mouse*
  -> VirtualHidInjectionBackend
  -> rshare-vhid VHF report
```

Manual validation should verify:

- real keyboard scan codes become normalized key events
- mouse button bitmasks become canonical button press/release events
- vertical and horizontal wheel packets become mouse wheel events
- injected loopback is not forwarded back into the network
- driver queue overflow drops oldest events instead of blocking the input callback

## Experimental USB forwarding boundary

The protocol now has capability and message types for USB-over-IP style forwarding:

- `supports_usb_forwarding_experimental`
- `usb_forwarding`
- `UsbDeviceAttached`
- `UsbDeviceDetached`
- `UsbDeviceClaimRequest`
- `UsbDeviceClaimResponse`
- `UsbDeviceRelease`
- `UsbDeviceReset`
- `UsbTransfer`
- `UsbTransferComplete`
- `UsbTransferCancel`
- `UsbFlowControl`
- `UsbForwardingError`

The metadata model can describe USB speed, active configuration, configurations, interfaces, alternate settings, endpoints, structured control setup packets, isochronous packet descriptors, transfer flags, typed completion status, and receiver-side flow control windows.

The intended experimental control flow is:

```text
host advertises UsbDeviceAttached
  -> receiver sends UsbDeviceClaimRequest
  -> host replies UsbDeviceClaimResponse with session_id
  -> receiver and host exchange UsbFlowControl
  -> UsbTransfer / UsbTransferComplete carry endpoint traffic
  -> UsbTransferCancel, UsbDeviceReset, UsbDeviceRelease handle teardown and recovery
```

The daemon now binds this protocol to a real Windows host-side runtime for WinUSB-compatible devices:

1. local IPC and local-control snapshots can list claimable USB device interfaces
2. `UsbDeviceClaimRequest` opens the requested device interface with `CreateFileW` and `WinUsb_Initialize`
3. endpoint metadata is queried with `WinUsb_QueryInterfaceSettings` / `WinUsb_QueryPipe`
4. `UsbTransfer` executes synchronous control, bulk, and interrupt transfers with WinUSB
5. `UsbTransferComplete`, `UsbForwardingError`, `UsbFlowControl`, reset, cancel, and release are wired through the daemon

USB host enumeration now attempts to enrich each WinUSB-compatible device with real standard descriptors:

- device descriptor fields: VID/PID, class/subclass/protocol, USB BCD, device BCD
- string descriptors: manufacturer/product/serial when readable
- configuration, interface, and endpoint descriptors, including endpoint direction/type/packet size/interval
- active configuration when the device answers `GET_CONFIGURATION`

The first transfer-level two-machine probe is also wired:

```text
peer connects
  -> host advertises local WinUSB-compatible devices with UsbDeviceAttached
  -> `rshare usb list` shows remote devices
  -> `rshare usb probe <device_id> <bus_id>` sends UsbDeviceClaimRequest
  -> host claims the device and executes a GET_DESCRIPTOR(Device) control transfer
  -> receiver reports parsed VID/PID/class plus raw descriptor bytes
```

This probe is intentionally narrow. It validates real USB control traffic over the R-ShareMouse network path without pretending that a full virtual USB bus exists yet.

This is a real host-side USB transfer loop, not only a serialization contract. A complete end-to-end generic USB implementation still needs:

1. receiver-side virtual USB bus or UDE-based device surface
2. asynchronous transfer scheduler and stricter per-transfer timeout handling
3. isochronous transfer support
4. per-device allowlist and explicit user confirmation
5. hotplug attach/detach advertisement from the host runtime
6. reconnect semantics across session/device loss

HID keyboard/mouse sharing should continue through the dedicated HID path. Generic USB forwarding is for devices that cannot be represented safely as standard input/audio/gamepad events.
