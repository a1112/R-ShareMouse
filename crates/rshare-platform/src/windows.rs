//! Windows platform-specific implementations

cfg_if::cfg_if! {
    if #[cfg(windows)] {
        pub use windows_impl::*;
    }
}

#[cfg(windows)]
mod windows_impl {
    use anyhow::{Context, Result};
    use std::cell::RefCell;
    use std::mem::size_of;
    use std::sync::{mpsc, Arc};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    type WindowsInputCallback = Arc<dyn Fn(WindowsInputEvent) + Send + Sync + 'static>;

    thread_local! {
        static WINDOWS_HOOK_CALLBACK: RefCell<Option<WindowsInputCallback>> = RefCell::new(None);
    }

    /// Input event captured by the native Windows low-level hooks.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WindowsInputEvent {
        MouseMove { x: i32, y: i32 },
        MouseButton { button: u8, down: bool },
        MouseWheel { delta_x: i32, delta_y: i32 },
        Key { vk: u32, down: bool },
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
                        tracing::debug!("Screen resolution: physical={}x{}, logical={}x{}",
                            physical_width, physical_height, logical_width, logical_height);
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

    type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Point {
        x: i32,
        y: i32,
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
        vec![WindowsInputListener::get_screen_info()]
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
            let scaling = if resolution_scaling > 1.0 && (resolution_scaling - dpi_scaling).abs() < 0.1 {
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
