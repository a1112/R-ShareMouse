#![cfg(all(target_os = "linux", feature = "x11"))]

/// Daemon snapshots may enumerate monitors from concurrent tasks. Exercise the
/// real XRandR allocation/cleanup path under Xvfb, without changing any mode.
#[test]
fn concurrent_display_enumeration_preserves_native_resource_lifetimes() {
    std::thread::scope(|scope| {
        for _ in 0..12 {
            scope.spawn(|| {
                for _ in 0..32 {
                    let state = rshare_platform::display::query_display_state()
                        .expect("Linux display lifecycle test requires XRandR (use xvfb-run)");
                    assert!(state.display_count > 0);
                    assert_eq!(state.display_count, state.displays.len());
                    assert!(state
                        .displays
                        .iter()
                        .all(|display| display.width > 0 && display.height > 0));
                }
            });
        }
    });
}
