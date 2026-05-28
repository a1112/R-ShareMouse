use anyhow::Result;
use rshare_core::{
    LocalDisplayInfo, LocalDisplayState, VirtualDisplayCreateRequest,
    VirtualDisplayOperationResult, VirtualDisplayOperationStatus, VirtualDisplayRemoveRequest,
    VirtualDisplaySnapshot, VirtualDisplayStatus,
};

const DEFAULT_REFRESH_RATE_MILLIHZ: u32 = 60_000;
const RSHARE_DRIVER_ABI: u16 = 1;
const FILE_DEVICE_UNKNOWN: u32 = 0x0000_0022;
const METHOD_BUFFERED: u32 = 0;
const FILE_ANY_ACCESS: u32 = 0;
const FILE_READ_DATA: u32 = 0x0001;
const FILE_WRITE_DATA: u32 = 0x0002;
const IOCTL_RSHARE_QUERY_VERSION: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_ANY_ACCESS);
const IOCTL_RSHARE_QUERY_CAPABILITIES: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_ANY_ACCESS);
const IOCTL_RSHARE_VDISPLAY_QUERY_STATE: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x811, METHOD_BUFFERED, FILE_READ_DATA);
const IOCTL_RSHARE_VDISPLAY_CREATE: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x812, METHOD_BUFFERED, FILE_WRITE_DATA);
const IOCTL_RSHARE_VDISPLAY_REMOVE: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x813, METHOD_BUFFERED, FILE_WRITE_DATA);
const RSHARE_CAP_VIRTUAL_DISPLAY: u32 = 0x0000_0010;
const RSHARE_DEFAULT_VDISPLAY_ID: &str = "rshare-vdisplay-1";
const RSHARE_VDISPLAY_ACTIVITY_REMOVED: u16 = 0;
const RSHARE_VDISPLAY_ACTIVITY_ACTIVE: u16 = 1;
const RSHARE_VDISPLAY_ACTIVITY_PENDING: u16 = 2;
const REFRESH_RATE_MATCH_TOLERANCE_MILLIHZ: u32 = 1_000;
const RSHARE_VDISPLAY_SUPPORTED_MODES: &[(u32, u32, u32)] = &[
    (3840, 2160, 60_000),
    (2560, 1440, 144_000),
    (2560, 1440, 90_000),
    (2560, 1440, 60_000),
    (1920, 1080, 144_000),
    (1920, 1080, 90_000),
    (1920, 1080, 60_000),
    (1600, 900, 60_000),
    (1280, 720, 90_000),
    (1280, 720, 60_000),
    (1024, 768, 75_000),
    (1024, 768, 60_000),
];

