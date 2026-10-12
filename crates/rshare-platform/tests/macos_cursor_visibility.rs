#![cfg(target_os = "macos")]

use core_graphics::display::CGDisplay;
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use rshare_platform::{restore_macos_local_pointer, set_macos_local_input_suppressed};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGCursorIsVisible() -> u32;
}

static CURSOR_TEST: Mutex<()> = Mutex::new(());

fn cursor_position() -> CGPoint {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).unwrap();
    CGEvent::new(source).unwrap().location()
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

struct RestoreCursor(Option<CGPoint>);

impl Drop for RestoreCursor {
    fn drop(&mut self) {
        set_macos_local_input_suppressed(false);
        if let Some(point) = self.0 {
            let _ = CGDisplay::warp_mouse_cursor_position(point);
        }
    }
}

// Run explicitly from a terminal/background process in an unlocked GUI session:
// cargo test -p rshare-platform --test macos_cursor_visibility -- --ignored
// This uses the production native backend, unlike platform unit-test mocks.
#[test]
#[ignore = "requires an unlocked macOS GUI session and briefly hides its cursor"]
fn background_remote_cursor_hides_and_balances_repeated_entry_with_one_restore() {
    let _serial = CURSOR_TEST
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(
        wait_for_visibility(true),
        "start with a visible local cursor"
    );
    let _restore = RestoreCursor(None);
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

#[test]
#[ignore = "requires an unlocked macOS GUI session and briefly moves its cursor"]
fn background_return_restores_physical_pointer_while_still_hidden() {
    let _serial = CURSOR_TEST
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(wait_for_visibility(true));
    let original = cursor_position();
    let _restore = RestoreCursor(Some(original));
    let bounds = CGDisplay::main().bounds();
    let x = (bounds.origin.x + bounds.size.width - 5.0) as i32;
    let y = (bounds.origin.y + bounds.size.height / 2.0) as i32;
    set_macos_local_input_suppressed(true);
    assert!(wait_for_visibility(false));
    restore_macos_local_pointer(x, y).expect("restore cursor from a background process");
    let point = cursor_position();
    assert_eq!((point.x, point.y), (f64::from(x), f64::from(y)));
    assert!(wait_for_visibility(false), "restore precedes unsuppression");
    set_macos_local_input_suppressed(false);
    assert!(wait_for_visibility(true));
    let point = cursor_position();
    assert_eq!((point.x, point.y), (f64::from(x), f64::from(y)));
}
