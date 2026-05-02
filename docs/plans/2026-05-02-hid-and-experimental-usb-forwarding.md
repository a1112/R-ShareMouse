# HID driver loop and experimental USB forwarding

## Priority

Stabilize the HID loop first:

1. KMDF filter captures hardware keyboard/mouse packets from class service callbacks.
2. The Rust driver client reads normalized driver events from the control device.
3. The daemon routes only hardware events and ignores injected loopback.
4. The VHF driver injects keyboard, relative mouse, button, and wheel reports on the remote side.

The generic USB forwarding path is experimental and disabled by default. It should not block the HID loop.

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
- `UsbDeviceAttached`
- `UsbDeviceDetached`
- `UsbTransfer`
- `UsbTransferComplete`
- `UsbForwardingError`

This is only the transport contract. A complete Windows implementation still needs:

1. host-side USB device selection and exclusive capture
2. transfer scheduler for control, bulk, interrupt, and isochronous transfers
3. virtual USB bus or UDE-based device surface on the receiver
4. per-device allowlist and explicit user confirmation
5. backpressure, cancellation, teardown, and reconnect semantics

HID keyboard/mouse sharing should continue through the dedicated HID path. Generic USB forwarding is for devices that cannot be represented safely as standard input/audio/gamepad events.
