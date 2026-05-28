use rshare_core::{
    VirtualDisplayCreateRequest, VirtualDisplayOperationStatus, VirtualDisplayRemoveRequest,
};

#[test]
fn rejects_zero_sized_virtual_display() {
    let result = rshare_platform::virtual_display::create_virtual_display(
        &VirtualDisplayCreateRequest {
            id: Some("vd-zero".to_string()),
            width: 0,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: None,
        },
    )
    .expect("invalid mode should be reported as an operation result");

    assert_eq!(result.status, VirtualDisplayOperationStatus::InvalidMode);
    assert!(result.display.is_none());
}

#[test]
fn reports_driver_unavailable_without_platform_driver() {
    let result = rshare_platform::virtual_display::create_virtual_display(
        &VirtualDisplayCreateRequest {
            id: Some("vd-1".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        },
    )
    .expect("platform availability should be reported as an operation result");

    let expected = if cfg!(windows) {
        VirtualDisplayOperationStatus::DriverUnavailable
    } else {
        VirtualDisplayOperationStatus::Unsupported
    };
    assert_eq!(result.status, expected);
    assert_eq!(result.display.as_ref().map(|display| display.id.as_str()), Some("vd-1"));
}

#[test]
fn remove_reports_missing_driver_without_faking_display() {
    let result = rshare_platform::virtual_display::remove_virtual_display(
        &VirtualDisplayRemoveRequest {
            id: "vd-1".to_string(),
        },
    )
    .expect("platform availability should be reported as an operation result");

    let expected = if cfg!(windows) {
        VirtualDisplayOperationStatus::DriverUnavailable
    } else {
        VirtualDisplayOperationStatus::Unsupported
    };
    assert_eq!(result.status, expected);
    assert!(result.display.is_none());
}
