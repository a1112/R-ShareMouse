# macOS Local Network recovery

## Scope and permission evidence

Native Qt/Rust LAN operations can trigger Local Network consent on macOS 15 and later. The bundle describes the feature with `NSLocalNetworkUsageDescription`. Network video does not require Camera permission merely because it contains camera images. This change adds no Camera/Full Disk Access entitlement, permission probe, TCC reset, firewall exception, or network security policy change.

A missing peer, HTTP 401/403, SQL authentication error, timeout, TLS failure, or POSIX `PermissionDenied` does not establish a TCC denial. These clients do not receive authoritative `NWPath.UnsatisfiedReason.localNetworkDenied` / Bonjour PolicyDenied evidence, so the permission state remains unknown. A successful connection is operation evidence, not a general permission-status claim. Settings navigation is manual because private URL anchors are not stable APIs.

See Apple's [TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy). WKWebView-originated JavaScript traffic is exempt; the declarations here are for native networking. No browser-only permission prompt was added.

## Required signed-macOS validation (not performed by the Linux fixtures)

- Inspect the final built `.app/Contents/Info.plist` with `plutil`, and verify its signature/identity with `codesign --verify --deep --strict` and `codesign -dr -`. Source plist validation is not bundle validation.
- On a clean macOS test account/VM, use the actual packaged GUI and its real feature: allow once, deny once, revoke during use, then return from System Settings → Privacy & Security → Local Network and explicitly retry. Include macOS 15.1+ when investigating 15.0 behavior.
- Compare an offline endpoint, bad address, timeout, TLS failure and server/authentication failure. They must not be presented as proven Local Network denial.
- Confirm other features remain usable. Repeat retry clicks; close the UI during a pending attempt; check that late completions do not revive stale work. Test an update under the same signing identity.
- Do not use Terminal/SSH success to claim the GUI/helper has permission. Do not automate permission grants or reset TCC as a recovery path.

## Networking helper identity and recovery

The daemon owns UDP discovery and QUIC communication. Both GUI bundle generators carry the purpose. The daemon embeds its own purpose and existing `com.rsharemouse.daemon` identity via `apps/rshare-daemon/build.rs` and `Info.macos.plist`; the Tauri GUI keeps `com.rsharemouse.desktop`. The legacy bundle's existing identifier is preserved. Raw UDP discovery does not justify adding speculative Bonjour service names.

The signing script now verifies the daemon signature and identifier separately from the outer bundle. On a signed Mac release, additionally inspect the daemon's embedded `__TEXT,__info_plist` metadata, its designated requirement, actual executable path and the PID shown in Service Settings. Verify both bundled launch and any separately installed service launch. Which Settings entry is responsible still requires native validation; this change does not claim the GUI's Accessibility/Input Monitoring grant authorizes a service's Local Network access. Existing input/helper permission flows are unchanged.

Discovery I/O errors travel through the network manager to a separate `last_discovery_error` in daemon snapshots. Service Settings shows the most recent error for that daemon session, not a live permission boolean. Empty peer lists remain ordinary discovery results. After a Settings change the existing Stop Service / Start Service controls provide explicit retry; the explanation warns this interrupts sharing. Restart creates a fresh error snapshot.

Portable gates: `python scripts/test_macos_lan_metadata.py`; `node --test apps/rshare-desktop-frontend/src/app/desktop-model.test.mjs`; shell syntax checks for both packaging scripts; Rust parser checks and standalone build-script output checks. The network-free Rust fixture `discovery_io_failure_reaches_consumers_without_claiming_tcc_denial` is included for a full dependency-equipped `cargo test -p rshare-net` run. Linux source checks cannot verify Mach-O linkage, signing, TCC attribution, native consent, or actual dual-computer input.
