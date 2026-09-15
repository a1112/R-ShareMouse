# PipeWire audio bridge

The C++ helper implements one native PipeWire stream backed by the common SPSC ring. It can expose an Audio/Source or Audio/Sink, or attach to an explicitly selected physical node. JACK clients use PipeWire's JACK compatibility layer.

Requires Linux, PipeWire 1.x headers/server, pkg-config and a C++17 compiler. From the repository root:

```sh
sh scripts/driver/build-linux-audio.sh
sh scripts/driver/install-linux-audio.sh
```

The daemon integration has not been connected and this helper has not been built or run on Linux in the current development session. The CI job is a future build check, not evidence of success. The helper consumes a private owner-only ring created by the daemon; it never opens a fallback/default hardware endpoint.

Uninstall uses `sh scripts/driver/uninstall-linux-audio.sh`, preserving the binary as `.uninstalled`. Existing installations/backups are not silently replaced.
