# Extended desktop, iPad and touch

User-approved direction: Windows host creates a real extra OS display, computers
and mobile devices receive it, latency takes priority, support wired transport and
Windows touch. Existing uncommitted mobile/UI work must be preserved.

## First integrated delivery

The daemon owns a single extended-display session, its virtual display, signaling,
receiver ownership and touch injection. A local Edge/Chrome sender page captures
the extra screen through getDisplayMedia and streams with WebRTC. This uses the
browser's native capture/codec/congestion stack, avoiding screenshots, JSON video
and a new unverified Rust hardware codec implementation. Capture requires the
user to choose the created screen in the browser picker. The sender is restricted
to loopback; authenticated receivers use the existing mobile gateway.

H.264 is preferred when negotiated, at most 1080p60 and 12 Mbps initially; hardware
acceleration depends on the browser/device. No latency guarantee without physical
measurement. Media takes a direct local WebRTC path without public STUN/TURN.
Input and signaling use a separate bounded WebSocket. Receiver video uses inline
playback, aspect-fit geometry and the lowest supported jitter-buffer target.

Touch is opt-in on the sender after verifying capture selection. Receiver pointer
contacts map to normalized coordinates within the actual video image, excluding
letterboxing. The daemon resolves the OS display geometry and injects real Windows
multi-touch, never fabricated mouse clicks. Sequence checks, bounded queues,
contact limits, heartbeat timeout, display changes and disconnect cancel contacts.
This is Windows touch injection; it does not enumerate a hardware HID digitizer
or claim Windows tablet-mode hardware capability.

## USB boundary

WebRTC works over reachable IP links including USB tethering/USB Ethernet. The
receiver must use the host address on that adapter; the selected ICE candidate
pair, rather than the URL or presence of a cable, is the evidence for routing.
iPad personal-hotspot USB networking depends on cellular hardware/plan and Apple
drivers. A Wi-Fi-only iPad cannot be promised generic cable-only browser access.
Direct iPad USB transport requires a native receiver and host usbmux connection;
this is separate work requiring an Apple development/signing environment and a
real device. No fake USB-ready state or automatic external driver installation.

## Ownership and failures

Do not adopt or remove a pre-existing virtual display. Create a session-owned ID
only when no virtual display already exists. Remove that ID on sender termination
and report cleanup failures. Receiver termination releases touch and closes media;
sender can accept a replacement receiver. Driver absence is an explicit failure.
All capture/media state remains visibly distinct from OS display creation.

## Validation

Automated tests cover coordinate mapping including negative monitor origins,
letterboxing, multi-touch transitions, stale/duplicate messages, release locations,
bounded signaling, role restrictions and cleanup. Build daemon and frontend.
Physical acceptance still requires Windows IDD installed, a second computer/iPad,
browser screen selection, drag-across, pinch/scroll, lost connectivity, and measured
glass-to-glass latency over Wi-Fi and the intended wired adapter.

References: https://developer.mozilla.org/en-US/docs/Web/API/MediaDevices/getDisplayMedia
and https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-injecttouchinput
