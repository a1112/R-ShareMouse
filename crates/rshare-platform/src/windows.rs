//! Windows platform-specific implementations

cfg_if::cfg_if! {
    if #[cfg(windows)] {
        pub use windows_impl::*;
    }
}

#[cfg(windows)]
mod windows_impl {
    use anyhow::{Context, Result};
    use rshare_core::LocalHardwareDevice;
    use std::cell::RefCell;
    use std::fmt;
    use std::mem::size_of;
    use std::sync::{mpsc, Arc};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    type WindowsInputCallback = Arc<dyn Fn(WindowsInputEvent) + Send + Sync + 'static>;

    thread_local! {
        static WINDOWS_HOOK_CALLBACK: RefCell<Option<WindowsInputCallback>> = RefCell::new(None);
    }

    #[derive(Debug)]
    struct DeviceIoControlError {
        ioctl: u32,
        source: std::io::Error,
    }

    impl fmt::Display for DeviceIoControlError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "DeviceIoControl(0x{:08x}) failed: {}",
                self.ioctl, self.source
            )
        }
    }

    impl std::error::Error for DeviceIoControlError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.source)
        }
    }

    pub fn is_driver_event_queue_empty(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            cause
                .downcast_ref::<DeviceIoControlError>()
                .map(|io_error| {
                    io_error.ioctl == IOCTL_RSHARE_READ_EVENT
                        && io_error.source.raw_os_error() == Some(ERROR_NO_MORE_ITEMS)
                })
                .unwrap_or(false)
        })
    }

    /// Enumerate physical keyboard and mouse devices through Windows Raw Input.
    pub fn enumerate_raw_input_devices(
    ) -> Result<(Vec<LocalHardwareDevice>, Vec<LocalHardwareDevice>)> {
        unsafe {
            let mut device_count = 0u32;
            let list_entry_size = size_of::<RawInputDeviceList>() as u32;
            let query =
                GetRawInputDeviceList(std::ptr::null_mut(), &mut device_count, list_entry_size);
            if query == u32::MAX {
                anyhow::bail!(
                    "GetRawInputDeviceList count query failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            if device_count == 0 {
                return Ok((Vec::new(), Vec::new()));
            }

            let mut devices = vec![
                RawInputDeviceList {
                    h_device: 0,
                    dw_type: 0,
                };
                device_count as usize
            ];
            let read =
                GetRawInputDeviceList(devices.as_mut_ptr(), &mut device_count, list_entry_size);
            if read == u32::MAX {
                anyhow::bail!(
                    "GetRawInputDeviceList failed: {}",
                    std::io::Error::last_os_error()
                );
            }

            let mut keyboards = Vec::new();
            let mut mice = Vec::new();
            for (index, device) in devices.into_iter().take(read as usize).enumerate() {
                let Some(kind) = raw_input_kind(device.dw_type) else {
                    continue;
                };
                let raw_name = raw_input_device_name(device.h_device)
                    .unwrap_or_else(|| format!("Raw Input {kind} {}", index + 1));
                let item = LocalHardwareDevice {
                    id: format!("raw-input:{}:{}", kind.to_lowercase(), device.h_device),
                    name: friendly_raw_input_name(&raw_name, kind, index),
                    source: "Windows Raw Input".to_string(),
                    connected: true,
                    driver_detail: Some(raw_name),
                    device_instance_id: None,
                    capture_path: Some("raw-input".to_string()),
                    event_count: 0,
                    last_event_ms: 0,
                    capabilities: vec!["enumeration".to_string()],
                };
                if device.dw_type == RIM_TYPEKEYBOARD {
                    keyboards.push(item);
                } else {
                    mice.push(item);
                }
            }

            Ok((keyboards, mice))
        }
    }

    unsafe fn raw_input_device_name(handle: isize) -> Option<String> {
        let mut len = 0u32;
        let query = GetRawInputDeviceInfoW(handle, RIDI_DEVICENAME, std::ptr::null_mut(), &mut len);
        if query == u32::MAX || len == 0 {
            return None;
        }
        let mut buffer = vec![0u16; len as usize];
        let result = GetRawInputDeviceInfoW(
            handle,
            RIDI_DEVICENAME,
            buffer.as_mut_ptr().cast(),
            &mut len,
        );
        if result == u32::MAX {
            return None;
        }
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        Some(String::from_utf16_lossy(&buffer))
    }

    fn raw_input_kind(device_type: u32) -> Option<&'static str> {
        match device_type {
            RIM_TYPEMOUSE => Some("Mouse"),
            RIM_TYPEKEYBOARD => Some("Keyboard"),
            _ => None,
        }
    }

    fn friendly_raw_input_name(raw_name: &str, kind: &str, index: usize) -> String {
        raw_name
            .rsplit('#')
            .find(|part| {
                !part.is_empty()
                    && !part.eq_ignore_ascii_case("kbd")
                    && !part.eq_ignore_ascii_case("mi")
            })
            .map(|part| part.replace('&', " "))
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("{kind} {}", index + 1))
    }

    /// Input event captured by the native Windows low-level hooks.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WindowsInputEvent {
        MouseMove { x: i32, y: i32 },
        MouseButton { button: u8, down: bool },
        MouseWheel { delta_x: i32, delta_y: i32 },
        Key { vk: u32, down: bool },
    }

    pub const RSHARE_DRIVER_DEVICE_PATH: &str = r"\\.\RShareInputControl";
    pub const RSHARE_VHID_DRIVER_DEVICE_PATH: &str = r"\\.\RShareVirtualHidControl";

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WindowsDriverVersion {
        pub major: u16,
        pub minor: u16,
        pub patch: u16,
        pub abi: u16,
    }

    impl WindowsDriverVersion {
        pub fn display(&self) -> String {
            format!(
                "{}.{}.{} abi {}",
                self.major, self.minor, self.patch, self.abi
            )
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WindowsDriverCapabilities {
        pub filter_events: bool,
        pub virtual_keyboard: bool,
        pub virtual_mouse: bool,
        pub virtual_gamepad_scaffold: bool,
        pub max_event_size: u32,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum WindowsDriverDeviceKind {
        Keyboard,
        Mouse,
        Gamepad,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum WindowsDriverEventKind {
        Key,
        MouseMove,
        MouseButton,
        MouseWheel,
        Synthetic,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum WindowsDriverEventSource {
        Hardware,
        InjectedLoopback,
        DriverTest,
        VirtualDevice,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WindowsDriverInputEvent {
        pub source: WindowsDriverEventSource,
        pub device_kind: WindowsDriverDeviceKind,
        pub event_kind: WindowsDriverEventKind,
        pub device_id: String,
        pub device_instance_id: String,
        pub value0: i32,
        pub value1: i32,
        pub value2: i32,
        pub flags: u32,
        pub timestamp_us: u64,
    }

    pub struct WindowsDriverClient {
        handle: isize,
        device_path: &'static str,
    }

    impl std::fmt::Debug for WindowsDriverClient {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("WindowsDriverClient")
                .field("device_path", &self.device_path)
                .field(
                    "open",
                    &(self.handle != INVALID_HANDLE_VALUE && self.handle != 0),
                )
                .finish()
        }
    }

    impl WindowsDriverClient {
        pub fn open() -> Result<Self> {
            Self::open_filter()
        }

        pub fn open_filter() -> Result<Self> {
            Self::open_path(
                RSHARE_DRIVER_DEVICE_PATH,
                "RShare filter driver control device",
            )
        }

        pub fn open_vhid() -> Result<Self> {
            Self::open_path(
                RSHARE_VHID_DRIVER_DEVICE_PATH,
                "RShare virtual HID driver control device",
            )
        }

        fn open_path(device_path: &'static str, label: &str) -> Result<Self> {
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
                        "{} is unavailable: {}",
                        label,
                        std::io::Error::last_os_error()
                    );
                }
                Ok(Self {
                    handle,
                    device_path,
                })
            }
        }

        pub fn query_version(&self) -> Result<WindowsDriverVersion> {
            let mut raw = RShareDriverVersionRaw::default();
            unsafe {
                device_io_control(
                    self.handle,
                    IOCTL_RSHARE_QUERY_VERSION,
                    std::ptr::null_mut(),
                    0,
                    (&mut raw as *mut RShareDriverVersionRaw).cast(),
                    size_of::<RShareDriverVersionRaw>() as u32,
                )?;
            }
            Ok(WindowsDriverVersion {
                major: raw.major,
                minor: raw.minor,
                patch: raw.patch,
                abi: raw.abi,
            })
        }

        pub fn query_capabilities(&self) -> Result<WindowsDriverCapabilities> {
            let mut raw = RShareDriverCapabilitiesRaw::default();
            unsafe {
                device_io_control(
                    self.handle,
                    IOCTL_RSHARE_QUERY_CAPABILITIES,
                    std::ptr::null_mut(),
                    0,
                    (&mut raw as *mut RShareDriverCapabilitiesRaw).cast(),
                    size_of::<RShareDriverCapabilitiesRaw>() as u32,
                )?;
            }
            Ok(WindowsDriverCapabilities {
                filter_events: raw.flags & RSHARE_CAP_FILTER_EVENTS != 0,
                virtual_keyboard: raw.flags & RSHARE_CAP_VIRTUAL_KEYBOARD != 0,
                virtual_mouse: raw.flags & RSHARE_CAP_VIRTUAL_MOUSE != 0,
                virtual_gamepad_scaffold: raw.flags & RSHARE_CAP_VIRTUAL_GAMEPAD_SCAFFOLD != 0,
                max_event_size: raw.max_event_size,
            })
        }

        pub fn read_event(&self) -> Result<WindowsDriverInputEvent> {
            let mut raw = RShareDriverEventRaw::default();
            unsafe {
                device_io_control(
                    self.handle,
                    IOCTL_RSHARE_READ_EVENT,
                    std::ptr::null_mut(),
                    0,
                    (&mut raw as *mut RShareDriverEventRaw).cast(),
                    size_of::<RShareDriverEventRaw>() as u32,
                )?;
            }
            raw.try_into()
        }

        pub fn inject_keyboard(&self, vk: u16, pressed: bool) -> Result<()> {
            let mut raw = RShareInjectReportRaw {
                report_kind: RSHARE_REPORT_KEYBOARD,
                value0: vk as i32,
                value1: if pressed { 1 } else { 0 },
                value2: 0,
                flags: 0,
            };
            unsafe {
                device_io_control(
                    self.handle,
                    IOCTL_RSHARE_INJECT_REPORT,
                    (&mut raw as *mut RShareInjectReportRaw).cast(),
                    size_of::<RShareInjectReportRaw>() as u32,
                    std::ptr::null_mut(),
                    0,
                )
            }
        }

        pub fn inject_mouse_move(&self, dx: i32, dy: i32) -> Result<()> {
            let mut raw = RShareInjectReportRaw {
                report_kind: RSHARE_REPORT_MOUSE_MOVE,
                value0: dx,
                value1: dy,
                value2: 0,
                flags: 0,
            };
            unsafe {
                device_io_control(
                    self.handle,
                    IOCTL_RSHARE_INJECT_REPORT,
                    (&mut raw as *mut RShareInjectReportRaw).cast(),
                    size_of::<RShareInjectReportRaw>() as u32,
                    std::ptr::null_mut(),
                    0,
                )
            }
        }

        /// Inject a mouse button event through the virtual HID driver.
        ///
        /// # Arguments
        /// * `button` - Button code (1=left, 2=middle, 3=right, 4/5=X buttons)
        /// * `pressed` - True for button down, false for button up
        pub fn inject_mouse_button(&self, button: u8, pressed: bool) -> Result<()> {
            let mut raw = RShareInjectReportRaw {
                report_kind: RSHARE_REPORT_MOUSE_BUTTON,
                value0: button as i32,
                value1: if pressed { 1 } else { 0 },
                value2: 0,
                flags: 0,
            };
            unsafe {
                device_io_control(
                    self.handle,
                    IOCTL_RSHARE_INJECT_REPORT,
                    (&mut raw as *mut RShareInjectReportRaw).cast(),
                    size_of::<RShareInjectReportRaw>() as u32,
                    std::ptr::null_mut(),
                    0,
                )
            }
        }

        /// Inject a mouse wheel event through the virtual HID driver.
        ///
        /// # Arguments
        /// * `delta_x` - Horizontal scroll delta (positive = right)
        /// * `delta_y` - Vertical scroll delta (positive = up)
        pub fn inject_mouse_wheel(&self, delta_x: i32, delta_y: i32) -> Result<()> {
            let mut raw = RShareInjectReportRaw {
                report_kind: RSHARE_REPORT_MOUSE_WHEEL,
                value0: delta_x,
                value1: delta_y,
                value2: 0,
                flags: 0,
            };
            unsafe {
                device_io_control(
                    self.handle,
                    IOCTL_RSHARE_INJECT_REPORT,
                    (&mut raw as *mut RShareInjectReportRaw).cast(),
                    size_of::<RShareInjectReportRaw>() as u32,
                    std::ptr::null_mut(),
                    0,
                )
            }
        }

        pub fn emit_test_packet(&self, device_kind: WindowsDriverDeviceKind) -> Result<()> {
            let mut raw = RShareTestPacketRaw {
                device_kind: driver_device_kind_code(device_kind),
                event_kind: RSHARE_EVENT_SYNTHETIC,
                value0: 0,
                value1: 0,
                value2: 0,
            };
            unsafe {
                device_io_control(
                    self.handle,
                    IOCTL_RSHARE_EMIT_TEST_PACKET,
                    (&mut raw as *mut RShareTestPacketRaw).cast(),
                    size_of::<RShareTestPacketRaw>() as u32,
                    std::ptr::null_mut(),
                    0,
                )
            }
        }
    }

    impl Drop for WindowsDriverClient {
        fn drop(&mut self) {
            unsafe {
                if self.handle != INVALID_HANDLE_VALUE && self.handle != 0 {
                    CloseHandle(self.handle);
                }
            }
            self.handle = INVALID_HANDLE_VALUE;
        }
    }

    pub fn probe_rshare_driver() -> rshare_core::LocalDriverDiagnosticState {
        let mut state = rshare_core::LocalDriverDiagnosticState {
            device_path: Some(format!(
                "filter={}, vhid={}",
                RSHARE_DRIVER_DEVICE_PATH, RSHARE_VHID_DRIVER_DEVICE_PATH
            )),
            test_signing_required: true,
            ..rshare_core::LocalDriverDiagnosticState::default()
        };
        let mut errors = Vec::new();

        match WindowsDriverClient::open_filter() {
            Ok(client) => {
                state.status = "available".to_string();
                match client.query_version() {
                    Ok(version) => state.version = Some(version.display()),
                    Err(error) => state.last_error = Some(error.to_string()),
                }
                match client.query_capabilities() {
                    Ok(capabilities) => {
                        state.filter_active = capabilities.filter_events;
                    }
                    Err(error) => errors.push(error.to_string()),
                }
            }
            Err(error) => {
                errors.push(error.to_string());
            }
        }

        match WindowsDriverClient::open_vhid() {
            Ok(client) => {
                state.status = "available".to_string();
                if state.version.is_none() {
                    match client.query_version() {
                        Ok(version) => state.version = Some(version.display()),
                        Err(error) => errors.push(error.to_string()),
                    }
                }
                match client.query_capabilities() {
                    Ok(capabilities) => {
                        state.vhid_active =
                            capabilities.virtual_keyboard || capabilities.virtual_mouse;
                    }
                    Err(error) => errors.push(error.to_string()),
                }
            }
            Err(error) => {
                errors.push(error.to_string());
            }
        }

        if state.status != "available" {
            state.status = "fallback".to_string();
        }
        if !errors.is_empty() {
            state.last_error = Some(errors.join("; "));
        }

        state
    }

    /// Windows input listener using low-level hooks
    pub struct WindowsInputListener {
        runtime: Option<HookRuntime>,
        runner: Box<dyn HookRunner>,
    }

    impl WindowsInputListener {
        pub fn new() -> Self {
            Self {
                runtime: None,
                runner: Box::new(Win32HookRunner),
            }
        }

        #[cfg(test)]
        fn new_with_runner_for_test(runner: Box<dyn HookRunner>) -> Self {
            Self {
                runtime: None,
                runner,
            }
        }

        /// Start listening using Windows hooks
        pub fn start(&mut self) -> Result<()> {
            self.start_with_callback(|_| {})
        }

        /// Start listening and send captured events through the provided channel.
        pub fn start_channel(&mut self, sender: mpsc::Sender<WindowsInputEvent>) -> Result<()> {
            self.start_with_callback(move |event| {
                let _ = sender.send(event);
            })
        }

        /// Start listening and invoke a callback for each captured event.
        pub fn start_with_callback<F>(&mut self, callback: F) -> Result<()>
        where
            F: Fn(WindowsInputEvent) + Send + Sync + 'static,
        {
            if self.is_running() {
                return Ok(());
            }

            tracing::info!("Windows input listener starting");
            self.runtime = Some(self.runner.start(Arc::new(callback))?);
            Ok(())
        }

        /// Stop listening and cleanup hooks
        pub fn stop(&mut self) -> Result<()> {
            if let Some(runtime) = self.runtime.take() {
                runtime.stop()?;
                tracing::info!("Windows input listener stopped");
            }

            Ok(())
        }

        pub fn is_running(&self) -> bool {
            self.runtime
                .as_ref()
                .map(|runtime| runtime.is_running())
                .unwrap_or(false)
        }

        #[cfg(test)]
        fn hook_kinds_for_test(&self) -> Option<(bool, bool)> {
            self.runtime
                .as_ref()
                .map(|runtime| (runtime.mouse_hook != 0, runtime.keyboard_hook != 0))
        }

        /// Get primary screen info (physical resolution, not DPI-scaled)
        pub fn get_screen_info() -> ScreenInfo {
            extern "C" {
                fn GetSystemMetrics(nIndex: i32) -> i32;
                fn GetDC(hwnd: isize) -> isize;
                fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
                fn GetDeviceCaps(hdc: isize, nIndex: i32) -> i32;
            }

            const HORZRES: i32 = 8;
            const VERTRES: i32 = 10;
            const DESKTOPVERTRES: i32 = 117;
            const DESKTOPHORZRES: i32 = 118;

            unsafe {
                // Try to get physical resolution using GetDeviceCaps
                let hdc = GetDC(0);
                if hdc != 0 {
                    let physical_width = GetDeviceCaps(hdc, DESKTOPHORZRES) as u32;
                    let physical_height = GetDeviceCaps(hdc, DESKTOPVERTRES) as u32;
                    let logical_width = GetDeviceCaps(hdc, HORZRES) as u32;
                    let logical_height = GetDeviceCaps(hdc, VERTRES) as u32;
                    ReleaseDC(0, hdc);

                    // If physical resolution is available and different from logical, use it
                    if physical_width > 0 && physical_height > 0 {
                        tracing::debug!(
                            "Screen resolution: physical={}x{}, logical={}x{}",
                            physical_width,
                            physical_height,
                            logical_width,
                            logical_height
                        );
                        return ScreenInfo {
                            x: 0,
                            y: 0,
                            width: physical_width,
                            height: physical_height,
                        };
                    }
                }

                // Fallback to GetSystemMetrics (logical resolution)
                ScreenInfo {
                    x: 0,
                    y: 0,
                    width: GetSystemMetrics(0) as u32, // SM_CXSCREEN
                    height: GetSystemMetrics(1) as u32, // SM_CYSCREEN
                }
            }
        }
    }

    impl Default for WindowsInputListener {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for WindowsInputListener {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    trait HookRunner: Send + Sync {
        fn start(&mut self, callback: WindowsInputCallback) -> Result<HookRuntime>;
    }

    struct HookRuntime {
        mouse_hook: isize,
        keyboard_hook: isize,
        thread_id: u32,
        thread: Option<JoinHandle<()>>,
    }

    impl HookRuntime {
        fn is_running(&self) -> bool {
            self.mouse_hook != 0 && self.keyboard_hook != 0 && self.thread_id != 0
        }

        fn stop(mut self) -> Result<()> {
            unsafe {
                PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
            }

            if let Some(thread) = self.thread.take() {
                thread
                    .join()
                    .map_err(|_| anyhow::anyhow!("Windows hook thread panicked"))?;
            }

            self.mouse_hook = 0;
            self.keyboard_hook = 0;
            self.thread_id = 0;
            Ok(())
        }
    }

    struct Win32HookRunner;

    impl HookRunner for Win32HookRunner {
        fn start(&mut self, callback: WindowsInputCallback) -> Result<HookRuntime> {
            let (tx, rx) = mpsc::channel();

            let thread = thread::Builder::new()
                .name("rshare-windows-input-hooks".to_string())
                .spawn(move || run_hook_thread(tx, callback))
                .context("Failed to spawn Windows hook thread")?;

            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok((mouse_hook, keyboard_hook, thread_id))) => Ok(HookRuntime {
                    mouse_hook,
                    keyboard_hook,
                    thread_id,
                    thread: Some(thread),
                }),
                Ok(Err(error)) => {
                    let _ = thread.join();
                    Err(anyhow::anyhow!(error))
                }
                Err(error) => Err(anyhow::anyhow!(
                    "Windows hook thread did not initialize in time: {error}"
                )),
            }
        }
    }

    fn run_hook_thread(
        tx: mpsc::Sender<std::result::Result<(isize, isize, u32), String>>,
        callback: WindowsInputCallback,
    ) {
        WINDOWS_HOOK_CALLBACK.with(|stored| {
            *stored.borrow_mut() = Some(callback);
        });

        unsafe {
            let thread_id = GetCurrentThreadId();
            let mut message = Message::default();
            PeekMessageW(&mut message as *mut Message, 0, 0, 0, PM_NOREMOVE);

            let mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), 0, 0);
            if mouse_hook == 0 {
                let _ = tx.send(Err(format!(
                    "SetWindowsHookExW(WH_MOUSE_LL) failed: {}",
                    std::io::Error::last_os_error()
                )));
                clear_hook_callback();
                return;
            }

            let keyboard_hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), 0, 0);
            if keyboard_hook == 0 {
                let error = std::io::Error::last_os_error();
                UnhookWindowsHookEx(mouse_hook);
                let _ = tx.send(Err(format!(
                    "SetWindowsHookExW(WH_KEYBOARD_LL) failed: {error}"
                )));
                clear_hook_callback();
                return;
            }

            let _ = tx.send(Ok((mouse_hook, keyboard_hook, thread_id)));

            while GetMessageW(&mut message as *mut Message, 0, 0, 0) > 0 {}

            UnhookWindowsHookEx(mouse_hook);
            UnhookWindowsHookEx(keyboard_hook);
        }

        clear_hook_callback();
    }

    unsafe extern "system" fn mouse_hook_proc(code: i32, w_param: usize, l_param: isize) -> isize {
        if code == HC_ACTION {
            if let Some(event) = unsafe { mouse_event_from_hook(w_param, l_param) } {
                dispatch_hook_event(event);
            }
        }

        CallNextHookEx(0, code, w_param, l_param)
    }

    unsafe extern "system" fn keyboard_hook_proc(
        code: i32,
        w_param: usize,
        l_param: isize,
    ) -> isize {
        if code == HC_ACTION {
            if let Some(event) = unsafe { keyboard_event_from_hook(w_param, l_param) } {
                dispatch_hook_event(event);
            }
        }

        CallNextHookEx(0, code, w_param, l_param)
    }

    fn dispatch_hook_event(event: WindowsInputEvent) {
        WINDOWS_HOOK_CALLBACK.with(|stored| {
            if let Some(callback) = stored.borrow().as_ref() {
                callback(event);
            }
        });
    }

    fn clear_hook_callback() {
        WINDOWS_HOOK_CALLBACK.with(|stored| {
            *stored.borrow_mut() = None;
        });
    }

    unsafe fn mouse_event_from_hook(w_param: usize, l_param: isize) -> Option<WindowsInputEvent> {
        if l_param == 0 {
            return None;
        }

        let event = &*(l_param as *const MouseHookStruct);
        convert_mouse_hook_event(w_param as u32, event)
    }

    unsafe fn keyboard_event_from_hook(
        w_param: usize,
        l_param: isize,
    ) -> Option<WindowsInputEvent> {
        if l_param == 0 {
            return None;
        }

        let event = &*(l_param as *const KeyboardHookStruct);
        convert_keyboard_hook_event(w_param as u32, event)
    }

    fn convert_mouse_hook_event(
        message: u32,
        event: &MouseHookStruct,
    ) -> Option<WindowsInputEvent> {
        if event.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0 {
            return None;
        }

        match message {
            WM_MOUSEMOVE => Some(WindowsInputEvent::MouseMove {
                x: event.pt.x,
                y: event.pt.y,
            }),
            WM_LBUTTONDOWN => Some(WindowsInputEvent::MouseButton {
                button: 1,
                down: true,
            }),
            WM_LBUTTONUP => Some(WindowsInputEvent::MouseButton {
                button: 1,
                down: false,
            }),
            WM_MBUTTONDOWN => Some(WindowsInputEvent::MouseButton {
                button: 2,
                down: true,
            }),
            WM_MBUTTONUP => Some(WindowsInputEvent::MouseButton {
                button: 2,
                down: false,
            }),
            WM_RBUTTONDOWN => Some(WindowsInputEvent::MouseButton {
                button: 3,
                down: true,
            }),
            WM_RBUTTONUP => Some(WindowsInputEvent::MouseButton {
                button: 3,
                down: false,
            }),
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                x_button_from_mouse_data(event.mouse_data).map(|button| {
                    WindowsInputEvent::MouseButton {
                        button,
                        down: message == WM_XBUTTONDOWN,
                    }
                })
            }
            WM_MOUSEWHEEL => Some(WindowsInputEvent::MouseWheel {
                delta_x: 0,
                delta_y: normalize_wheel_delta(high_word_signed(event.mouse_data)),
            }),
            WM_MOUSEHWHEEL => Some(WindowsInputEvent::MouseWheel {
                delta_x: normalize_wheel_delta(high_word_signed(event.mouse_data)),
                delta_y: 0,
            }),
            _ => None,
        }
    }

    fn convert_keyboard_hook_event(
        message: u32,
        event: &KeyboardHookStruct,
    ) -> Option<WindowsInputEvent> {
        if event.flags & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0 {
            return None;
        }

        match message {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some(WindowsInputEvent::Key {
                vk: event.vk_code,
                down: true,
            }),
            WM_KEYUP | WM_SYSKEYUP => Some(WindowsInputEvent::Key {
                vk: event.vk_code,
                down: false,
            }),
            _ => None,
        }
    }

    fn x_button_from_mouse_data(mouse_data: u32) -> Option<u8> {
        match high_word(mouse_data) {
            XBUTTON1 => Some(4),
            XBUTTON2 => Some(5),
            _ => None,
        }
    }

    fn high_word(value: u32) -> u16 {
        ((value >> 16) & 0xffff) as u16
    }

    fn high_word_signed(value: u32) -> i32 {
        high_word(value) as i16 as i32
    }

    fn normalize_wheel_delta(delta: i32) -> i32 {
        let normalized = delta / WHEEL_DELTA;
        if normalized == 0 && delta != 0 {
            delta.signum()
        } else {
            normalized
        }
    }

    /// Windows input emulator using SendInput
    pub struct WindowsInputEmulator {
        active: bool,
        screen_width: u32,
        screen_height: u32,
    }

    impl WindowsInputEmulator {
        pub fn new() -> Self {
            extern "C" {
                fn GetSystemMetrics(nIndex: i32) -> i32;
            }

            unsafe {
                Self {
                    active: false,
                    screen_width: GetSystemMetrics(0) as u32,
                    screen_height: GetSystemMetrics(1) as u32,
                }
            }
        }

        pub fn activate(&mut self) -> Result<()> {
            self.active = true;
            tracing::info!("Windows input emulator activated");
            Ok(())
        }

        pub fn deactivate(&mut self) -> Result<()> {
            self.active = false;
            tracing::info!("Windows input emulator deactivated");
            Ok(())
        }

        pub fn is_active(&self) -> bool {
            self.active
        }

        /// Send absolute mouse move
        pub fn send_mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            tracing::trace!("Windows: mouse move to ({}, {})", x, y);

            // Convert to normalized coordinates (0-65535)
            let sw = self.screen_width.saturating_sub(1) as i32;
            let sh = self.screen_height.saturating_sub(1) as i32;
            let nx = if sw > 0 {
                (x * 65535 / sw).max(0).min(65535)
            } else {
                0
            };
            let ny = if sh > 0 {
                (y * 65535 / sh).max(0).min(65535)
            } else {
                0
            };

            unsafe {
                send_mouse_move_input(nx, ny)?;
            }

            Ok(())
        }

        /// Send mouse button event
        pub fn send_button(&mut self, button: u8, down: bool) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            tracing::trace!(
                "Windows: mouse button {} {}",
                button,
                if down { "down" } else { "up" }
            );

            let (flags, data) = match button {
                1 => {
                    if down {
                        (MOUSEEVENTF_LEFTDOWN, 0)
                    } else {
                        (MOUSEEVENTF_LEFTUP, 0)
                    }
                }
                2 => {
                    if down {
                        (MOUSEEVENTF_MIDDLEDOWN, 0)
                    } else {
                        (MOUSEEVENTF_MIDDLEUP, 0)
                    }
                }
                3 => {
                    if down {
                        (MOUSEEVENTF_RIGHTDOWN, 0)
                    } else {
                        (MOUSEEVENTF_RIGHTUP, 0)
                    }
                }
                4 => {
                    if down {
                        (MOUSEEVENTF_XDOWN, 1)
                    } else {
                        (MOUSEEVENTF_XUP, 1)
                    }
                }
                5 => {
                    if down {
                        (MOUSEEVENTF_XDOWN, 2)
                    } else {
                        (MOUSEEVENTF_XUP, 2)
                    }
                }
                _ => return Ok(()),
            };

            unsafe {
                send_mouse_button_input(flags, data)?;
            }

            Ok(())
        }

        /// Send mouse wheel event
        pub fn send_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            tracing::trace!("Windows: mouse wheel ({}, {})", delta_x, delta_y);

            if delta_y != 0 {
                unsafe {
                    send_wheel_input((delta_y * WHEEL_DELTA) as u32, MOUSEEVENTF_WHEEL)?;
                }
            }

            if delta_x != 0 {
                unsafe {
                    send_wheel_input((delta_x * WHEEL_DELTA) as u32, MOUSEEVENTF_HWHEEL)?;
                }
            }

            Ok(())
        }

        /// Send keyboard event
        pub fn send_key(&mut self, vk: u16, down: bool) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            tracing::trace!("Windows: key {} {}", vk, if down { "down" } else { "up" });

            unsafe {
                send_key_input(vk, down)?;
            }

            Ok(())
        }
    }

    impl Default for WindowsInputEmulator {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Screen information on Windows
    #[derive(Debug, Clone)]
    pub struct ScreenInfo {
        pub x: i32,
        pub y: i32,
        pub width: u32,
        pub height: u32,
    }

    // Windows API constants

    const RSHARE_DRIVER_ABI: u16 = 1;
    const RSHARE_CAP_FILTER_EVENTS: u32 = 0x0000_0001;
    const RSHARE_CAP_VIRTUAL_KEYBOARD: u32 = 0x0000_0002;
    const RSHARE_CAP_VIRTUAL_MOUSE: u32 = 0x0000_0004;
    const RSHARE_CAP_VIRTUAL_GAMEPAD_SCAFFOLD: u32 = 0x0000_0008;

    const RSHARE_SOURCE_HARDWARE: u16 = 1;
    const RSHARE_SOURCE_INJECTED_LOOPBACK: u16 = 2;
    const RSHARE_SOURCE_DRIVER_TEST: u16 = 3;
    const RSHARE_SOURCE_VIRTUAL_DEVICE: u16 = 4;

    const RSHARE_DEVICE_KEYBOARD: u32 = 1;
    const RSHARE_DEVICE_MOUSE: u32 = 2;
    const RSHARE_DEVICE_GAMEPAD: u32 = 3;

    const RSHARE_EVENT_KEY: u32 = 1;
    const RSHARE_EVENT_MOUSE_MOVE: u32 = 2;
    const RSHARE_EVENT_MOUSE_BUTTON: u32 = 3;
    const RSHARE_EVENT_MOUSE_WHEEL: u32 = 4;
    const RSHARE_EVENT_SYNTHETIC: u32 = 5;

    const RSHARE_REPORT_KEYBOARD: u32 = 1;
    const RSHARE_REPORT_MOUSE_MOVE: u32 = 2;
    const RSHARE_REPORT_MOUSE_BUTTON: u32 = 3;
    const RSHARE_REPORT_MOUSE_WHEEL: u32 = 4;

    const FILE_DEVICE_UNKNOWN: u32 = 0x0000_0022;
    const METHOD_BUFFERED: u32 = 0;
    const FILE_ANY_ACCESS: u32 = 0;
    const FILE_READ_DATA: u32 = 0x0001;
    const FILE_WRITE_DATA: u32 = 0x0002;

    const IOCTL_RSHARE_QUERY_VERSION: u32 =
        ctl_code(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_ANY_ACCESS);
    const IOCTL_RSHARE_QUERY_CAPABILITIES: u32 =
        ctl_code(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_ANY_ACCESS);
    const IOCTL_RSHARE_READ_EVENT: u32 =
        ctl_code(FILE_DEVICE_UNKNOWN, 0x803, METHOD_BUFFERED, FILE_READ_DATA);
    const IOCTL_RSHARE_INJECT_REPORT: u32 =
        ctl_code(FILE_DEVICE_UNKNOWN, 0x804, METHOD_BUFFERED, FILE_WRITE_DATA);
    const IOCTL_RSHARE_EMIT_TEST_PACKET: u32 =
        ctl_code(FILE_DEVICE_UNKNOWN, 0x805, METHOD_BUFFERED, FILE_WRITE_DATA);
    const ERROR_NO_MORE_ITEMS: i32 = 259;

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const OPEN_EXISTING: u32 = 3;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    const INVALID_HANDLE_VALUE: isize = -1isize;

    const MOUSEEVENTF_MOVE: u32 = 0x0001;
    const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
    const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
    const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
    const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
    const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
    const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
    const MOUSEEVENTF_XDOWN: u32 = 0x0080;
    const MOUSEEVENTF_XUP: u32 = 0x0100;
    const MOUSEEVENTF_WHEEL: u32 = 0x0800;
    const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
    const MOUSEEVENTF_HWHEEL: u32 = 0x1000;

    const WHEEL_DELTA: i32 = 120;

    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const INPUT_MOUSE: u32 = 0;
    const INPUT_KEYBOARD: u32 = 1;
    const HC_ACTION: i32 = 0;
    const WH_MOUSE_LL: i32 = 14;
    const WH_KEYBOARD_LL: i32 = 13;
    const LLMHF_INJECTED: u32 = 0x00000001;
    const LLMHF_LOWER_IL_INJECTED: u32 = 0x00000002;
    const LLKHF_LOWER_IL_INJECTED: u32 = 0x00000002;
    const LLKHF_INJECTED: u32 = 0x00000010;
    const WM_QUIT: u32 = 0x0012;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_KEYUP: u32 = 0x0101;
    const WM_SYSKEYDOWN: u32 = 0x0104;
    const WM_SYSKEYUP: u32 = 0x0105;
    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_RBUTTONDOWN: u32 = 0x0204;
    const WM_RBUTTONUP: u32 = 0x0205;
    const WM_MBUTTONDOWN: u32 = 0x0207;
    const WM_MBUTTONUP: u32 = 0x0208;
    const WM_MOUSEWHEEL: u32 = 0x020A;
    const WM_XBUTTONDOWN: u32 = 0x020B;
    const WM_XBUTTONUP: u32 = 0x020C;
    const WM_MOUSEHWHEEL: u32 = 0x020E;
    const PM_NOREMOVE: u32 = 0x0000;
    const XBUTTON1: u16 = 1;
    const XBUTTON2: u16 = 2;
    const MONITORINFOF_PRIMARY: u32 = 0x0000_0001;
    const RIM_TYPEMOUSE: u32 = 0;
    const RIM_TYPEKEYBOARD: u32 = 1;
    const RIDI_DEVICENAME: u32 = 0x20000007;

    const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
        (device_type << 16) | (access << 14) | (function << 2) | method
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn driver_device_kind_code(kind: WindowsDriverDeviceKind) -> u32 {
        match kind {
            WindowsDriverDeviceKind::Keyboard => RSHARE_DEVICE_KEYBOARD,
            WindowsDriverDeviceKind::Mouse => RSHARE_DEVICE_MOUSE,
            WindowsDriverDeviceKind::Gamepad => RSHARE_DEVICE_GAMEPAD,
        }
    }

    fn driver_device_kind_from_code(code: u32) -> Option<WindowsDriverDeviceKind> {
        match code {
            RSHARE_DEVICE_KEYBOARD => Some(WindowsDriverDeviceKind::Keyboard),
            RSHARE_DEVICE_MOUSE => Some(WindowsDriverDeviceKind::Mouse),
            RSHARE_DEVICE_GAMEPAD => Some(WindowsDriverDeviceKind::Gamepad),
            _ => None,
        }
    }

    fn driver_event_kind_from_code(code: u32) -> Option<WindowsDriverEventKind> {
        match code {
            RSHARE_EVENT_KEY => Some(WindowsDriverEventKind::Key),
            RSHARE_EVENT_MOUSE_MOVE => Some(WindowsDriverEventKind::MouseMove),
            RSHARE_EVENT_MOUSE_BUTTON => Some(WindowsDriverEventKind::MouseButton),
            RSHARE_EVENT_MOUSE_WHEEL => Some(WindowsDriverEventKind::MouseWheel),
            RSHARE_EVENT_SYNTHETIC => Some(WindowsDriverEventKind::Synthetic),
            _ => None,
        }
    }

    fn driver_event_source_from_code(code: u16) -> Option<WindowsDriverEventSource> {
        match code {
            RSHARE_SOURCE_HARDWARE => Some(WindowsDriverEventSource::Hardware),
            RSHARE_SOURCE_INJECTED_LOOPBACK => Some(WindowsDriverEventSource::InjectedLoopback),
            RSHARE_SOURCE_DRIVER_TEST => Some(WindowsDriverEventSource::DriverTest),
            RSHARE_SOURCE_VIRTUAL_DEVICE => Some(WindowsDriverEventSource::VirtualDevice),
            _ => None,
        }
    }

    type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;
    type MonitorEnumProc = unsafe extern "system" fn(isize, isize, *mut Rect, isize) -> i32;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct RShareDriverVersionRaw {
        major: u16,
        minor: u16,
        patch: u16,
        abi: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct RShareDriverCapabilitiesRaw {
        abi: u16,
        flags: u32,
        max_event_size: u32,
        reserved: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct RShareDriverEventRaw {
        abi: u16,
        source: u16,
        device_kind: u32,
        event_kind: u32,
        flags: u32,
        device_id: u64,
        device_instance_hash: u64,
        value0: i32,
        value1: i32,
        value2: i32,
        timestamp_us: u64,
    }

    impl TryFrom<RShareDriverEventRaw> for WindowsDriverInputEvent {
        type Error = anyhow::Error;

        fn try_from(raw: RShareDriverEventRaw) -> Result<Self> {
            if raw.abi != RSHARE_DRIVER_ABI {
                anyhow::bail!("Unsupported RShare driver event ABI {}", raw.abi);
            }
            let device_kind = driver_device_kind_from_code(raw.device_kind)
                .ok_or_else(|| anyhow::anyhow!("Unknown driver device kind {}", raw.device_kind))?;
            let event_kind = driver_event_kind_from_code(raw.event_kind)
                .ok_or_else(|| anyhow::anyhow!("Unknown driver event kind {}", raw.event_kind))?;
            let source = driver_event_source_from_code(raw.source)
                .ok_or_else(|| anyhow::anyhow!("Unknown driver event source {}", raw.source))?;

            Ok(Self {
                source,
                device_kind,
                event_kind,
                device_id: format!("rshare-driver:{:016x}", raw.device_id),
                device_instance_id: format!("hash:{:016x}", raw.device_instance_hash),
                value0: raw.value0,
                value1: raw.value1,
                value2: raw.value2,
                flags: raw.flags,
                timestamp_us: raw.timestamp_us,
            })
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct RShareInjectReportRaw {
        report_kind: u32,
        value0: i32,
        value1: i32,
        value2: i32,
        flags: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct RShareTestPacketRaw {
        device_kind: u32,
        event_kind: u32,
        value0: i32,
        value1: i32,
        value2: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RawInputDeviceList {
        h_device: isize,
        dw_type: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MonitorInfo {
        cb_size: u32,
        rc_monitor: Rect,
        rc_work: Rect,
        flags: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Message {
        hwnd: isize,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        pt: Point,
        l_private: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MouseHookStruct {
        pt: Point,
        mouse_data: u32,
        flags: u32,
        time: u32,
        extra_info: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeyboardHookStruct {
        vk_code: u32,
        scan_code: u32,
        flags: u32,
        time: u32,
        extra_info: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MouseInput {
        dx: i32,
        dy: i32,
        mouse_data: u32,
        flags: u32,
        time: u32,
        extra_info: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeyboardInput {
        vk: u16,
        scan: u16,
        flags: u32,
        time: u32,
        extra_info: usize,
    }

    #[repr(C)]
    union InputPayload {
        mouse: MouseInput,
        keyboard: KeyboardInput,
    }

    #[repr(C)]
    struct Input {
        kind: u32,
        payload: InputPayload,
    }

    extern "system" {
        fn SetWindowsHookExW(
            id_hook: i32,
            hook_proc: Option<HookProc>,
            instance: isize,
            thread_id: u32,
        ) -> isize;
        fn UnhookWindowsHookEx(hook: isize) -> i32;
        fn CallNextHookEx(hook: isize, code: i32, w_param: usize, l_param: isize) -> isize;
        fn GetMessageW(message: *mut Message, hwnd: isize, min_filter: u32, max_filter: u32)
            -> i32;
        fn PeekMessageW(
            message: *mut Message,
            hwnd: isize,
            min_filter: u32,
            max_filter: u32,
            remove_msg: u32,
        ) -> i32;
        fn PostThreadMessageW(thread_id: u32, msg: u32, w_param: usize, l_param: isize) -> i32;
        fn GetCurrentThreadId() -> u32;
        fn GetRawInputDeviceList(
            p_raw_input_device_list: *mut RawInputDeviceList,
            pui_num_devices: *mut u32,
            cb_size: u32,
        ) -> u32;
        fn GetRawInputDeviceInfoW(
            h_device: isize,
            ui_command: u32,
            p_data: *mut std::ffi::c_void,
            pcb_size: *mut u32,
        ) -> u32;
        fn EnumDisplayMonitors(
            hdc: isize,
            lprc_clip: *const Rect,
            lpfn_enum: Option<MonitorEnumProc>,
            dw_data: isize,
        ) -> i32;
        fn GetMonitorInfoW(h_monitor: isize, lpmi: *mut MonitorInfo) -> i32;
        fn CreateFileW(
            lp_file_name: *const u16,
            dw_desired_access: u32,
            dw_share_mode: u32,
            lp_security_attributes: *mut std::ffi::c_void,
            dw_creation_disposition: u32,
            dw_flags_and_attributes: u32,
            h_template_file: isize,
        ) -> isize;
        fn CloseHandle(h_object: isize) -> i32;
        fn DeviceIoControl(
            h_device: isize,
            dw_io_control_code: u32,
            lp_in_buffer: *mut std::ffi::c_void,
            n_in_buffer_size: u32,
            lp_out_buffer: *mut std::ffi::c_void,
            n_out_buffer_size: u32,
            lp_bytes_returned: *mut u32,
            lp_overlapped: *mut std::ffi::c_void,
        ) -> i32;
    }

    unsafe fn device_io_control(
        handle: isize,
        ioctl: u32,
        in_buffer: *mut std::ffi::c_void,
        in_buffer_size: u32,
        out_buffer: *mut std::ffi::c_void,
        out_buffer_size: u32,
    ) -> Result<()> {
        let mut bytes_returned = 0u32;
        let ok = DeviceIoControl(
            handle,
            ioctl,
            in_buffer,
            in_buffer_size,
            out_buffer,
            out_buffer_size,
            &mut bytes_returned,
            std::ptr::null_mut(),
        );
        if ok == 0 {
            return Err(DeviceIoControlError {
                ioctl,
                source: std::io::Error::last_os_error(),
            }
            .into());
        }

        Ok(())
    }

    unsafe fn send_input(input: &Input, context: &str) -> Result<()> {
        extern "system" {
            fn SendInput(cInputs: u32, pInputs: *const Input, cbSize: i32) -> u32;
        }

        let sent = SendInput(1, input as *const Input, size_of::<Input>() as i32);
        if sent != 1 {
            return Err(anyhow::anyhow!(
                "SendInput({}) failed: {}",
                context,
                std::io::Error::last_os_error()
            ));
        }

        Ok(())
    }

    /// Send mouse move via SendInput using the native INPUT ABI.
    unsafe fn send_mouse_move_input(dx: i32, dy: i32) -> Result<()> {
        let input = Input {
            kind: INPUT_MOUSE,
            payload: InputPayload {
                mouse: MouseInput {
                    dx,
                    dy,
                    mouse_data: 0,
                    flags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                    time: 0,
                    extra_info: 0,
                },
            },
        };

        send_input(&input, "mouse move")
    }

    /// Send mouse button via SendInput using the native INPUT ABI.
    unsafe fn send_mouse_button_input(flags: u32, data: u32) -> Result<()> {
        let input = Input {
            kind: INPUT_MOUSE,
            payload: InputPayload {
                mouse: MouseInput {
                    dx: 0,
                    dy: 0,
                    mouse_data: data,
                    flags,
                    time: 0,
                    extra_info: 0,
                },
            },
        };

        send_input(&input, "mouse button")
    }

    /// Send wheel via SendInput using the native INPUT ABI.
    unsafe fn send_wheel_input(data: u32, flags: u32) -> Result<()> {
        let input = Input {
            kind: INPUT_MOUSE,
            payload: InputPayload {
                mouse: MouseInput {
                    dx: 0,
                    dy: 0,
                    mouse_data: data,
                    flags,
                    time: 0,
                    extra_info: 0,
                },
            },
        };

        send_input(&input, "mouse wheel")
    }

    /// Send key via SendInput using the native INPUT ABI.
    unsafe fn send_key_input(vk: u16, down: bool) -> Result<()> {
        let input = Input {
            kind: INPUT_KEYBOARD,
            payload: InputPayload {
                keyboard: KeyboardInput {
                    vk,
                    scan: 0,
                    flags: if down { 0 } else { KEYEVENTF_KEYUP },
                    time: 0,
                    extra_info: 0,
                },
            },
        };

        send_input(&input, "keyboard")
    }

    /// Get all screen information (multi-monitor support)
    pub fn get_all_screens() -> Vec<ScreenInfo> {
        unsafe extern "system" fn collect_monitor(
            monitor: isize,
            _hdc: isize,
            _rect: *mut Rect,
            data: isize,
        ) -> i32 {
            let screens = &mut *(data as *mut Vec<(bool, ScreenInfo)>);
            let mut info = MonitorInfo {
                cb_size: size_of::<MonitorInfo>() as u32,
                rc_monitor: Rect::default(),
                rc_work: Rect::default(),
                flags: 0,
            };

            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return 1;
            }

            let rect = info.rc_monitor;
            let width = rect.right.saturating_sub(rect.left).max(0) as u32;
            let height = rect.bottom.saturating_sub(rect.top).max(0) as u32;
            if width > 0 && height > 0 {
                screens.push((
                    info.flags & MONITORINFOF_PRIMARY != 0,
                    ScreenInfo {
                        x: rect.left,
                        y: rect.top,
                        width,
                        height,
                    },
                ));
            }
            1
        }

        let mut screens: Vec<(bool, ScreenInfo)> = Vec::new();
        let ok = unsafe {
            EnumDisplayMonitors(
                0,
                std::ptr::null(),
                Some(collect_monitor),
                &mut screens as *mut _ as isize,
            )
        };
        if ok == 0 || screens.is_empty() {
            return vec![WindowsInputListener::get_screen_info()];
        }

        screens.sort_by_key(|(primary, screen)| (!*primary, screen.x, screen.y));
        screens.into_iter().map(|(_, screen)| screen).collect()
    }

    /// Open Windows display settings dialog
    pub fn open_display_settings() -> Result<()> {
        use std::process::Command;

        // Open the Windows Settings app to the Display settings page
        // This works on Windows 10/11
        Command::new("cmd")
            .args(["/c", "start", "ms-settings:display"])
            .spawn()
            .context("Failed to open display settings")?;

        tracing::info!("Opened Windows display settings");
        Ok(())
    }

    /// Get DPI scaling factor for the primary monitor
    pub fn get_dpi_scaling() -> f64 {
        extern "C" {
            fn GetDC(hwnd: isize) -> isize;
            fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
            fn GetDeviceCaps(hdc: isize, nIndex: i32) -> i32;
        }

        const LOGPIXELSX: i32 = 88;
        const DESKTOPHORZRES: i32 = 118;
        const HORZRES: i32 = 8;

        unsafe {
            let hdc = GetDC(0);
            if hdc == 0 {
                return 1.0; // Default to 100% if we can't get DPI
            }

            let dpi = GetDeviceCaps(hdc, LOGPIXELSX) as f64;
            let physical_width = GetDeviceCaps(hdc, DESKTOPHORZRES) as f64;
            let logical_width = GetDeviceCaps(hdc, HORZRES) as f64;
            ReleaseDC(0, hdc);

            // Calculate scaling factor from both methods
            let dpi_scaling = dpi / 96.0; // 96 DPI is 100%
            let resolution_scaling = if logical_width > 0.0 {
                physical_width / logical_width
            } else {
                1.0
            };

            // Use the more accurate method
            let scaling =
                if resolution_scaling > 1.0 && (resolution_scaling - dpi_scaling).abs() < 0.1 {
                    resolution_scaling
                } else {
                    dpi_scaling
                };

            tracing::trace!("DPI scaling: {:.0}%", scaling * 100.0);
            scaling
        }
    }

    /// Virtual key codes for Windows
    pub mod vk {
        pub const VK_LBUTTON: u16 = 0x01;
        pub const VK_RBUTTON: u16 = 0x02;
        pub const VK_CANCEL: u16 = 0x03;
        pub const VK_MBUTTON: u16 = 0x04;
        pub const VK_XBUTTON1: u16 = 0x05;
        pub const VK_XBUTTON2: u16 = 0x06;
        pub const VK_BACK: u16 = 0x08;
        pub const VK_TAB: u16 = 0x09;
        pub const VK_CLEAR: u16 = 0x0C;
        pub const VK_RETURN: u16 = 0x0D;
        pub const VK_SHIFT: u16 = 0x10;
        pub const VK_CONTROL: u16 = 0x11;
        pub const VK_MENU: u16 = 0x12;
        pub const VK_PAUSE: u16 = 0x13;
        pub const VK_CAPITAL: u16 = 0x14;
        pub const VK_ESCAPE: u16 = 0x1B;
        pub const VK_SPACE: u16 = 0x20;
        pub const VK_PRIOR: u16 = 0x21;
        pub const VK_NEXT: u16 = 0x22;
        pub const VK_END: u16 = 0x23;
        pub const VK_HOME: u16 = 0x24;
        pub const VK_LEFT: u16 = 0x25;
        pub const VK_UP: u16 = 0x26;
        pub const VK_RIGHT: u16 = 0x27;
        pub const VK_DOWN: u16 = 0x28;
        pub const VK_SNAPSHOT: u16 = 0x2C;
        pub const VK_INSERT: u16 = 0x2D;
        pub const VK_DELETE: u16 = 0x2E;
        pub const VK_LWIN: u16 = 0x5B;
        pub const VK_RWIN: u16 = 0x5C;
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::mem::{align_of, offset_of, size_of};

        #[test]
        fn send_input_layout_matches_windows_64bit_abi() {
            assert_eq!(size_of::<MouseInput>(), 32);
            assert_eq!(size_of::<KeyboardInput>(), 24);
            assert_eq!(size_of::<InputPayload>(), 32);
            assert_eq!(size_of::<Input>(), 40);
            assert_eq!(align_of::<InputPayload>(), align_of::<usize>());
            assert_eq!(offset_of!(Input, kind), 0);
            assert_eq!(offset_of!(Input, payload), 8);
        }

        #[test]
        fn rshare_driver_abi_layout_is_stable() {
            assert_eq!(size_of::<RShareDriverVersionRaw>(), 8);
            assert_eq!(size_of::<RShareDriverCapabilitiesRaw>(), 16);
            assert_eq!(size_of::<RShareInjectReportRaw>(), 20);
            assert_eq!(size_of::<RShareTestPacketRaw>(), 20);
            assert_eq!(size_of::<RShareDriverEventRaw>(), 56);
            assert_eq!(IOCTL_RSHARE_QUERY_VERSION, 0x0022_2004);
            assert_eq!(IOCTL_RSHARE_READ_EVENT, 0x0022_600c);
        }

        #[test]
        fn rshare_driver_empty_queue_error_is_classified() {
            let empty_error: anyhow::Error = DeviceIoControlError {
                ioctl: IOCTL_RSHARE_READ_EVENT,
                source: std::io::Error::from_raw_os_error(ERROR_NO_MORE_ITEMS),
            }
            .into();
            assert!(is_driver_event_queue_empty(&empty_error));

            let other_error: anyhow::Error = DeviceIoControlError {
                ioctl: IOCTL_RSHARE_QUERY_VERSION,
                source: std::io::Error::from_raw_os_error(ERROR_NO_MORE_ITEMS),
            }
            .into();
            assert!(!is_driver_event_queue_empty(&other_error));
        }

        #[test]
        fn rshare_driver_raw_event_converts_to_public_event() {
            let raw = RShareDriverEventRaw {
                abi: RSHARE_DRIVER_ABI,
                source: RSHARE_SOURCE_DRIVER_TEST,
                device_kind: RSHARE_DEVICE_KEYBOARD,
                event_kind: RSHARE_EVENT_SYNTHETIC,
                device_id: 0x1234,
                device_instance_hash: 0x5678,
                value0: 0x10,
                value1: 1,
                timestamp_us: 99,
                ..RShareDriverEventRaw::default()
            };

            let event = WindowsDriverInputEvent::try_from(raw).unwrap();

            assert_eq!(event.device_kind, WindowsDriverDeviceKind::Keyboard);
            assert_eq!(event.event_kind, WindowsDriverEventKind::Synthetic);
            assert_eq!(event.source, WindowsDriverEventSource::DriverTest);
            assert_eq!(event.device_id, "rshare-driver:0000000000001234");
            assert_eq!(event.device_instance_id, "hash:0000000000005678");
        }

        #[test]
        fn windows_input_listener_uses_mouse_and_keyboard_hook_handles() {
            let mut listener =
                WindowsInputListener::new_with_runner_for_test(Box::new(FakeHookRunner::default()));

            listener.start().unwrap();

            assert!(listener.is_running());
            assert_eq!(listener.hook_kinds_for_test(), Some((true, true)));

            listener.stop().unwrap();

            assert!(!listener.is_running());
        }

        #[test]
        fn converts_mouse_hook_messages_to_windows_events() {
            let event = MouseHookStruct {
                pt: Point { x: 120, y: 240 },
                mouse_data: 0,
                flags: 0,
                time: 0,
                extra_info: 0,
            };

            assert_eq!(
                convert_mouse_hook_event(WM_MOUSEMOVE, &event),
                Some(WindowsInputEvent::MouseMove { x: 120, y: 240 })
            );
            assert_eq!(
                convert_mouse_hook_event(WM_LBUTTONDOWN, &event),
                Some(WindowsInputEvent::MouseButton {
                    button: 1,
                    down: true
                })
            );
            assert_eq!(
                convert_mouse_hook_event(
                    WM_MOUSEWHEEL,
                    &MouseHookStruct {
                        mouse_data: (WHEEL_DELTA as u32) << 16,
                        ..event
                    }
                ),
                Some(WindowsInputEvent::MouseWheel {
                    delta_x: 0,
                    delta_y: 1
                })
            );
            assert_eq!(
                convert_mouse_hook_event(
                    WM_XBUTTONUP,
                    &MouseHookStruct {
                        mouse_data: (XBUTTON2 as u32) << 16,
                        ..event
                    }
                ),
                Some(WindowsInputEvent::MouseButton {
                    button: 5,
                    down: false
                })
            );
        }

        #[test]
        fn converts_keyboard_hook_messages_to_windows_events() {
            let event = KeyboardHookStruct {
                vk_code: vk::VK_SPACE as u32,
                scan_code: 0,
                flags: 0,
                time: 0,
                extra_info: 0,
            };

            assert_eq!(
                convert_keyboard_hook_event(WM_KEYDOWN, &event),
                Some(WindowsInputEvent::Key {
                    vk: vk::VK_SPACE as u32,
                    down: true
                })
            );
            assert_eq!(
                convert_keyboard_hook_event(WM_SYSKEYUP, &event),
                Some(WindowsInputEvent::Key {
                    vk: vk::VK_SPACE as u32,
                    down: false
                })
            );
        }

        #[test]
        fn ignores_injected_mouse_hook_events() {
            let event = MouseHookStruct {
                pt: Point { x: 120, y: 240 },
                mouse_data: 0,
                flags: LLMHF_INJECTED,
                time: 0,
                extra_info: 0,
            };

            assert_eq!(convert_mouse_hook_event(WM_MOUSEMOVE, &event), None);
        }

        #[test]
        fn ignores_injected_keyboard_hook_events() {
            let event = KeyboardHookStruct {
                vk_code: vk::VK_SPACE as u32,
                scan_code: 0,
                flags: LLKHF_INJECTED,
                time: 0,
                extra_info: 0,
            };

            assert_eq!(convert_keyboard_hook_event(WM_KEYDOWN, &event), None);
        }

        #[derive(Default)]
        struct FakeHookRunner;

        impl HookRunner for FakeHookRunner {
            fn start(&mut self, _callback: WindowsInputCallback) -> Result<HookRuntime> {
                Ok(HookRuntime {
                    mouse_hook: 1,
                    keyboard_hook: 2,
                    thread_id: 3,
                    thread: None,
                })
            }
        }
    }
}

// Stub for non-Windows platforms
#[cfg(not(windows))]
pub struct ScreenInfo {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
