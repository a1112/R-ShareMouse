# R-ShareMouse

R-ShareMouse is a cross-platform mouse and keyboard sharing software written in Rust. It allows you to use one mouse and keyboard across multiple computers.

## Features

- **Cross-platform**: Supports Windows, macOS, and Linux
- **Low latency**: Optimized for real-time input sharing
- **Secure**: Encrypted communication using QUIC/TLS
- **Clipboard sync**: Share clipboard content across devices
- **Auto-discovery**: Automatically find devices on your local network
- **Desktop UI & CLI**: Borderless Tauri desktop shell plus command-line interface

## Project Status

This is a new project currently under active development.

## Building

### Prerequisites

- Rust 1.75 or later
- Cargo

### Build from source

```bash
# Clone the repository
git clone https://github.com/a1112/R-ShareMouse.git
cd R-ShareMouse

# Build the workspace
cargo build --release

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
