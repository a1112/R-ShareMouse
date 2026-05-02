//! Experimental generic USB forwarding host runtime.
//!
//! This module implements the host side only: enumerate local USB device
//! interfaces, claim WinUSB-compatible devices, and execute control/bulk/
//! interrupt transfers. Receiver-side virtual USB bus materialization is a
//! separate driver milestone.

use anyhow::{Context, Result};
use rshare_core::{
    UsbDeviceClaimRequest, UsbDeviceClaimResponse, UsbDeviceDescriptor, UsbDeviceResetKind,
    UsbDeviceSpeed, UsbEndpointDescriptor, UsbFlowControl, UsbForwardingCapabilities,
    UsbTransferDirection, UsbTransferKind, UsbTransferPayload, UsbTransferStatus,
};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbTransferCompletion {
    pub transfer_id: u64,
    pub bus_id: String,
    pub status: i32,
    pub transfer_status: UsbTransferStatus,
    pub endpoint_address: Option<u8>,
    pub transfer_kind: Option<UsbTransferKind>,
    pub actual_length: Option<u32>,
    pub data: Vec<u8>,
}

pub struct ExperimentalUsbHostRuntime {
    inner: platform::PlatformUsbHostRuntime,
}

impl ExperimentalUsbHostRuntime {
    pub fn new() -> Self {
        Self {
            inner: platform::PlatformUsbHostRuntime::new(),
        }
    }

    pub fn capabilities(&self) -> UsbForwardingCapabilities {
        platform::capabilities()
    }

    pub fn enumerate_devices(&self) -> Result<Vec<UsbDeviceDescriptor>> {
        platform::enumerate_devices()
    }

    pub fn claim_device(&mut self, request: UsbDeviceClaimRequest) -> UsbDeviceClaimResponse {
        self.inner.claim_device(request)
    }

    pub fn submit_transfer(
        &mut self,
        transfer: &UsbTransferPayload,
    ) -> Result<UsbTransferCompletion> {
        self.inner.submit_transfer(transfer)
    }

    pub fn release_device(&mut self, session_id: Uuid) -> Result<()> {
        self.inner.release_device(session_id)
    }

    pub fn reset_device(
        &mut self,
        session_id: Option<Uuid>,
        bus_id: &str,
        reset_kind: UsbDeviceResetKind,
    ) -> Result<()> {
        self.inner.reset_device(session_id, bus_id, reset_kind)
    }

    pub fn cancel_transfer(&mut self, transfer_id: u64, bus_id: &str) -> Result<()> {
        self.inner.cancel_transfer(transfer_id, bus_id)
    }

    pub fn flow_control(&self, bus_id: String, session_id: Option<Uuid>) -> UsbFlowControl {
        UsbFlowControl {
            bus_id,
            session_id,
            available_window_bytes: self.capabilities().max_transfer_size.saturating_mul(4),
            max_in_flight_transfers: self.capabilities().max_in_flight_transfers,
        }
    }
}

