#![cfg(target_os = "linux")]

/// Exercise actual libxdo construction/drop concurrently, without injecting
/// input. Hosted Linux workspace tests run this under an isolated Xvfb display.
#[test]
fn concurrent_native_input_initialization_does_not_corrupt_xlib_state() {
    std::thread::scope(|scope| {
        for _ in 0..12 {
            scope.spawn(|| {
                for _ in 0..32 {
                    let emulator = rshare_input::EnigoInputEmulator::new()
                        .expect("Linux native input test requires an X11 display (use xvfb-run)");
                    drop(emulator);
                }
            });
        }
    });
}
