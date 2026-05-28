use anyhow::Result;
use rshare_core::{
    VirtualDisplayCreateRequest, VirtualDisplayOperationResult, VirtualDisplayOperationStatus,
    VirtualDisplayRemoveRequest, VirtualDisplaySnapshot, VirtualDisplayStatus,
};

const DEFAULT_REFRESH_RATE_MILLIHZ: u32 = 60_000;

pub fn list_virtual_displays() -> Result<Vec<VirtualDisplaySnapshot>> {
    Ok(Vec::new())
}

pub fn create_virtual_display(
    request: &VirtualDisplayCreateRequest,
) -> Result<VirtualDisplayOperationResult> {
    if !is_valid_mode(request.width, request.height, request.refresh_rate_millihz) {
        return Ok(VirtualDisplayOperationResult {
            status: VirtualDisplayOperationStatus::InvalidMode,
            display: None,
            message: Some("virtual display width, height and refresh rate must be positive".into()),
        });
    }

    let status = unavailable_operation_status();
    Ok(VirtualDisplayOperationResult {
        status,
        display: Some(VirtualDisplaySnapshot {
            id: virtual_display_id(request.id.as_deref()),
            width: request.width,
            height: request.height,
            refresh_rate_millihz: request
                .refresh_rate_millihz
                .or(Some(DEFAULT_REFRESH_RATE_MILLIHZ)),
            name: request.name.clone(),
            status: unavailable_display_status(),
            display_id: None,
            message: Some(unavailable_message().to_string()),
        }),
        message: Some(unavailable_message().to_string()),
    })
}

pub fn remove_virtual_display(
    request: &VirtualDisplayRemoveRequest,
) -> Result<VirtualDisplayOperationResult> {
    let status = unavailable_operation_status();
    Ok(VirtualDisplayOperationResult {
        status,
        display: None,
        message: Some(format!(
            "{}: {}",
            unavailable_message(),
            request.id.trim()
        )),
    })
}

fn is_valid_mode(width: u32, height: u32, refresh_rate_millihz: Option<u32>) -> bool {
    width > 0 && height > 0 && refresh_rate_millihz.unwrap_or(DEFAULT_REFRESH_RATE_MILLIHZ) > 0
}

fn virtual_display_id(id: Option<&str>) -> String {
    id.map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or("rshare-vdisplay-1")
        .to_string()
}

fn unavailable_operation_status() -> VirtualDisplayOperationStatus {
    if cfg!(windows) {
        VirtualDisplayOperationStatus::DriverUnavailable
    } else {
        VirtualDisplayOperationStatus::Unsupported
    }
}

fn unavailable_display_status() -> VirtualDisplayStatus {
    if cfg!(windows) {
        VirtualDisplayStatus::DriverUnavailable
    } else {
        VirtualDisplayStatus::Unsupported
    }
}

fn unavailable_message() -> &'static str {
    if cfg!(windows) {
        "Windows virtual display driver is not installed or no control device is available"
    } else {
        "virtual display creation is not implemented on this platform"
    }
}
