# RShare Audio HAL prototype

This package contains a real AudioServerPlugIn vtable and launchd/XPC shared-memory broker. It is a **development component, not a completed network audio product**. See [implementation status](../../../docs/plans/2026-09-15-network-audio-implementation.md).

Build with `sh scripts/driver/build-macos-audio.sh`, test without installing using `sh scripts/driver/test-macos-audio.sh` from the repository root. Builds contain arm64 and x86_64 slices and use ad-hoc development signatures. The test exercises the actual vtable but substitutes private file mapping for production XPC mapping.

Explicit installation uses `sudo sh scripts/driver/install-macos-audio.sh`; it stages the HAL bundle, root-owned memory broker and launchd configuration and rolls back a failed installation. It refuses to overwrite an existing installation. Uninstall with `sudo sh scripts/driver/uninstall-macos-audio.sh`; files are moved to a recoverable backup. Neither command automatically restarts coreaudiod or the computer.

The production driver obtains memory through `org.rshare.audio.broker`, declared in `AudioServerPlugIn_MachServices`. The broker derives callers' uid from XPC, limits entries, and allows the audio host to attach only to owner/token-qualified entries. Network and audio conversion work belongs in the daemon; none runs in the broker.

The plug-in advertises only the sample rate chosen at device creation. Runtime sample-rate changes, daemon session integration, broker restart recovery and long-lived mapping reclamation remain unfinished. Do not publish this as production-ready or claim the native vtable test proves sandbox-hosted audio playback.
