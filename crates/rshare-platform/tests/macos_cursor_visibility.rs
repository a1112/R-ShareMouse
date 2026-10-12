#![cfg(target_os = "macos")]

use rshare_platform::set_macos_local_input_suppressed;
use std::time::{Duration, Instant};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGCursorIsVisible() -> u32;
}

fn wait_for_visibility(visible: bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if (unsafe { CGCursorIsVisible() } != 0) == visible {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct RestoreCursor;

impl Drop for RestoreCursor {
    fn drop(&mut self) {
        set_macos_local_input_suppressed(false);
    }
}

// Run explicitly from a terminal/background process in an unlocked GUI session:
// cargo test -p rshare-platform --test macos_cursor_visibility -- --ignored
// This uses the production native backend, unlike platform unit-test mocks.
#[test]
#[ignore = "requires an unlocked macOS GUI session and briefly hides its cursor"]
fn background_remote_cursor_hides_and_balances_repeated_entry_with_one_restore() {
    assert!(
        wait_for_visibility(true),
        "start with a visible local cursor"
    );
    let _restore = RestoreCursor;
    set_macos_local_input_suppressed(true);
    assert!(
        wait_for_visibility(false),
        "background remote ownership must actually hide the system cursor"
    );
    set_macos_local_input_suppressed(true);
    set_macos_local_input_suppressed(false);
    assert!(
        wait_for_visibility(true),
        "one restore must balance repeated remote updates"
    );
}
