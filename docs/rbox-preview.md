# R-Box Windows preview

Build from the committed release worktree with Rust 1.94.1, locked dependencies,
and the production frontend:

```text
npm ci --prefix apps/rshare-desktop-frontend
npm run build --prefix apps/rshare-desktop-frontend
cargo build --release --locked -p rshare-desktop -p rshare-daemon --features tauri/custom-protocol -j 1
```

Ship `rshare-gui.exe` next to `rshare-daemon.exe`, with the source licenses. The
GUI uses `com.rsharemouse.rbox.preview` and the native user configuration folder
`rshare-rbox-preview`. It keeps the existing QUIC port 27431 for peer discovery
compatibility; an occupied port produces an explicit startup error. IPC, local
WebSocket and mobile ports are allocated separately for each desktop instance.

The desktop keeps its profile exclusively open, tracks a real daemon Child,
checks its status PID for local requests, and owns its descendants through a
Windows process Job. Normal Quit waits for that child; forced desktop exit kills
only its inherited process tree. A stale PID file is removed only while the
profile is exclusively held, the old child is gone, and the IPC port is empty.
No process is terminated using a stale PID. Both GUI and daemon logs use the
preview profile. Automatic firewall modification is disabled for this preview;
pairing across hosts may require a user-configured firewall rule.

The Windows preview uses the portable input backend. Optional virtual display,
virtual HID and audio drivers are not silently installed. Dual-host pairing,
actual cross-device input and optional drivers need separate native acceptance.
macOS and Linux runtime lifecycle are not verified by this Windows adaptation.

Validation uses the real GUI and IPC daemon. `RSHARE_USER_ROOT` accepts an
absolute dedicated test profile. `RBOX_PREVIEW_EXIT_AFTER_MS` (1000–120000 ms)
requests the same shutdown function as normal Quit for a controlled native test.
The core example `preview_ownership_check` rejects occupied IPC and missing
bundled daemon resources, and verifies that no Shutdown is sent without an owned
child. R-Box's `accept-sharemouse.ps1` observes a real window and records normal,
forced, and stale-PID startup/exit evidence; those are distinct from full pairing
or clean-OS acceptance.