pub fn list_virtual_displays() -> Result<Vec<VirtualDisplaySnapshot>> {
    #[cfg(windows)]
    {
        windows_list_virtual_displays()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

pub fn create_virtual_display(
    request: &VirtualDisplayCreateRequest,
) -> Result<VirtualDisplayOperationResult> {
    if !is_valid_mode(request.width, request.height, request.refresh_rate_millihz) {
        return Ok(VirtualDisplayOperationResult {
            status: VirtualDisplayOperationStatus::InvalidMode,
            display: None,
            message: Some(invalid_mode_message(
                request.width,
                request.height,
                request.refresh_rate_millihz,
            )),
        });
    }

    #[cfg(windows)]
    {
        windows_create_virtual_display(request)
    }
    #[cfg(not(windows))]
    {
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
}

pub fn remove_virtual_display(
    request: &VirtualDisplayRemoveRequest,
) -> Result<VirtualDisplayOperationResult> {
    #[cfg(windows)]
    {
        windows_remove_virtual_display(request)
    }
    #[cfg(not(windows))]
    {
        let status = unavailable_operation_status();
        Ok(VirtualDisplayOperationResult {
            status,
            display: None,
            message: Some(format!("{}: {}", unavailable_message(), request.id.trim())),
        })
    }
}

fn is_valid_mode(width: u32, height: u32, refresh_rate_millihz: Option<u32>) -> bool {
    is_positive_mode(width, height, refresh_rate_millihz)
        && is_supported_mode(width, height, refresh_rate_millihz)
}

fn is_positive_mode(width: u32, height: u32, refresh_rate_millihz: Option<u32>) -> bool {
    width > 0 && height > 0 && refresh_rate_millihz.unwrap_or(DEFAULT_REFRESH_RATE_MILLIHZ) > 0
}

fn is_supported_mode(width: u32, height: u32, refresh_rate_millihz: Option<u32>) -> bool {
    let refresh_rate_millihz = refresh_rate_millihz.unwrap_or(DEFAULT_REFRESH_RATE_MILLIHZ);
    RSHARE_VDISPLAY_SUPPORTED_MODES
        .iter()
        .any(|mode| *mode == (width, height, refresh_rate_millihz))
}

fn invalid_mode_message(width: u32, height: u32, refresh_rate_millihz: Option<u32>) -> String {
    if !is_positive_mode(width, height, refresh_rate_millihz) {
        return "virtual display width, height and refresh rate must be positive".to_string();
    }

    let refresh_rate_millihz = refresh_rate_millihz.unwrap_or(DEFAULT_REFRESH_RATE_MILLIHZ);
    format!(
        "unsupported virtual display mode {width}x{height}@{refresh_rate_millihz}; use one of the driver-reported modes"
    )
}

fn virtual_display_id(id: Option<&str>) -> String {
    id.map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(RSHARE_DEFAULT_VDISPLAY_ID)
        .to_string()
}

#[cfg(not(windows))]
fn unavailable_operation_status() -> VirtualDisplayOperationStatus {
    if cfg!(windows) {
        VirtualDisplayOperationStatus::DriverUnavailable
    } else {
        VirtualDisplayOperationStatus::Unsupported
    }
}

#[cfg(not(windows))]
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RShareDriverVersionRaw {
    major: u16,
    minor: u16,
    patch: u16,
    abi: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RShareDriverCapabilitiesRaw {
    abi: u16,
    flags: u32,
    max_event_size: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RShareVdisplayRequestRaw {
    width: u32,
    height: u32,
    refresh_rate_millihz: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RShareVdisplayStateRaw {
    abi: u16,
    active: u16,
    width: u32,
    height: u32,
    refresh_rate_millihz: u32,
    connector_index: u32,
}

const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

fn driver_request_from_create(
    request: &VirtualDisplayCreateRequest,
) -> Result<RShareVdisplayRequestRaw> {
    if !is_valid_mode(request.width, request.height, request.refresh_rate_millihz) {
        anyhow::bail!(invalid_mode_message(
            request.width,
            request.height,
            request.refresh_rate_millihz
        ));
    }

    Ok(RShareVdisplayRequestRaw {
        width: request.width,
        height: request.height,
        refresh_rate_millihz: request
            .refresh_rate_millihz
            .unwrap_or(DEFAULT_REFRESH_RATE_MILLIHZ),
        flags: 0,
    })
}

fn snapshot_from_driver_state(
    id: &str,
    name: Option<String>,
    state: RShareVdisplayStateRaw,
    message: Option<String>,
) -> Result<VirtualDisplaySnapshot> {
    snapshot_from_driver_state_with_displays(id, name, state, None, message)
}

fn snapshot_from_driver_state_with_displays(
    id: &str,
    name: Option<String>,
    state: RShareVdisplayStateRaw,
    display_state: Option<&LocalDisplayState>,
    message: Option<String>,
) -> Result<VirtualDisplaySnapshot> {
    if state.abi != RSHARE_DRIVER_ABI {
        anyhow::bail!(
            "Unsupported RShare virtual display driver ABI {}",
            state.abi
        );
    }

    Ok(VirtualDisplaySnapshot {
        id: id.to_string(),
        width: state.width,
        height: state.height,
        refresh_rate_millihz: Some(state.refresh_rate_millihz),
        name,
        status: virtual_display_status_from_driver_activity(state.active),
        display_id: if state.active == RSHARE_VDISPLAY_ACTIVITY_ACTIVE {
            matching_windows_display_id(&state, display_state)
        } else {
            None
        },
        message,
    })
}

fn virtual_display_status_from_driver_activity(activity: u16) -> VirtualDisplayStatus {
    match activity {
        RSHARE_VDISPLAY_ACTIVITY_REMOVED => VirtualDisplayStatus::Removed,
        RSHARE_VDISPLAY_ACTIVITY_ACTIVE => VirtualDisplayStatus::Active,
        RSHARE_VDISPLAY_ACTIVITY_PENDING => VirtualDisplayStatus::Pending,
        _ => VirtualDisplayStatus::Failed,
    }
}

fn matching_windows_display_id(
    state: &RShareVdisplayStateRaw,
    display_state: Option<&LocalDisplayState>,
) -> Option<String> {
    let display_state = display_state?;
    display_state
        .displays
        .iter()
        .find(|display| virtual_display_matches_local_display(state, display))
        .map(|display| display.display_id.clone())
}

fn virtual_display_matches_local_display(
    state: &RShareVdisplayStateRaw,
    display: &LocalDisplayInfo,
) -> bool {
    if !display.active || display.width != state.width || display.height != state.height {
        return false;
    }

    let refresh_matches = display
        .refresh_rate_millihz
        .map(|refresh| refresh_rate_matches(refresh, state.refresh_rate_millihz))
        .unwrap_or(true);
    if !refresh_matches {
        return false;
    }

    let name_hint = [
        display.friendly_name.as_deref(),
        display.device_name.as_deref(),
        display.target_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|name| {
        let name = name.to_ascii_lowercase();
        name.contains("r-sharemouse") || name.contains("rshare") || name.contains("virtual")
    });

    name_hint
}

fn refresh_rate_matches(actual_millihz: u32, expected_millihz: u32) -> bool {
    actual_millihz.abs_diff(expected_millihz) <= REFRESH_RATE_MATCH_TOLERANCE_MILLIHZ
}

fn operation_from_driver_state(
    status: VirtualDisplayOperationStatus,
    id: &str,
    name: Option<String>,
    state: RShareVdisplayStateRaw,
    message: Option<String>,
) -> VirtualDisplayOperationResult {
    match snapshot_from_driver_state(id, name, state, message.clone()) {
        Ok(display) => VirtualDisplayOperationResult {
            status,
            display: Some(display),
            message,
        },
        Err(error) => VirtualDisplayOperationResult {
            status: VirtualDisplayOperationStatus::Failed,
            display: None,
            message: Some(error.to_string()),
        },
    }
}

fn operation_from_driver_state_with_displays(
    status: VirtualDisplayOperationStatus,
    id: &str,
    name: Option<String>,
    state: RShareVdisplayStateRaw,
    display_state: Option<&LocalDisplayState>,
    message: Option<String>,
) -> VirtualDisplayOperationResult {
    match snapshot_from_driver_state_with_displays(id, name, state, display_state, message.clone())
    {
        Ok(display) => VirtualDisplayOperationResult {
            status,
            display: Some(display),
            message,
        },
        Err(error) => VirtualDisplayOperationResult {
            status: VirtualDisplayOperationStatus::Failed,
            display: None,
            message: Some(error.to_string()),
        },
    }
}

#[cfg(windows)]
fn windows_list_virtual_displays() -> Result<Vec<VirtualDisplaySnapshot>> {
    let client = match WindowsVirtualDisplayClient::open() {
        Ok(client) => client,
        Err(_) => return Ok(Vec::new()),
    };

    let state = client.query_state()?;
    if state.active == RSHARE_VDISPLAY_ACTIVITY_REMOVED {
        return Ok(Vec::new());
    }

    let display_state = crate::display::query_display_state().ok();
    Ok(vec![snapshot_from_driver_state_with_displays(
        RSHARE_DEFAULT_VDISPLAY_ID,
        Some("R-ShareMouse Virtual Display".to_string()),
        state,
        display_state.as_ref(),
        None,
    )?])
}

#[cfg(windows)]
fn windows_create_virtual_display(
    request: &VirtualDisplayCreateRequest,
) -> Result<VirtualDisplayOperationResult> {
    let raw_request = driver_request_from_create(request)?;
    let id = virtual_display_id(request.id.as_deref());
    let client = match WindowsVirtualDisplayClient::open() {
        Ok(client) => client,
        Err(error) => return Ok(driver_unavailable_create_result(request, error.to_string())),
    };

    if let Err(error) = client.ensure_virtual_display_capable() {
        return Ok(driver_unavailable_create_result(request, error.to_string()));
    }

    if let Err(error) = client.create_or_update(raw_request) {
        return Ok(VirtualDisplayOperationResult {
            status: VirtualDisplayOperationStatus::Failed,
            display: None,
            message: Some(error.to_string()),
        });
    }

    let state = client.query_state()?;
    let display_state = crate::display::query_display_state().ok();
    Ok(operation_from_driver_state_with_displays(
        VirtualDisplayOperationStatus::Created,
        &id,
        request.name.clone(),
        state,
        display_state.as_ref(),
        None,
    ))
}

#[cfg(windows)]
fn windows_remove_virtual_display(
    request: &VirtualDisplayRemoveRequest,
) -> Result<VirtualDisplayOperationResult> {
    let client = match WindowsVirtualDisplayClient::open() {
        Ok(client) => client,
        Err(error) => {
            return Ok(VirtualDisplayOperationResult {
                status: VirtualDisplayOperationStatus::DriverUnavailable,
                display: None,
                message: Some(format!("{}: {}", unavailable_message(), error)),
            });
        }
    };

    if let Err(error) = client.remove() {
        return Ok(VirtualDisplayOperationResult {
            status: VirtualDisplayOperationStatus::Failed,
            display: None,
            message: Some(error.to_string()),
        });
    }

    let state = client.query_state()?;
    Ok(operation_from_driver_state(
        VirtualDisplayOperationStatus::Removed,
        request.id.trim(),
        None,
        state,
        None,
    ))
}

#[cfg(windows)]
fn driver_unavailable_create_result(
    request: &VirtualDisplayCreateRequest,
    error: String,
) -> VirtualDisplayOperationResult {
    VirtualDisplayOperationResult {
        status: VirtualDisplayOperationStatus::DriverUnavailable,
        display: Some(VirtualDisplaySnapshot {
            id: virtual_display_id(request.id.as_deref()),
            width: request.width,
            height: request.height,
            refresh_rate_millihz: request
                .refresh_rate_millihz
                .or(Some(DEFAULT_REFRESH_RATE_MILLIHZ)),
            name: request.name.clone(),
            status: VirtualDisplayStatus::DriverUnavailable,
            display_id: None,
            message: Some(format!("{}: {}", unavailable_message(), error)),
        }),
        message: Some(format!("{}: {}", unavailable_message(), error)),
    }
}

#[cfg(windows)]
struct WindowsVirtualDisplayClient {
    handle: isize,
    device_path: String,
}

#[cfg(windows)]
impl WindowsVirtualDisplayClient {
    fn open() -> Result<Self> {
        let device_paths = enumerate_virtual_display_device_paths()?;
        let mut last_error = None;

        for device_path in device_paths {
            match Self::open_path(&device_path) {
                Ok(client) => return Ok(client),
                Err(error) => last_error = Some(error),
            }
        }

        match last_error {
            Some(error) => Err(error),
            None => anyhow::bail!("RShare virtual display driver interface was not found"),
        }
    }

    fn open_path(device_path: &str) -> Result<Self> {
        unsafe {
            let path = wide_null(device_path);
            let handle = CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                0,
            );
            if handle == INVALID_HANDLE_VALUE {
                anyhow::bail!(
                    "RShare virtual display driver interface is unavailable: {}",
                    std::io::Error::last_os_error()
                );
            }

            Ok(Self {
                handle,
                device_path: device_path.to_string(),
            })
        }
    }

    fn query_version(&self) -> Result<RShareDriverVersionRaw> {
        let mut raw = RShareDriverVersionRaw::default();
        unsafe {
            device_io_control(
                self.handle,
                IOCTL_RSHARE_QUERY_VERSION,
                std::ptr::null_mut(),
                0,
                (&mut raw as *mut RShareDriverVersionRaw).cast(),
                std::mem::size_of::<RShareDriverVersionRaw>() as u32,
            )?;
        }

        if raw.abi != RSHARE_DRIVER_ABI {
            anyhow::bail!("Unsupported RShare virtual display driver ABI {}", raw.abi);
        }
        Ok(raw)
    }

    fn query_capabilities(&self) -> Result<RShareDriverCapabilitiesRaw> {
        let mut raw = RShareDriverCapabilitiesRaw::default();
        unsafe {
            device_io_control(
                self.handle,
                IOCTL_RSHARE_QUERY_CAPABILITIES,
                std::ptr::null_mut(),
                0,
                (&mut raw as *mut RShareDriverCapabilitiesRaw).cast(),
                std::mem::size_of::<RShareDriverCapabilitiesRaw>() as u32,
            )?;
        }

        if raw.abi != RSHARE_DRIVER_ABI {
            anyhow::bail!("Unsupported RShare virtual display driver ABI {}", raw.abi);
        }
        Ok(raw)
    }

    fn ensure_virtual_display_capable(&self) -> Result<()> {
        let version = self.query_version()?;
        if version.abi != RSHARE_DRIVER_ABI {
            anyhow::bail!(
                "Unsupported RShare virtual display driver ABI {}",
                version.abi
            );
        }

        let capabilities = self.query_capabilities()?;
        if capabilities.flags & RSHARE_CAP_VIRTUAL_DISPLAY == 0 {
            anyhow::bail!(
                "RShare driver at {} does not expose virtual display capability",
                self.device_path
            );
        }
        Ok(())
    }

    fn query_state(&self) -> Result<RShareVdisplayStateRaw> {
        let mut raw = RShareVdisplayStateRaw::default();
        unsafe {
            device_io_control(
                self.handle,
                IOCTL_RSHARE_VDISPLAY_QUERY_STATE,
                std::ptr::null_mut(),
                0,
                (&mut raw as *mut RShareVdisplayStateRaw).cast(),
                std::mem::size_of::<RShareVdisplayStateRaw>() as u32,
            )?;
        }
        Ok(raw)
    }

    fn create_or_update(&self, mut request: RShareVdisplayRequestRaw) -> Result<()> {
        unsafe {
            device_io_control(
                self.handle,
                IOCTL_RSHARE_VDISPLAY_CREATE,
                (&mut request as *mut RShareVdisplayRequestRaw).cast(),
                std::mem::size_of::<RShareVdisplayRequestRaw>() as u32,
                std::ptr::null_mut(),
                0,
            )
        }
    }

    fn remove(&self) -> Result<()> {
        unsafe {
            device_io_control(
                self.handle,
                IOCTL_RSHARE_VDISPLAY_REMOVE,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                0,
            )
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsVirtualDisplayClient {
    fn drop(&mut self) {
        unsafe {
            if self.handle != INVALID_HANDLE_VALUE && self.handle != 0 {
                CloseHandle(self.handle);
            }
        }
        self.handle = INVALID_HANDLE_VALUE;
    }
}

#[cfg(windows)]
fn enumerate_virtual_display_device_paths() -> Result<Vec<String>> {
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW,
        CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CR_SUCCESS,
    };

    const GUID_DEVINTERFACE_RSHARE_VDISPLAY: GUID =
        GUID::from_u128(0x8c1fd719_6fb8_4f82_a4d2_07c6fd490875);

    unsafe {
        let mut buffer_len = 0u32;
        let status = CM_Get_Device_Interface_List_SizeW(
            &mut buffer_len,
            &GUID_DEVINTERFACE_RSHARE_VDISPLAY,
            PCWSTR::null(),
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        );
        if status != CR_SUCCESS {
            anyhow::bail!(
                "CM_Get_Device_Interface_List_SizeW failed for RShare virtual display interface: {:?}",
                status
            );
        }
        if buffer_len <= 1 {
            return Ok(Vec::new());
        }

        let mut buffer = vec![0u16; buffer_len as usize];
        let status = CM_Get_Device_Interface_ListW(
            &GUID_DEVINTERFACE_RSHARE_VDISPLAY,
            PCWSTR::null(),
            &mut buffer,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        );
        if status != CR_SUCCESS {
            anyhow::bail!(
                "CM_Get_Device_Interface_ListW failed for RShare virtual display interface: {:?}",
                status
            );
        }

        Ok(parse_multi_sz(&buffer))
    }
}

#[cfg(windows)]
fn parse_multi_sz(buffer: &[u16]) -> Vec<String> {
    let mut entries = Vec::new();
    let mut start = 0usize;

    while start < buffer.len() && buffer[start] != 0 {
        let end = buffer[start..]
            .iter()
            .position(|value| *value == 0)
            .map(|offset| start + offset)
            .unwrap_or(buffer.len());
        if end > start {
            entries.push(String::from_utf16_lossy(&buffer[start..end]));
        }
        start = end.saturating_add(1);
    }

    entries
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
unsafe fn device_io_control(
    handle: isize,
    ioctl: u32,
    in_buffer: *mut std::ffi::c_void,
    in_size: u32,
    out_buffer: *mut std::ffi::c_void,
    out_size: u32,
) -> Result<()> {
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            ioctl,
            in_buffer,
            in_size,
            out_buffer,
            out_size,
            &mut returned as *mut u32,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        anyhow::bail!(
            "DeviceIoControl(0x{ioctl:08x}) failed for RShare virtual display driver: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(windows)]
const GENERIC_READ: u32 = 0x8000_0000;
#[cfg(windows)]
const GENERIC_WRITE: u32 = 0x4000_0000;
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;
#[cfg(windows)]
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
#[cfg(windows)]
const OPEN_EXISTING: u32 = 3;
#[cfg(windows)]
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
#[cfg(windows)]
const INVALID_HANDLE_VALUE: isize = -1isize;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *mut std::ffi::c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: isize,
    ) -> isize;
    fn DeviceIoControl(
        hDevice: isize,
        dwIoControlCode: u32,
        lpInBuffer: *mut std::ffi::c_void,
        nInBufferSize: u32,
        lpOutBuffer: *mut std::ffi::c_void,
        nOutBufferSize: u32,
        lpBytesReturned: *mut u32,
        lpOverlapped: *mut std::ffi::c_void,
    ) -> i32;
    fn CloseHandle(hObject: isize) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_driver_state_without_topology_match_has_no_display_id() {
        let snapshot = snapshot_from_driver_state(
            "vd-1",
            Some("R-ShareMouse Virtual Display".to_string()),
            RShareVdisplayStateRaw {
                abi: RSHARE_DRIVER_ABI,
                active: 1,
                width: 2560,
                height: 1440,
                refresh_rate_millihz: 144_000,
                connector_index: 2,
            },
            None,
        )
        .expect("valid active driver state should map to a snapshot");

        assert_eq!(snapshot.id, "vd-1");
        assert_eq!(snapshot.width, 2560);
        assert_eq!(snapshot.height, 1440);
        assert_eq!(snapshot.refresh_rate_millihz, Some(144_000));
        assert_eq!(
            snapshot.name.as_deref(),
            Some("R-ShareMouse Virtual Display")
        );
        assert_eq!(snapshot.status, VirtualDisplayStatus::Active);
        assert!(snapshot.display_id.is_none());
    }

    #[test]
    fn driver_state_maps_requested_but_inactive_display_to_pending_snapshot() {
        let snapshot = snapshot_from_driver_state(
            "vd-pending",
            Some("R-ShareMouse Virtual Display".to_string()),
            RShareVdisplayStateRaw {
                abi: RSHARE_DRIVER_ABI,
                active: 2,
                width: 2560,
                height: 1440,
                refresh_rate_millihz: 144_000,
                connector_index: 0,
            },
            Some("monitor requested; waiting for IddCx arrival".to_string()),
        )
        .expect("requested driver state should map to a pending snapshot");

        assert_eq!(snapshot.id, "vd-pending");
        assert_eq!(snapshot.status, VirtualDisplayStatus::Pending);
        assert!(snapshot.display_id.is_none());
    }

    #[test]
    fn create_request_maps_to_driver_request_with_default_refresh() {
        let raw = driver_request_from_create(&VirtualDisplayCreateRequest {
            id: Some("vd-2".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: None,
            name: None,
        })
        .expect("valid request should map to driver ABI");

        assert_eq!(raw.width, 1920);
        assert_eq!(raw.height, 1080);
        assert_eq!(raw.refresh_rate_millihz, DEFAULT_REFRESH_RATE_MILLIHZ);
        assert_eq!(raw.flags, 0);
    }

    #[test]
    fn create_request_rejects_mode_not_reported_by_virtual_display_driver() {
        let error = driver_request_from_create(&VirtualDisplayCreateRequest {
            id: Some("vd-unsupported-mode".to_string()),
            width: 1234,
            height: 567,
            refresh_rate_millihz: Some(60_000),
            name: None,
        })
        .expect_err("unsupported driver mode should be rejected before driver IO");

        assert!(error
            .to_string()
            .contains("unsupported virtual display mode"));
    }

    #[test]
    fn unsupported_driver_abi_is_reported_as_failed_operation() {
        let result = operation_from_driver_state(
            VirtualDisplayOperationStatus::Created,
            "vd-3",
            None,
            RShareVdisplayStateRaw {
                abi: RSHARE_DRIVER_ABI + 1,
                active: 1,
                width: 1920,
                height: 1080,
                refresh_rate_millihz: 60_000,
                connector_index: 0,
            },
            Some("driver returned an incompatible ABI".to_string()),
        );

        assert_eq!(result.status, VirtualDisplayOperationStatus::Failed);
        assert!(result.display.is_none());
        assert!(result
            .message
            .unwrap()
            .contains("Unsupported RShare virtual display driver ABI"));
    }

    #[test]
    fn active_driver_state_prefers_matching_windows_display_id() {
        let display_state = LocalDisplayState {
            displays: vec![LocalDisplayInfo {
                display_id: "windows-display-real".to_string(),
                width: 1920,
                height: 1080,
                refresh_rate_millihz: Some(60_000),
                friendly_name: Some("R-ShareMouse Virtual Display".to_string()),
                active: true,
                ..LocalDisplayInfo::default()
            }],
            ..LocalDisplayState::default()
        };

        let snapshot = snapshot_from_driver_state_with_displays(
            "vd-1",
            Some("R-ShareMouse Virtual Display".to_string()),
            RShareVdisplayStateRaw {
                abi: RSHARE_DRIVER_ABI,
                active: 1,
                width: 1920,
                height: 1080,
                refresh_rate_millihz: 60_000,
                connector_index: 0,
            },
            Some(&display_state),
            None,
        )
        .expect("driver state should map to matched display snapshot");

        assert_eq!(snapshot.display_id.as_deref(), Some("windows-display-real"));
    }

    #[test]
    fn active_driver_state_matches_windows_display_with_refresh_rounding_delta() {
        let display_state = LocalDisplayState {
            displays: vec![LocalDisplayInfo {
                display_id: "windows-display-rounded-refresh".to_string(),
                width: 1920,
                height: 1080,
                refresh_rate_millihz: Some(59_940),
                friendly_name: Some("R-SHAREMOUSE".to_string()),
                active: true,
                ..LocalDisplayInfo::default()
            }],
            ..LocalDisplayState::default()
        };

        let snapshot = snapshot_from_driver_state_with_displays(
            "vd-1",
            Some("R-ShareMouse Virtual Display".to_string()),
            RShareVdisplayStateRaw {
                abi: RSHARE_DRIVER_ABI,
                active: RSHARE_VDISPLAY_ACTIVITY_ACTIVE,
                width: 1920,
                height: 1080,
                refresh_rate_millihz: 60_000,
                connector_index: 0,
            },
            Some(&display_state),
            None,
        )
        .expect("driver state should map to rounded display snapshot");

        assert_eq!(
            snapshot.display_id.as_deref(),
            Some("windows-display-rounded-refresh")
        );
    }
}
