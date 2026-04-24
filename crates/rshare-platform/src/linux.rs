//! Linux platform-specific implementations (X11 and Wayland)

cfg_if::cfg_if! {
    if #[cfg(target_os = "linux")] {
        pub use linux_impl::*;
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use anyhow::Result;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    #[cfg(feature = "x11")]
    use x11::xlib;

    // Wrapper to make X11 display pointer Send
    #[cfg(feature = "x11")]
    struct SendDisplay(*mut x11::xlib::Display);

    #[cfg(feature = "x11")]
    unsafe impl Send for SendDisplay {}

    /// Detect if running under Wayland
    pub fn is_wayland() -> bool {
        std::env::var("WAYLAND_DISPLAY").is_ok()
    }

    /// Detect if running under X11
    pub fn is_x11() -> bool {
        std::env::var("DISPLAY").is_ok()
    }

    /// Get the current display server type
    pub fn display_server_type() -> DisplayServer {
        if is_wayland() {
            DisplayServer::Wayland
        } else if is_x11() {
            DisplayServer::X11
        } else {
            DisplayServer::Unknown
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum DisplayServer {
        X11,
        Wayland,
        Unknown,
    }

    impl DisplayServer {
        pub fn name(&self) -> &'static str {
            match self {
                DisplayServer::X11 => "X11",
                DisplayServer::Wayland => "Wayland",
                DisplayServer::Unknown => "Unknown",
            }
        }

        pub fn supports_global_input(&self) -> bool {
            match self {
                DisplayServer::X11 => true,
                DisplayServer::Wayland => false,
                DisplayServer::Unknown => false,
            }
        }
    }

    // ============================================================================
    // X11 Native Input Capture using XInput2
    // ============================================================================

    /// X11 native input listener using XInput2 extension
    pub struct X11InputListener {
        display: Option<*mut x11::xlib::Display>,
        running: Arc<AtomicBool>,
        event_count: Arc<AtomicUsize>,
        thread_handle: Option<JoinHandle<()>>,
    }

    // Generic input event
    #[derive(Debug, Clone)]
    pub enum InputEvent {
        MouseMove { x: i32, y: i32 },
        MouseButton { button: u32, pressed: bool },
        MouseWheel { delta_x: i32, delta_y: i32 },
        Key { keycode: u32, pressed: bool },
    }

    unsafe impl Send for X11InputListener {}

    impl X11InputListener {
        pub fn new() -> Result<Self> {
            Ok(Self {
                display: None,
                running: Arc::new(AtomicBool::new(false)),
                event_count: Arc::new(AtomicUsize::new(0)),
                thread_handle: None,
            })
        }

        pub fn start_with_callback<F>(&mut self, callback: F) -> Result<()>
        where
            F: Fn(InputEvent) + Send + Sync + 'static,
        {
            if self.running.load(Ordering::Relaxed) {
                return Ok(());
            }

            #[cfg(feature = "x11")]
            {
                use std::ptr;

                // Open X11 display
                let display = unsafe { x11::xlib::XOpenDisplay(ptr::null()) };
                if display.is_null() {
                    anyhow::bail!("Failed to open X11 display");
                }

                self.display = Some(display);
                self.running.store(true, Ordering::Relaxed);

                // Store the callback in an Arc for thread sharing
                let callback = Arc::new(callback);
                let running = self.running.clone();
                let event_count = self.event_count.clone();

                // Use unsafe to assert that the display pointer is safe to send
                // X11 display connections are thread-local, but we're only using
                // this display in the spawned thread
                let display_ptr = display as usize;
                let handle = thread::Builder::new()
                    .name("rshare-x11-input-listener".to_string())
                    .spawn(move || {
                        tracing::info!("X11 input listener thread started");
                        let display = display_ptr as *mut x11::xlib::Display;
                        if let Err(e) = Self::event_loop(display, running, event_count, callback) {
                            tracing::error!("X11 input listener error: {:?}", e);
                        }
                    })?;

                self.thread_handle = Some(handle);

                tracing::info!("X11 input listener started (using XInput2)");
                Ok(())
            }

            #[cfg(not(feature = "x11"))]
            {
                anyhow::bail!("X11 support not enabled. Enable with 'x11' feature.");
            }
        }

        #[cfg(feature = "x11")]
        fn event_loop(
            display: *mut x11::xlib::Display,
            running: Arc<AtomicBool>,
            event_count: Arc<AtomicUsize>,
            callback: Arc<dyn Fn(InputEvent) + Send + Sync>,
        ) -> Result<()> {
            use std::ptr;
            use x11::xlib::{self, KeyPress, KeyRelease};

            unsafe {
                let screen = xlib::XDefaultScreen(display);
                let root_window = xlib::XRootWindow(display, screen);

                // Select input events
                let event_mask = xlib::ButtonPressMask
                    | xlib::ButtonReleaseMask
                    | xlib::PointerMotionMask
                    | xlib::KeyPressMask
                    | xlib::KeyReleaseMask;

                xlib::XSelectInput(display, root_window, event_mask);

                // Query XInput2 extension
                let mut major_opcode: i32 = 0;
                let mut minor_opcode: i32 = 0;
                let mut event_base: i32 = 0;
                let mut error_base: i32 = 0;

                let xi2_supported = xlib::XQueryExtension(
                    display,
                    b"XInputExtension\0".as_ptr() as *const i8,
                    &mut major_opcode,
                    &mut event_base,
                    &mut error_base,
                ) != 0;

                if xi2_supported {
                    tracing::debug!(
                        "XInput2 extension available (opcode: {}, event base: {})",
                        major_opcode,
                        event_base
                    );
                } else {
                    tracing::warn!("XInput2 extension not available, using legacy input handling");
                }

                while running.load(Ordering::Relaxed) {
                    let mut event: xlib::XEvent = std::mem::zeroed();

                    // Check for events with timeout
                    if xlib::XPending(display) > 0 {
                        xlib::XNextEvent(display, &mut event);

                        let event_type = event.type_;

                        match event_type {
                            xlib::ButtonPress => {
                                let button_event = event.button;
                                let _ = event_count.fetch_add(1, Ordering::Relaxed);
                                callback(InputEvent::MouseButton {
                                    button: button_event.button as u32,
                                    pressed: true,
                                });
                            }
                            xlib::ButtonRelease => {
                                let button_event = event.button;
                                let _ = event_count.fetch_add(1, Ordering::Relaxed);
                                callback(InputEvent::MouseButton {
                                    button: button_event.button as u32,
                                    pressed: false,
                                });
                            }
                            xlib::MotionNotify => {
                                let motion_event = event.motion;
                                let _ = event_count.fetch_add(1, Ordering::Relaxed);
                                callback(InputEvent::MouseMove {
                                    x: motion_event.x as i32,
                                    y: motion_event.y as i32,
                                });
                            }
                            xlib::KeyPress => {
                                let key_event = event.key;
                                let _ = event_count.fetch_add(1, Ordering::Relaxed);
                                callback(InputEvent::Key {
                                    keycode: key_event.keycode as u32,
                                    pressed: true,
                                });
                            }
                            xlib::KeyRelease => {
                                let key_event = event.key;
                                let _ = event_count.fetch_add(1, Ordering::Relaxed);
                                callback(InputEvent::Key {
                                    keycode: key_event.keycode as u32,
                                    pressed: false,
                                });
                            }
                            _ => {}
                        }
                    } else {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }

            Ok(())
        }

        pub fn stop(&mut self) -> Result<()> {
            self.running.store(false, Ordering::Relaxed);

            if let Some(handle) = self.thread_handle.take() {
                let _ = handle.join();
            }

            #[cfg(feature = "x11")]
            if let Some(display) = self.display.take() {
                unsafe { x11::xlib::XCloseDisplay(display) };
            }

            tracing::info!("X11 input listener stopped");
            Ok(())
        }

        pub fn is_running(&self) -> bool {
            self.running.load(Ordering::Relaxed)
        }

        pub fn event_count(&self) -> usize {
            self.event_count.load(Ordering::Relaxed)
        }
    }

    impl Drop for X11InputListener {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    // ============================================================================
    // X11 Native Input Injection using XTest
    // ============================================================================

    /// X11 native input emulator using XTest extension
    pub struct X11InputEmulator {
        display: Option<*mut x11::xlib::Display>,
        screen: i32,
        active: bool,
    }

    unsafe impl Send for X11InputEmulator {}

    impl X11InputEmulator {
        pub fn new() -> Result<Self> {
            #[cfg(feature = "x11")]
            {
                use std::ptr;

                let display = unsafe { x11::xlib::XOpenDisplay(ptr::null()) };
                if display.is_null() {
                    anyhow::bail!("Failed to open X11 display");
                }

                let screen = unsafe { x11::xlib::XDefaultScreen(display) };

                Ok(Self {
                    display: Some(display),
                    screen,
                    active: false,
                })
            }

            #[cfg(not(feature = "x11"))]
            {
                anyhow::bail!("X11 support not enabled. Enable with 'x11' feature.");
            }
        }

        pub fn activate(&mut self) -> Result<()> {
            self.active = true;
            tracing::info!("X11 input emulator activated");
            Ok(())
        }

        pub fn deactivate(&mut self) -> Result<()> {
            self.active = false;
            tracing::info!("X11 input emulator deactivated");
            Ok(())
        }

        pub fn is_active(&self) -> bool {
            self.active
        }

        pub fn send_mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            #[cfg(feature = "x11")]
            unsafe {
                if let Some(display) = self.display {
                    use x11::xtest;

                    xtest::XTestFakeMotionEvent(display, self.screen, x, y, 0);
                    x11::xlib::XFlush(display);
                }
            }

            Ok(())
        }

        pub fn send_mouse_button(&mut self, button: u32, press: bool) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            #[cfg(feature = "x11")]
            unsafe {
                if let Some(display) = self.display {
                    use x11::xtest;

                    // Map button indices: 1=Left, 2=Middle, 3=Right, 4=ScrollUp, 5=ScrollDown, etc.
                    let x11_button = match button {
                        1 => 1, // Left
                        2 => 3, // Right
                        3 => 2, // Middle
                        _ => button as u32,
                    };

                    if press {
                        xtest::XTestFakeButtonEvent(display, x11_button, 1, 0);
                    } else {
                        xtest::XTestFakeButtonEvent(display, x11_button, 0, 0);
                    }
                    x11::xlib::XFlush(display);
                }
            }

            Ok(())
        }

        pub fn send_key(&mut self, keycode: u32, press: bool) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            #[cfg(feature = "x11")]
            unsafe {
                if let Some(display) = self.display {
                    use x11::xtest;

                    // Convert our keycode to X11 keycode
                    let x11_keycode = self.keycode_to_x11(keycode);

                    if press {
                        xtest::XTestFakeKeyEvent(display, x11_keycode, 1, 0);
                    } else {
                        xtest::XTestFakeKeyEvent(display, x11_keycode, 0, 0);
                    }
                    x11::xlib::XFlush(display);
                }
            }

            Ok(())
        }

        #[cfg(feature = "x11")]
        fn keycode_to_x11(&self, keycode: u32) -> u32 {
            // Common X11 keycodes
            match keycode {
                0x01 => 9,   // Escape
                0x02 => 10,  // 1
                0x03 => 11,  // 2
                0x04 => 12,  // 3
                0x05 => 13,  // 4
                0x06 => 14,  // 5
                0x07 => 15,  // 6
                0x08 => 16,  // 7
                0x09 => 17,  // 8
                0x0A => 18,  // 9
                0x0B => 19,  // 0
                0x0C => 20,  // -
                0x0D => 21,  // =
                0x0E => 22,  // Backspace
                0x0F => 23,  // Tab
                0x10 => 24,  // Q
                0x11 => 25,  // W
                0x12 => 26,  // E
                0x13 => 27,  // R
                0x14 => 28,  // T
                0x15 => 29,  // Y
                0x16 => 30,  // U
                0x17 => 31,  // I
                0x18 => 32,  // O
                0x19 => 33,  // P
                0x1A => 34,  // [
                0x1B => 35,  // ]
                0x1C => 36,  // Enter
                0x1D => 37,  // Left Ctrl
                0x1E => 38,  // A
                0x1F => 39,  // S
                0x20 => 40,  // D
                0x21 => 41,  // F
                0x22 => 42,  // G
                0x23 => 43,  // H
                0x24 => 44,  // J
                0x25 => 45,  // K
                0x26 => 46,  // L
                0x27 => 47,  // ;
                0x28 => 48,  // '
                0x29 => 49,  // `
                0x2A => 50,  // Left Shift
                0x2B => 51,  // \
                0x2C => 52,  // Z
                0x2D => 53,  // X
                0x2E => 54,  // C
                0x2F => 55,  // V
                0x30 => 56,  // B
                0x31 => 57,  // N
                0x32 => 58,  // M
                0x33 => 59,  // ,
                0x34 => 60,  // .
                0x35 => 61,  // /
                0x36 => 62,  // Right Shift
                0x37 => 63,  // KP *
                0x38 => 64,  // Left Alt
                0x39 => 65,  // Space
                0x3A => 66,  // Caps Lock
                0x3B => 67,  // F1
                0x3C => 68,  // F2
                0x3D => 69,  // F3
                0x3E => 70,  // F4
                0x3F => 71,  // F5
                0x40 => 72,  // F6
                0x41 => 73,  // F7
                0x42 => 74,  // F8
                0x43 => 75,  // F9
                0x44 => 76,  // F10
                0x45 => 77,  // Num Lock
                0x46 => 78,  // Scroll Lock
                0x47 => 79,  // KP 7
                0x48 => 80,  // KP 8
                0x49 => 81,  // KP 9
                0x4A => 82,  // KP -
                0x4B => 83,  // KP 4
                0x4C => 84,  // KP 5
                0x4D => 85,  // KP 6
                0x4E => 86,  // KP +
                0x4F => 87,  // KP 1
                0x50 => 88,  // KP 2
                0x51 => 89,  // KP 3
                0x52 => 90,  // KP 0
                0x53 => 91,  // KP .
                0x57 => 95,  // F11
                0x58 => 96,  // F12
                0x64 => 97,  // Home
                0x65 => 98,  // Up
                0x66 => 99,  // Page Up
                0x67 => 100, // Left
                0x68 => 102, // Right
                0x69 => 103, // End
                0x6A => 104, // Down
                0x6B => 105, // Page Down
                0x6C => 106, // Insert
                0x6D => 107, // Delete
                _ => keycode,
            }
        }

        pub fn send_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
            if !self.active {
                return Ok(());
            }

            #[cfg(feature = "x11")]
            unsafe {
                if let Some(display) = self.display {
                    use x11::xtest;

                    // X11 uses button 4 for scroll up, 5 for scroll down
                    // 6 for scroll left, 7 for scroll right
                    if delta_y > 0 {
                        for _ in 0..delta_y.abs().min(5) {
                            xtest::XTestFakeButtonEvent(display, 4, 1, 0);
                            xtest::XTestFakeButtonEvent(display, 4, 0, 0);
                        }
                    } else if delta_y < 0 {
                        for _ in 0..delta_y.abs().min(5) {
                            xtest::XTestFakeButtonEvent(display, 5, 1, 0);
                            xtest::XTestFakeButtonEvent(display, 5, 0, 0);
                        }
                    }

                    if delta_x > 0 {
                        for _ in 0..delta_x.abs().min(5) {
                            xtest::XTestFakeButtonEvent(display, 7, 1, 0);
                            xtest::XTestFakeButtonEvent(display, 7, 0, 0);
                        }
                    } else if delta_x < 0 {
                        for _ in 0..delta_x.abs().min(5) {
                            xtest::XTestFakeButtonEvent(display, 6, 1, 0);
                            xtest::XTestFakeButtonEvent(display, 6, 0, 0);
                        }
                    }

                    x11::xlib::XFlush(display);
                }
            }

            Ok(())
        }
    }

    impl Drop for X11InputEmulator {
        fn drop(&mut self) {
            #[cfg(feature = "x11")]
            if let Some(display) = self.display.take() {
                unsafe { x11::xlib::XCloseDisplay(display) };
            }
        }
    }

    // ============================================================================
    // Linux Input Listener (Unified)
    // ============================================================================

    /// Linux input listener using platform-specific implementation
    pub struct LinuxInputListener {
        display_server: DisplayServer,
        running: Arc<AtomicBool>,
        #[cfg(feature = "x11")]
        x11_listener: Option<X11InputListener>,
    }

    impl LinuxInputListener {
        pub fn new() -> Self {
            let display_server = display_server_type();
            tracing::info!("Detected display server: {}", display_server.name());

            Self {
                display_server,
                running: Arc::new(AtomicBool::new(false)),
                #[cfg(feature = "x11")]
                x11_listener: None,
            }
        }

        pub fn start_with_callback<F>(&mut self, callback: F) -> Result<()>
        where
            F: Fn(InputEvent) + Send + Sync + 'static,
        {
            if self.running.load(Ordering::Relaxed) {
                return Ok(());
            }

            self.running.store(true, Ordering::Relaxed);

            match self.display_server {
                DisplayServer::X11 => {
                    #[cfg(feature = "x11")]
                    {
                        tracing::info!("Linux input listener starting (X11 mode)");
                        let mut listener = X11InputListener::new()?;
                        listener.start_with_callback(callback)?;
                        self.x11_listener = Some(listener);
                    }
                    #[cfg(not(feature = "x11"))]
                    {
                        tracing::warn!("X11 support not enabled, input capture may not work");
                    }
                }
                DisplayServer::Wayland => {
                    tracing::warn!(
                        "Wayland detected. Global input listening is not supported on Wayland \
                         due to security restrictions. For full support, consider using X11."
                    );
                }
                DisplayServer::Unknown => {
                    tracing::warn!("Unknown display server, input capture may not work");
                }
            }

            Ok(())
        }

        pub fn stop(&mut self) -> Result<()> {
            self.running.store(false, Ordering::Relaxed);

            #[cfg(feature = "x11")]
            if let Some(listener) = self.x11_listener.as_mut() {
                listener.stop()?;
            }
            #[cfg(feature = "x11")]
            {
                self.x11_listener = None;
            }

            Ok(())
        }

        pub fn is_running(&self) -> bool {
            self.running.load(Ordering::Relaxed)
        }

        pub fn display_server(&self) -> DisplayServer {
            self.display_server
        }
    }

    // ============================================================================
    // Linux Input Emulator (Unified)
    // ============================================================================

    /// Linux input emulator using platform-specific implementation
    pub struct LinuxInputEmulator {
        display_server: DisplayServer,
        active: bool,
        #[cfg(feature = "x11")]
        x11_emulator: Option<X11InputEmulator>,
    }

    impl LinuxInputEmulator {
        pub fn new() -> Result<Self> {
            let display_server = display_server_type();

            #[cfg(feature = "x11")]
            let x11_emulator = if display_server == DisplayServer::X11 {
                match X11InputEmulator::new() {
                    Ok(emulator) => Some(emulator),
                    Err(e) => {
                        tracing::warn!("Failed to create X11 emulator: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            Ok(Self {
                display_server,
                active: false,
                #[cfg(feature = "x11")]
                x11_emulator,
            })
        }

        pub fn activate(&mut self) -> Result<()> {
            #[cfg(feature = "x11")]
            if let Some(emulator) = self.x11_emulator.as_mut() {
                emulator.activate()?;
            }
            self.active = true;
            Ok(())
        }

        pub fn deactivate(&mut self) -> Result<()> {
            #[cfg(feature = "x11")]
            if let Some(emulator) = self.x11_emulator.as_mut() {
                emulator.deactivate()?;
            }
            self.active = false;
            Ok(())
        }

        pub fn is_active(&self) -> bool {
            self.active
        }

        pub fn send_mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
            #[cfg(feature = "x11")]
            if let Some(emulator) = self.x11_emulator.as_mut() {
                emulator.send_mouse_move(x, y)?;
                return Ok(());
            }

            tracing::debug!("Mouse move to ({}, {}) - no backend available", x, y);
            Ok(())
        }

        pub fn send_mouse_button(&mut self, button: u32, press: bool) -> Result<()> {
            #[cfg(feature = "x11")]
            if let Some(emulator) = self.x11_emulator.as_mut() {
                emulator.send_mouse_button(button, press)?;
                return Ok(());
            }

            tracing::debug!(
                "Mouse button {} {} - no backend available",
                button,
                if press { "press" } else { "release" }
            );
            Ok(())
        }

        pub fn send_key(&mut self, keycode: u32, press: bool) -> Result<()> {
            #[cfg(feature = "x11")]
            if let Some(emulator) = self.x11_emulator.as_mut() {
                emulator.send_key(keycode, press)?;
                return Ok(());
            }

            tracing::debug!(
                "Key {} {} - no backend available",
                keycode,
                if press { "press" } else { "release" }
            );
            Ok(())
        }

        pub fn send_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
            #[cfg(feature = "x11")]
            if let Some(emulator) = self.x11_emulator.as_mut() {
                emulator.send_wheel(delta_x, delta_y)?;
                return Ok(());
            }

            tracing::debug!("Wheel ({}, {}) - no backend available", delta_x, delta_y);
            Ok(())
        }

        pub fn display_server(&self) -> DisplayServer {
            self.display_server
        }
    }
}
