use rshare_core::{
    VirtualDisplayCreateRequest, VirtualDisplayOperationStatus, VirtualDisplayRemoveRequest,
};

fn assert_create_status(status: VirtualDisplayOperationStatus) {
    if cfg!(windows) {
        assert!(
            matches!(
                status,
                VirtualDisplayOperationStatus::Created
                    | VirtualDisplayOperationStatus::DriverUnavailable
            ),
            "unexpected virtual display create status: {status:?}"
        );
    } else {
        assert_eq!(status, VirtualDisplayOperationStatus::Unsupported);
    }
}

fn assert_remove_status(status: VirtualDisplayOperationStatus) {
    if cfg!(windows) {
        assert!(
            matches!(
                status,
                VirtualDisplayOperationStatus::Removed
                    | VirtualDisplayOperationStatus::DriverUnavailable
            ),
            "unexpected virtual display remove status: {status:?}"
        );
    } else {
        assert_eq!(status, VirtualDisplayOperationStatus::Unsupported);
    }
}

#[test]
fn rejects_zero_sized_virtual_display() {
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

#[test]
fn reports_platform_create_result_without_faking_display() {
    let result =
        rshare_platform::virtual_display::create_virtual_display(&VirtualDisplayCreateRequest {
            id: Some("vd-1".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        })
        .expect("platform availability should be reported as an operation result");

    assert_create_status(result.status);
    assert_eq!(
        result.display.as_ref().map(|display| display.id.as_str()),
        Some("vd-1")
    );
    if result.status == VirtualDisplayOperationStatus::Created {
        let _ = rshare_platform::virtual_display::remove_virtual_display(
            &VirtualDisplayRemoveRequest {
                id: "vd-1".to_string(),
            },
        );
    }
}

#[test]
fn reports_platform_remove_result_without_faking_display() {
    let result =
        rshare_platform::virtual_display::remove_virtual_display(&VirtualDisplayRemoveRequest {
            id: "vd-1".to_string(),
        })
        .expect("platform availability should be reported as an operation result");

    assert_remove_status(result.status);
    if result.status == VirtualDisplayOperationStatus::Removed {
        assert!(result.display.is_some());
    } else {
        assert!(result.display.is_none());
    }
}
