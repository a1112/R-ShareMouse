use rshare_core::{VirtualDisplayCreateRequest, VirtualDisplayOperationStatus};

#[cfg(windows)]
const DRIVER_TEST_ENV: &str = "RSHARE_RUN_VDISPLAY_DRIVER_TESTS";
#[cfg(windows)]
const DRIVER_TEST_DISPLAY_ID: &str = "rshare-vdisplay-driver-test";
#[cfg(windows)]
const DRIVER_OPERATION_MUTEX_NAME: &str = "Global\\RShareMouseVirtualDisplayOperation";

#[cfg(windows)]
fn require_idle_virtual_display_driver(
    existing: &[rshare_core::VirtualDisplaySnapshot],
) -> Result<(), String> {
    if existing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "refusing to replace the existing R-ShareMouse virtual display ({:?}); remove it and stop every R-ShareMouse daemon before running this destructive opt-in test",
            existing[0].status
        ))
    }
}

#[cfg(windows)]
struct VirtualDisplayTestOperationGuard {
    handle: isize,
}

#[cfg(windows)]
impl VirtualDisplayTestOperationGuard {
    fn acquire() -> Result<Self, String> {
        let mut name = DRIVER_OPERATION_MUTEX_NAME
            .encode_utf16()
            .collect::<Vec<_>>();
        name.push(0);
        let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
        if handle == 0 {
            return Err(format!(
                "failed to create virtual display test mutex: {}",
                std::io::Error::last_os_error()
            ));
        }
        let wait_result = unsafe { WaitForSingleObject(handle, INFINITE) };
        if wait_result != WAIT_OBJECT_0 && wait_result != WAIT_ABANDONED {
            let error = std::io::Error::last_os_error();
            unsafe {
                CloseHandle(handle);
            }
            return Err(format!(
                "failed to acquire virtual display test mutex: {error}"
            ));
        }
        Ok(Self { handle })
    }
}

#[cfg(windows)]
impl Drop for VirtualDisplayTestOperationGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            CloseHandle(self.handle);
        }
        self.handle = 0;
    }
}

#[cfg(windows)]
const INFINITE: u32 = u32::MAX;
#[cfg(windows)]
const WAIT_OBJECT_0: u32 = 0;
#[cfg(windows)]
const WAIT_ABANDONED: u32 = 0x0000_0080;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(
        lpMutexAttributes: *mut std::ffi::c_void,
        bInitialOwner: i32,
        lpName: *const u16,
    ) -> isize;
    fn WaitForSingleObject(hHandle: isize, dwMilliseconds: u32) -> u32;
    fn ReleaseMutex(hMutex: isize) -> i32;
    fn CloseHandle(hObject: isize) -> i32;
}

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
#[test]
fn real_driver_preflight_rejects_an_existing_singleton_display() {
    let existing = rshare_core::VirtualDisplaySnapshot {
        id: "already-active".to_string(),
        width: 1920,
        height: 1080,
        refresh_rate_millihz: Some(60_000),
        name: None,
        status: rshare_core::VirtualDisplayStatus::Active,
        display_id: Some("existing-display".to_string()),
        message: None,
    };

    let error = require_idle_virtual_display_driver(&[existing]).unwrap_err();

    assert!(error.contains("refusing to replace"));
    assert!(error.contains("stop every R-ShareMouse daemon"));
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
/// Stop every R-ShareMouse daemon, then run:
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

    let _operation_guard = VirtualDisplayTestOperationGuard::acquire()
        .expect("opt-in driver test must acquire the cross-process operation mutex");
    let existing = rshare_platform::virtual_display::list_virtual_displays()
        .expect("opt-in driver test must query existing virtual display state");
    require_idle_virtual_display_driver(&existing)
        .expect("opt-in driver test refuses to replace an existing singleton display");

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
