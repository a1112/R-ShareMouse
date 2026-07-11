use rshare_core::{VirtualDisplayCreateRequest, VirtualDisplayOperationStatus};

#[cfg(windows)]
const DRIVER_TEST_ENV: &str = "RSHARE_RUN_VDISPLAY_DRIVER_TESTS";
#[cfg(windows)]
const DRIVER_TEST_DISPLAY_ID: &str = "rshare-vdisplay-driver-test";

#[test]
fn rejects_zero_sized_virtual_display_without_accessing_driver() {
    let result =
        rshare_platform::virtual_display::create_virtual_display(&VirtualDisplayCreateRequest {
            id: Some("vd-zero".to_string()),
            width: 0,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: None,
        })
        .expect("invalid mode should be reported as an operation result");

    assert_eq!(result.status, VirtualDisplayOperationStatus::InvalidMode);
    assert!(result.display.is_none());
}

#[cfg(not(windows))]
#[test]
fn reports_virtual_display_as_unsupported_off_windows() {
    let result =
        rshare_platform::virtual_display::create_virtual_display(&VirtualDisplayCreateRequest {
            id: Some("vd-unsupported".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: None,
        })
        .expect("unsupported platform should return an operation result");

    assert_eq!(result.status, VirtualDisplayOperationStatus::Unsupported);
}

#[cfg(windows)]
struct VirtualDisplayCleanup {
    id: String,
    armed: bool,
}

#[cfg(windows)]
impl VirtualDisplayCleanup {
    fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            armed: false,
        }
    }

    fn arm(&mut self) {
        self.armed = true;
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(windows)]
#[test]
fn virtual_display_cleanup_starts_disarmed() {
    let cleanup = std::mem::ManuallyDrop::new(VirtualDisplayCleanup::new("vd-test"));

    assert!(!cleanup.armed);
}

#[cfg(windows)]
#[test]
fn virtual_display_cleanup_can_be_armed_after_create_and_disarmed_after_remove() {
    let mut cleanup = std::mem::ManuallyDrop::new(VirtualDisplayCleanup::new("vd-test"));

    cleanup.arm();
    assert!(cleanup.armed);
    cleanup.disarm();
    assert!(!cleanup.armed);
}

#[cfg(windows)]
impl Drop for VirtualDisplayCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        let request = rshare_core::VirtualDisplayRemoveRequest {
            id: self.id.clone(),
        };
        match rshare_platform::virtual_display::remove_virtual_display(&request) {
            Ok(result) if result.status == VirtualDisplayOperationStatus::Removed => {}
            Ok(result) => eprintln!(
                "virtual display cleanup returned {:?}: {}",
                result.status,
                result.message.as_deref().unwrap_or("no diagnostic")
            ),
            Err(error) => eprintln!("virtual display cleanup failed: {error}"),
        }
    }
}

/// Manual Windows driver validation only.
///
/// PowerShell:
/// `$env:RSHARE_RUN_VDISPLAY_DRIVER_TESTS='1'; cargo test -p rshare-platform --test virtual_display -- --test-threads=1`
#[cfg(windows)]
#[test]
fn windows_virtual_display_driver_round_trip_requires_explicit_opt_in() {
    if std::env::var(DRIVER_TEST_ENV).as_deref() != Ok("1") {
        eprintln!(
            "skipping real virtual display driver test; set {DRIVER_TEST_ENV}=1 and run `cargo test -p rshare-platform --test virtual_display -- --test-threads=1`"
        );
        return;
    }

    let mut cleanup = VirtualDisplayCleanup::new(DRIVER_TEST_DISPLAY_ID);
    let create_result =
        rshare_platform::virtual_display::create_virtual_display(&VirtualDisplayCreateRequest {
            id: Some(DRIVER_TEST_DISPLAY_ID.to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display Driver Test".to_string()),
        })
        .expect("opt-in virtual display driver create call should complete");

    assert_eq!(
        create_result.status,
        VirtualDisplayOperationStatus::Created,
        "opt-in driver test requires an installed, available driver: {}",
        create_result
            .message
            .as_deref()
            .unwrap_or("no driver diagnostic")
    );
    cleanup.arm();
    let display = create_result
        .display
        .as_ref()
        .expect("created virtual display should include a snapshot");
    assert_eq!(display.id, DRIVER_TEST_DISPLAY_ID);

    let remove_result = rshare_platform::virtual_display::remove_virtual_display(
        &rshare_core::VirtualDisplayRemoveRequest {
            id: DRIVER_TEST_DISPLAY_ID.to_string(),
        },
    )
    .expect("opt-in virtual display driver remove call should complete");
    if remove_result.status == VirtualDisplayOperationStatus::Removed {
        cleanup.disarm();
    }
    assert_eq!(
        remove_result.status,
        VirtualDisplayOperationStatus::Removed,
        "opt-in driver cleanup should remove the display: {}",
        remove_result
            .message
            .as_deref()
            .unwrap_or("no driver diagnostic")
    );
}
