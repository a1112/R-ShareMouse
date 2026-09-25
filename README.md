# R-ShareMouse

R-ShareMouse is a cross-platform mouse and keyboard sharing software written in Rust. It allows you to use one mouse and keyboard across multiple computers.

## Features

- **Cross-platform**: Supports Windows, macOS, and Linux
- **Low latency**: Optimized for real-time input sharing
- **Secure**: Encrypted communication using QUIC/TLS
- **Clipboard sync**: Share clipboard content across devices
- **File drag and drop**: Drag files across screens into Finder/Explorer folders, or onto a device card; encrypted copying with progress, cancellation and SHA-256 verification
- **Auto-discovery**: Automatically find devices on your local network
- **Desktop UI & CLI**: Borderless Tauri desktop shell plus command-line interface

## Project Status

This is a new project currently under active development.

## Building

### Prerequisites

- Rust and Cargo compatible with `Cargo.lock` (validated with Rust 1.94.1;
  the locked desktop dependencies require at least Rust 1.88).
- Node.js as pinned in `.node-version`, and npm.
- Native desktop build dependencies: Xcode Command Line Tools on macOS;
  MSVC and WebView2 on Windows; WebKitGTK 4.1, appindicator, X11 input,
  libudev and ALSA development libraries on Linux. See the
  [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) and
  [Linux CI setup](.github/actions/prepare-linux-build/action.yml).

### Build from source

```bash
# Clone the repository
git clone https://github.com/a1112/R-ShareMouse.git
cd R-ShareMouse

# Build the frontend embedded by the Tauri desktop executable
npm ci --prefix apps/rshare-desktop-frontend
npm run build --prefix apps/rshare-desktop-frontend

# Build the workspace from the lockfile
cargo build --workspace --release --locked

# The binaries will be in:
# - target/release/rshare-gui
# - target/release/rshare
# - target/release/rshare-daemon
```

## Usage

### CLI

```bash
# Start the service
rshare start

# Show connected devices
rshare devices

# Show version
rshare version
```

### GUI

```bash
# Launch the desktop UI
rshare-gui
```

The GUI binary belongs to the `rshare-desktop` crate. From the repository root,
use `cargo run -p rshare-desktop --bin rshare-gui`. The daemon owns runtime state;
the desktop app starts it when local IPC is unavailable.

For browser development, run `npm run dev --prefix apps/rshare-desktop-frontend`
and open `http://127.0.0.1:5176`. Its daemon bridge accepts same-origin loopback
requests only; mobile/LAN access uses the separate mobile gateway.

### Cross-device file drag and drop

Update both daemons and desktop apps and verify cross-screen mouse control.
On macOS/Windows, grab a local file in Finder/Explorer, hold the left mouse
button across the screen edge, and release over a folder icon or the blank
file area in the other computer's file-manager window. Files are copied into
that folder; existing names receive a numbered suffix. Sources are preserved.
Finder folder lookup may require Automation permission to access Finder.

You can also open **设备 → 文件传输** and drop files onto a device card to save
them under `Downloads/R-ShareMouse/<transfer-id>/`. Both paths share progress,
cancellation and the **打开文件夹** action. Browser mode can display progress
and cancel transfers. Direct folder dragging currently targets Finder/Explorer
filesystem folders; Linux, virtual locations and other application drop targets
are unsupported. The remote pointer does not display a native file-drag thumbnail.
See the [file-drop guide](docs/guides/cross-device-file-drop.md) for limits,
failure handling and validation details.

### Wake-on-LAN

On the **设备** page, save the Ethernet MAC address of a discovered R-ShareMouse
peer, or add a standalone LAN device with a name, MAC address, and IPv4 address.
Saved targets remain available after they disappear from discovery. The local
daemon must be running.

```bash
rshare wake add --name "Office PC" --mac 02:11:22:33:44:55 --ip 192.168.1.50
rshare wake add --name "Peer PC" --mac 02:11:22:33:44:66 --peer-id <device-uuid>
rshare wake list
rshare wake send <wake-target-uuid>
rshare wake edit <wake-target-uuid> --ip 192.168.1.51
rshare wake remove <wake-target-uuid>
```

The daemon broadcasts the magic packet within the local subnet, then waits up
to 90 seconds for the peer to reappear or for a standalone device to answer
ping. A successful send confirms only that the packet left this computer.
`wake send` exits with 0 for a confirmed response, 2 when power-on could not
be confirmed, and 1 for a send error. The target's firmware, operating system,
and network adapter must support and enable Wake-on-LAN, including wake from
the intended power state; standalone IPv4 addresses should be stable and allow
ICMP echo if confirmation is desired.

## Validation

```bash
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked
npm test --prefix apps/rshare-desktop-frontend
npm run test:perf --prefix apps/rshare-desktop-frontend -- --grep-invert @fixed-runner
```

The browser scenarios require Playwright Chromium (`npx playwright install chromium`
from the frontend directory) and a free port 5176. Automated loopback tests do not
replace dual-machine keyboard/mouse, permissions, sleep/wake or driver acceptance;
see [the roadmap](docs/roadmap.md).

## Architecture

```
┌─────────────────────────────────────────┐
│            Applications                 │
│  (GUI / CLI / Daemon)                   │
├─────────────────────────────────────────┤
│            Core Layer                   │
│  (Protocol / Config / Device / Clipboard)│
├─────────────────────────────────────────┤
│          Input Layer                    │
│  (Listener / Emulator / Edge Detection)  │
├─────────────────────────────────────────┤
│         Platform Layer                  │
│  (Windows / macOS / Linux)               │
├─────────────────────────────────────────┤
│          Network Layer                  │
│  (Discovery / QUIC Transport)            │
└─────────────────────────────────────────┘
```

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under either MIT or Apache-2.0 at your option.

## Acknowledgments

Inspired by [ShareMouse](https://www.sharemouse.com/) and the open-source [Barrier](https://github.com/debauchee/barrier) project.