impl Default for ExperimentalUsbHostRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::c_void;
    use std::mem::size_of;

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const OPEN_EXISTING: u32 = 3;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    const INVALID_HANDLE_VALUE: isize = -1isize;

    const DIGCF_PRESENT: u32 = 0x0000_0002;
    const DIGCF_DEVICEINTERFACE: u32 = 0x0000_0010;
    const ERROR_NO_MORE_ITEMS: i32 = 259;
    const ERROR_INSUFFICIENT_BUFFER: i32 = 122;

    const PIPE_TYPE_ISOCHRONOUS: u32 = 1;
    const PIPE_TYPE_BULK: u32 = 2;
    const PIPE_TYPE_INTERRUPT: u32 = 3;

    const GUID_DEVINTERFACE_USB_DEVICE: Guid = Guid {
        data1: 0xa5dcbf10,
        data2: 0x6530,
        data3: 0x11d2,
        data4: [0x90, 0x1f, 0x00, 0xc0, 0x4f, 0xb9, 0x51, 0xed],
    };

    pub struct PlatformUsbHostRuntime {
        sessions: HashMap<Uuid, WindowsUsbSession>,
    }

    impl PlatformUsbHostRuntime {
        pub fn new() -> Self {
            Self {
                sessions: HashMap::new(),
            }
        }

        pub fn claim_device(&mut self, request: UsbDeviceClaimRequest) -> UsbDeviceClaimResponse {
            match WindowsUsbSession::open(&request.bus_id, request.exclusive) {
                Ok(mut session) => {
                    let session_id = Uuid::new_v4();
                    let granted_interfaces = session
                        .refresh_endpoints()
                        .map(|_| vec![0])
                        .unwrap_or_default();
                    self.sessions.insert(session_id, session);
                    UsbDeviceClaimResponse {
                        request_id: request.request_id,
                        bus_id: request.bus_id,
                        accepted: true,
                        session_id: Some(session_id),
                        granted_interfaces,
                        message: None,
                    }
                }
                Err(error) => UsbDeviceClaimResponse {
                    request_id: request.request_id,
                    bus_id: request.bus_id,
                    accepted: false,
                    session_id: None,
                    granted_interfaces: Vec::new(),
                    message: Some(error.to_string()),
                },
            }
        }

        pub fn submit_transfer(
            &mut self,
            transfer: &UsbTransferPayload,
        ) -> Result<UsbTransferCompletion> {
            let session = self.session_for_transfer(transfer)?;
            session.submit_transfer(transfer)
        }

        pub fn release_device(&mut self, session_id: Uuid) -> Result<()> {
            self.sessions
                .remove(&session_id)
                .map(|_| ())
                .ok_or_else(|| anyhow::anyhow!("USB session {session_id} is not claimed"))
        }

        pub fn reset_device(
            &mut self,
            session_id: Option<Uuid>,
            bus_id: &str,
            reset_kind: UsbDeviceResetKind,
        ) -> Result<()> {
            match reset_kind {
                UsbDeviceResetKind::Endpoint => {
                    let session = self.session_for_bus(session_id, bus_id)?;
                    let endpoints: Vec<u8> = session
                        .endpoints
                        .iter()
                        .map(|endpoint| endpoint.address)
                        .collect();
                    for endpoint in endpoints {
                        session.reset_pipe(endpoint)?;
                    }
                    Ok(())
                }
                UsbDeviceResetKind::Interface | UsbDeviceResetKind::Device => {
                    anyhow::bail!(
                        "WinUSB runtime does not support {:?} reset without a kernel virtual bus",
                        reset_kind
                    )
                }
            }
        }

        pub fn cancel_transfer(&mut self, _transfer_id: u64, bus_id: &str) -> Result<()> {
            let session = self.session_for_bus(None, bus_id)?;
            let endpoints: Vec<u8> = session
                .endpoints
                .iter()
                .map(|endpoint| endpoint.address)
                .collect();
            for endpoint in endpoints {
                session.abort_pipe(endpoint)?;
            }
            Ok(())
        }

        fn session_for_transfer(
            &mut self,
            transfer: &UsbTransferPayload,
        ) -> Result<&mut WindowsUsbSession> {
            self.session_for_bus(transfer.session_id, &transfer.bus_id)
        }

        fn session_for_bus(
            &mut self,
            session_id: Option<Uuid>,
            bus_id: &str,
        ) -> Result<&mut WindowsUsbSession> {
            if let Some(session_id) = session_id {
                return self
                    .sessions
                    .get_mut(&session_id)
                    .ok_or_else(|| anyhow::anyhow!("USB session {session_id} is not claimed"));
            }
            self.sessions
                .values_mut()
                .find(|session| session.bus_id.eq_ignore_ascii_case(bus_id))
                .ok_or_else(|| anyhow::anyhow!("USB device {bus_id} is not claimed"))
        }
    }

    pub fn capabilities() -> UsbForwardingCapabilities {
        UsbForwardingCapabilities {
            max_transfer_size: 1024 * 1024,
            max_in_flight_transfers: 32,
            supports_hotplug: true,
            supports_cancel: true,
            supports_reset: true,
            supports_isochronous: false,
            supported_transfer_kinds: vec![
                UsbTransferKind::Control,
                UsbTransferKind::Bulk,
                UsbTransferKind::Interrupt,
            ],
        }
    }

    pub fn enumerate_devices() -> Result<Vec<UsbDeviceDescriptor>> {
        unsafe {
            let set = SetupDiGetClassDevsW(
                &GUID_DEVINTERFACE_USB_DEVICE,
                std::ptr::null(),
                0,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            );
            if set == INVALID_HANDLE_VALUE {
                anyhow::bail!(
                    "SetupDiGetClassDevsW(GUID_DEVINTERFACE_USB_DEVICE) failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            let _guard = DeviceInfoSetGuard(set);
            let mut devices = Vec::new();
            let mut index = 0u32;
            loop {
                let mut interface_data = SpDeviceInterfaceData {
                    cb_size: size_of::<SpDeviceInterfaceData>() as u32,
                    ..SpDeviceInterfaceData::default()
                };
                let ok = SetupDiEnumDeviceInterfaces(
                    set,
                    std::ptr::null_mut(),
                    &GUID_DEVINTERFACE_USB_DEVICE,
                    index,
                    &mut interface_data,
                );
                if ok == 0 {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() == Some(ERROR_NO_MORE_ITEMS) {
                        break;
                    }
                    return Err(error).context("SetupDiEnumDeviceInterfaces failed");
                }

                match device_path_for_interface(set, &mut interface_data) {
                    Ok(device_path) => devices.push(descriptor_from_device_path(device_path)),
                    Err(error) => tracing::debug!("Failed to read USB interface detail: {error}"),
                }
                index = index.saturating_add(1);
            }
            devices.sort_by(|left, right| left.bus_id.cmp(&right.bus_id));
            Ok(devices)
        }
    }

    fn descriptor_from_device_path(device_path: String) -> UsbDeviceDescriptor {
        let (vendor_id, product_id) = parse_vid_pid(&device_path).unwrap_or((0, 0));
        UsbDeviceDescriptor {
            bus_id: device_path.clone(),
            vendor_id,
            product_id,
            class_code: 0,
            subclass_code: 0,
            protocol_code: 0,
            manufacturer: None,
            product: Some(format_usb_product_label(vendor_id, product_id)),
            serial_number: parse_serial_from_device_path(&device_path),
            usb_version_bcd: 0,
            device_version_bcd: 0,
            speed: UsbDeviceSpeed::Unknown,
            active_configuration: None,
            container_id: None,
            capture_exclusive_required: true,
            configurations: Vec::new(),
            endpoints: Vec::new(),
        }
    }

    struct WindowsUsbSession {
        bus_id: String,
        device_handle: isize,
        interface_handle: usize,
        endpoints: Vec<UsbEndpointDescriptor>,
    }

    impl WindowsUsbSession {
        fn open(bus_id: &str, exclusive: bool) -> Result<Self> {
            unsafe {
                let path = wide_null(bus_id);
                let share_mode = if exclusive {
                    0
                } else {
                    FILE_SHARE_READ | FILE_SHARE_WRITE
                };
                let device_handle = CreateFileW(
                    path.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    share_mode,
                    std::ptr::null_mut(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    0,
                );
                if device_handle == INVALID_HANDLE_VALUE {
                    anyhow::bail!(
                        "USB device is unavailable or not claimable by WinUSB: {}",
                        std::io::Error::last_os_error()
                    );
                }

                let mut interface_handle = std::ptr::null_mut();
                if WinUsb_Initialize(device_handle, &mut interface_handle) == 0 {
                    let error = std::io::Error::last_os_error();
                    CloseHandle(device_handle);
                    anyhow::bail!("WinUsb_Initialize failed: {error}");
                }

                let mut session = Self {
                    bus_id: bus_id.to_string(),
                    device_handle,
                    interface_handle: interface_handle as usize,
                    endpoints: Vec::new(),
                };
                session.refresh_endpoints()?;
                Ok(session)
            }
        }

        fn refresh_endpoints(&mut self) -> Result<()> {
            unsafe {
                let mut descriptor = UsbInterfaceDescriptorRaw::default();
                if WinUsb_QueryInterfaceSettings(self.interface_handle(), 0, &mut descriptor) == 0 {
                    return Err(std::io::Error::last_os_error())
                        .context("WinUsb_QueryInterfaceSettings failed");
                }

                let mut endpoints = Vec::new();
                for index in 0..descriptor.num_endpoints {
                    let mut pipe = WinUsbPipeInformation::default();
                    if WinUsb_QueryPipe(self.interface_handle(), 0, index, &mut pipe) == 0 {
                        tracing::debug!(
                            "WinUsb_QueryPipe({}) failed for {}: {}",
                            index,
                            self.bus_id,
                            std::io::Error::last_os_error()
                        );
                        continue;
                    }
                    if let Some(endpoint) = endpoint_from_pipe(descriptor.interface_number, pipe) {
                        endpoints.push(endpoint);
                    }
                }
                self.endpoints = endpoints;
                Ok(())
            }
        }

        fn submit_transfer(
            &mut self,
            transfer: &UsbTransferPayload,
        ) -> Result<UsbTransferCompletion> {
            match transfer.transfer_kind {
                UsbTransferKind::Control => self.control_transfer(transfer),
                UsbTransferKind::Bulk | UsbTransferKind::Interrupt => self.pipe_transfer(transfer),
                UsbTransferKind::Isochronous => anyhow::bail!(
                    "Isochronous USB forwarding is not supported by the WinUSB runtime yet"
                ),
            }
        }

        fn control_transfer(
            &mut self,
            transfer: &UsbTransferPayload,
        ) -> Result<UsbTransferCompletion> {
            let setup = transfer
                .control_setup_packet()
                .ok_or_else(|| anyhow::anyhow!("USB control transfer is missing setup packet"))?;
            let expected_len = transfer
                .expected_length
                .unwrap_or(setup.length as u32)
                .min(capabilities().max_transfer_size);
            let mut data = if matches!(transfer.direction, UsbTransferDirection::In) {
                vec![0u8; expected_len as usize]
            } else {
                transfer.data.clone()
            };
            let mut transferred = 0u32;
            let raw_setup = UsbSetupPacketRaw {
                request_type: setup.request_type,
                request: setup.request,
                value: setup.value,
                index: setup.index,
                length: setup.length,
            };
            unsafe {
                if WinUsb_ControlTransfer(
                    self.interface_handle(),
                    raw_setup,
                    data.as_mut_ptr(),
                    data.len() as u32,
                    &mut transferred,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return Err(std::io::Error::last_os_error())
                        .context("WinUsb_ControlTransfer failed");
                }
            }
            if matches!(transfer.direction, UsbTransferDirection::In) {
                data.truncate(transferred as usize);
            } else {
                data.clear();
            }
            Ok(completion_from_transfer(
                transfer,
                0,
                UsbTransferStatus::Completed,
                transferred,
                data,
            ))
        }

        fn pipe_transfer(
            &mut self,
            transfer: &UsbTransferPayload,
        ) -> Result<UsbTransferCompletion> {
            let mut transferred = 0u32;
            match transfer.direction {
                UsbTransferDirection::In => {
                    let expected_len = transfer
                        .expected_length
                        .unwrap_or_else(|| {
                            endpoint_packet_size(&self.endpoints, transfer.endpoint_address) as u32
                        })
                        .max(1)
                        .min(capabilities().max_transfer_size);
                    let mut data = vec![0u8; expected_len as usize];
                    unsafe {
                        if WinUsb_ReadPipe(
                            self.interface_handle(),
                            transfer.endpoint_address,
                            data.as_mut_ptr(),
                            data.len() as u32,
                            &mut transferred,
                            std::ptr::null_mut(),
                        ) == 0
                        {
                            return Err(std::io::Error::last_os_error())
                                .context("WinUsb_ReadPipe failed");
                        }
                    }
                    data.truncate(transferred as usize);
                    Ok(completion_from_transfer(
                        transfer,
                        0,
                        UsbTransferStatus::Completed,
                        transferred,
                        data,
                    ))
                }
                UsbTransferDirection::Out => {
                    let mut data = transfer.data.clone();
                    unsafe {
                        if WinUsb_WritePipe(
                            self.interface_handle(),
                            transfer.endpoint_address,
                            data.as_mut_ptr(),
                            data.len() as u32,
                            &mut transferred,
                            std::ptr::null_mut(),
                        ) == 0
                        {
                            return Err(std::io::Error::last_os_error())
                                .context("WinUsb_WritePipe failed");
                        }
                    }
                    Ok(completion_from_transfer(
                        transfer,
                        0,
                        UsbTransferStatus::Completed,
                        transferred,
                        Vec::new(),
                    ))
                }
            }
        }

        fn reset_pipe(&mut self, endpoint_address: u8) -> Result<()> {
            unsafe {
                if WinUsb_ResetPipe(self.interface_handle(), endpoint_address) == 0 {
                    return Err(std::io::Error::last_os_error()).context("WinUsb_ResetPipe failed");
                }
            }
            Ok(())
        }

        fn abort_pipe(&mut self, endpoint_address: u8) -> Result<()> {
            unsafe {
                if WinUsb_AbortPipe(self.interface_handle(), endpoint_address) == 0 {
                    return Err(std::io::Error::last_os_error()).context("WinUsb_AbortPipe failed");
                }
            }
            Ok(())
        }

        fn interface_handle(&self) -> *mut c_void {
            self.interface_handle as *mut c_void
        }
    }

    impl Drop for WindowsUsbSession {
        fn drop(&mut self) {
            unsafe {
                if self.interface_handle != 0 {
                    WinUsb_Free(self.interface_handle());
                }
                if self.device_handle != INVALID_HANDLE_VALUE {
                    CloseHandle(self.device_handle);
                }
            }
        }
    }

    fn completion_from_transfer(
        transfer: &UsbTransferPayload,
        status: i32,
        transfer_status: UsbTransferStatus,
        actual_length: u32,
        data: Vec<u8>,
    ) -> UsbTransferCompletion {
        UsbTransferCompletion {
            transfer_id: transfer.transfer_id,
            bus_id: transfer.bus_id.clone(),
            status,
            transfer_status,
            endpoint_address: Some(transfer.endpoint_address),
            transfer_kind: Some(transfer.transfer_kind),
            actual_length: Some(actual_length),
            data,
        }
    }

    fn endpoint_packet_size(endpoints: &[UsbEndpointDescriptor], endpoint_address: u8) -> u16 {
        endpoints
            .iter()
            .find(|endpoint| endpoint.address == endpoint_address)
            .map(|endpoint| endpoint.max_packet_size)
            .unwrap_or(64)
    }

    fn endpoint_from_pipe(
        interface_number: u8,
        pipe: WinUsbPipeInformation,
    ) -> Option<UsbEndpointDescriptor> {
        let transfer_kind = match pipe.pipe_type {
            PIPE_TYPE_BULK => UsbTransferKind::Bulk,
            PIPE_TYPE_INTERRUPT => UsbTransferKind::Interrupt,
            PIPE_TYPE_ISOCHRONOUS => UsbTransferKind::Isochronous,
            _ => return None,
        };
        let direction = if (pipe.pipe_id & 0x80) != 0 {
            UsbTransferDirection::In
        } else {
            UsbTransferDirection::Out
        };
        Some(UsbEndpointDescriptor {
            address: pipe.pipe_id,
            interface_number,
            alternate_setting: 0,
            transfer_kind,
            direction,
            max_packet_size: pipe.maximum_packet_size,
            interval_ms: Some(pipe.interval),
            attributes: pipe.pipe_type as u8,
            max_burst: None,
            max_streams: None,
        })
    }

    unsafe fn device_path_for_interface(
        set: isize,
        interface_data: &mut SpDeviceInterfaceData,
    ) -> Result<String> {
        let mut required = 0u32;
        let _ = SetupDiGetDeviceInterfaceDetailW(
            set,
            interface_data,
            std::ptr::null_mut(),
            0,
            &mut required,
            std::ptr::null_mut(),
        );
        if required == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER) {
                return Err(error).context("SetupDiGetDeviceInterfaceDetailW size query failed");
            }
        }

        let mut buffer = vec![0u8; required as usize];
        let detail = buffer.as_mut_ptr() as *mut SpDeviceInterfaceDetailDataW;
        (*detail).cb_size = if cfg!(target_pointer_width = "64") {
            8
        } else {
            6
        };
        if SetupDiGetDeviceInterfaceDetailW(
            set,
            interface_data,
            detail,
            required,
            &mut required,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error())
                .context("SetupDiGetDeviceInterfaceDetailW failed");
        }

        let path_ptr = (*detail).device_path.as_ptr();
        let max_chars = (required as usize).saturating_sub(4) / 2;
        let len = (0..max_chars)
            .find(|offset| *path_ptr.add(*offset) == 0)
            .unwrap_or(max_chars);
        Ok(String::from_utf16_lossy(std::slice::from_raw_parts(
            path_ptr, len,
        )))
    }

    fn parse_vid_pid(device_path: &str) -> Option<(u16, u16)> {
        let lower = device_path.to_ascii_lowercase();
        let vid = parse_hex_after(&lower, "vid_")?;
        let pid = parse_hex_after(&lower, "pid_")?;
        Some((vid, pid))
    }

    fn parse_hex_after(value: &str, marker: &str) -> Option<u16> {
        let start = value.find(marker)? + marker.len();
        let hex = value.get(start..start + 4)?;
        u16::from_str_radix(hex, 16).ok()
    }

    fn parse_serial_from_device_path(device_path: &str) -> Option<String> {
        let trimmed = device_path.trim_matches('#');
        let mut parts = trimmed.split('#');
        let _prefix = parts.next()?;
        let _id = parts.next()?;
        parts.next().map(|serial| serial.to_string())
    }

    fn format_usb_product_label(vendor_id: u16, product_id: u16) -> String {
        if vendor_id == 0 && product_id == 0 {
            "USB device".to_string()
        } else {
            format!("USB device {:04x}:{:04x}", vendor_id, product_id)
        }
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct DeviceInfoSetGuard(isize);

    impl Drop for DeviceInfoSetGuard {
        fn drop(&mut self) {
            unsafe {
                SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct SpDeviceInterfaceData {
        cb_size: u32,
        interface_class_guid: Guid,
        flags: u32,
        reserved: usize,
    }

    #[repr(C)]
    struct SpDeviceInterfaceDetailDataW {
        cb_size: u32,
        device_path: [u16; 1],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct SpDevInfoData {
        cb_size: u32,
        class_guid: Guid,
        dev_inst: u32,
        reserved: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct UsbSetupPacketRaw {
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct UsbInterfaceDescriptorRaw {
        length: u8,
        descriptor_type: u8,
        interface_number: u8,
        alternate_setting: u8,
        num_endpoints: u8,
        interface_class: u8,
        interface_sub_class: u8,
        interface_protocol: u8,
        interface: u8,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct WinUsbPipeInformation {
        pipe_type: u32,
        pipe_id: u8,
        maximum_packet_size: u16,
        interval: u8,
    }

    #[link(name = "setupapi")]
    extern "system" {
        fn SetupDiGetClassDevsW(
            class_guid: *const Guid,
            enumerator: *const u16,
            hwnd_parent: isize,
            flags: u32,
        ) -> isize;
        fn SetupDiEnumDeviceInterfaces(
            device_info_set: isize,
            device_info_data: *mut SpDevInfoData,
            interface_class_guid: *const Guid,
            member_index: u32,
            device_interface_data: *mut SpDeviceInterfaceData,
        ) -> i32;
        fn SetupDiGetDeviceInterfaceDetailW(
            device_info_set: isize,
            device_interface_data: *mut SpDeviceInterfaceData,
            device_interface_detail_data: *mut SpDeviceInterfaceDetailDataW,
            device_interface_detail_data_size: u32,
            required_size: *mut u32,
            device_info_data: *mut SpDevInfoData,
        ) -> i32;
        fn SetupDiDestroyDeviceInfoList(device_info_set: isize) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *mut c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: isize,
        ) -> isize;
        fn CloseHandle(handle: isize) -> i32;
    }

    #[link(name = "winusb")]
    extern "system" {
        fn WinUsb_Initialize(device_handle: isize, interface_handle: *mut *mut c_void) -> i32;
        fn WinUsb_Free(interface_handle: *mut c_void) -> i32;
        fn WinUsb_QueryInterfaceSettings(
            interface_handle: *mut c_void,
            alternate_interface_number: u8,
            usb_alt_interface_descriptor: *mut UsbInterfaceDescriptorRaw,
        ) -> i32;
        fn WinUsb_QueryPipe(
            interface_handle: *mut c_void,
            alternate_interface_number: u8,
            pipe_index: u8,
            pipe_information: *mut WinUsbPipeInformation,
        ) -> i32;
        fn WinUsb_ControlTransfer(
            interface_handle: *mut c_void,
            setup_packet: UsbSetupPacketRaw,
            buffer: *mut u8,
            buffer_length: u32,
            length_transferred: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn WinUsb_ReadPipe(
            interface_handle: *mut c_void,
            pipe_id: u8,
            buffer: *mut u8,
            buffer_length: u32,
            length_transferred: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn WinUsb_WritePipe(
            interface_handle: *mut c_void,
            pipe_id: u8,
            buffer: *mut u8,
            buffer_length: u32,
            length_transferred: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn WinUsb_ResetPipe(interface_handle: *mut c_void, pipe_id: u8) -> i32;
        fn WinUsb_AbortPipe(interface_handle: *mut c_void, pipe_id: u8) -> i32;
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_vid_pid_from_windows_usb_path() {
            let path = r#"\\?\usb#vid_045e&pid_028e#123456#{a5dcbf10-6530-11d2-901f-00c04fb951ed}"#;
            assert_eq!(parse_vid_pid(path), Some((0x045e, 0x028e)));
            assert_eq!(
                parse_serial_from_device_path(path),
                Some("123456".to_string())
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub struct PlatformUsbHostRuntime;

    impl PlatformUsbHostRuntime {
        pub fn new() -> Self {
            Self
        }

        pub fn claim_device(&mut self, request: UsbDeviceClaimRequest) -> UsbDeviceClaimResponse {
            UsbDeviceClaimResponse {
                request_id: request.request_id,
                bus_id: request.bus_id,
                accepted: false,
                session_id: None,
                granted_interfaces: Vec::new(),
                message: Some(
                    "Experimental USB forwarding host runtime is only implemented on Windows"
                        .to_string(),
                ),
            }
        }

        pub fn submit_transfer(
            &mut self,
            transfer: &UsbTransferPayload,
        ) -> Result<UsbTransferCompletion> {
            let _ = transfer;
            anyhow::bail!("Experimental USB forwarding host runtime is only implemented on Windows")
        }

        pub fn release_device(&mut self, _session_id: Uuid) -> Result<()> {
            anyhow::bail!("Experimental USB forwarding host runtime is only implemented on Windows")
        }

        pub fn reset_device(
            &mut self,
            _session_id: Option<Uuid>,
            _bus_id: &str,
            _reset_kind: UsbDeviceResetKind,
        ) -> Result<()> {
            anyhow::bail!("Experimental USB forwarding host runtime is only implemented on Windows")
        }

        pub fn cancel_transfer(&mut self, _transfer_id: u64, _bus_id: &str) -> Result<()> {
            anyhow::bail!("Experimental USB forwarding host runtime is only implemented on Windows")
        }
    }

    pub fn capabilities() -> UsbForwardingCapabilities {
        UsbForwardingCapabilities::default()
    }

    pub fn enumerate_devices() -> Result<Vec<UsbDeviceDescriptor>> {
        Ok(Vec::new())
    }
}
