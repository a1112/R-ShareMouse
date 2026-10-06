# Mobile discovery, desktop pairing, and landscape keyboard

## User flow

1. The Android app probes its Wi-Fi or Ethernet private IPv4 /24 networks for an R-ShareMouse gateway on TCP 27437. On 192.168 networks, it also scans the common routed 192.168.1/24, 192.168.0/24, and 192.168.10/24 subnets after the local subnet. A gateway answers `GET /api/discover` with only a service marker and computer name.
2. The user selects the computer and sends `POST /api/pair/request` with a short phone name. The gateway records a random request ID for two minutes and displays the phone name and source IP in desktop Settings → Mobile.
3. The computer user chooses Allow or Reject. The phone polls `GET /api/pair/status` using the request ID. Only the requesting IP receives the current session token after approval.
4. The app opens the existing authenticated controller page. It stores the connection locally and tries it automatically on the next launch. When the daemon rotates its token, the user repeats desktop confirmation.
5. The Android toolbar switches to independent keyboard mode and requests landscape orientation. The gateway page exposes a QWERTY keyboard with number, modifier, navigation, and editing keys; each key uses the existing input injection path and held input release handling.

## Security and operational limits

- Discovery never advertises the access token. Pair requests are capped at eight pending entries and expire after two minutes.
- Pairing is approved only through local desktop IPC. The status endpoint checks the source IP as well as the unguessable request ID.
- Mobile gateway traffic is currently plain HTTP on the local network, matching the existing experimental gateway. Use a trusted LAN; TLS and durable device grants remain future work.
- The app scans the phone's private IPv4 /24 subnet, plus three common routed home subnets on 192.168 networks. Networks that isolate Wi-Fi clients or place the computer outside these subnets need manual connection.

## Visual reference

The concept image is `docs/design/mobile-discovery-keyboard-concept.png`, generated with the built-in imagegen tool. Final API 36 emulator captures are `docs/design/mobile-discovery-emulator.png`, `docs/design/mobile-touchpad-emulator.png`, and `docs/design/mobile-keyboard-emulator.png`.

Final imagegen prompt: “Create a polished product UI concept sheet for an Android app named R-ShareMouse. Show TWO distinct phone screen mockups side by side on a plain neutral background: left portrait 20:9 screen for local computer auto discovery and desktop-approved pairing; right landscape 20:9 screen for a standalone full QWERTY keyboard remote control. Premium practical interaction design, dark charcoal background #101816, emerald green #49C989 accent, accessible high contrast, subtle elevated rounded cards, clear touch targets, minimalist technical style. Left screen: header R-ShareMouse, title '发现电脑', a discovered computer card with PC monitor icon, name 'DESKTOP-14', LAN address '192.168.1.253', status '等待电脑端确认', prominent action '请求配对', small instruction '请在电脑端确认此设备', secondary manual connection. Right screen: top bar computer name and connected dot, mode tabs '触控板' and active '独立键盘', keyboard with functional Esc, Tab, Shift, Ctrl, Alt, Space, Enter, Backspace and 4 rows of QWERTY letter keys, keyboard filling most of landscape area, a clear return affordance. Realistic app screen UI, pixel-clean typography, no hands, no device hardware bezel, no decorative 3D, avoid small illegible text. This is a design reference image, not a wireframe.”
