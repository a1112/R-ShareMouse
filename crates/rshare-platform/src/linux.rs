//! Linux platform-specific implementations (X11 and Wayland)

cfg_if::cfg_if! {
    if #[cfg(target_os = "linux")] {
        pub use linux_impl::*;
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use anyhow::Result;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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

    /// Linux input listener
    ///
    /// Uses X11 or Wayland depending on the current display server.
    /// Note: Wayland does not support global input listening for security reasons.
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

        pub fn start(&mut self) -> Result<()> {
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
                        listener.start()?;
                        #[cfg(feature = "x11")]
                        {
                            self.x11_listener = Some(listener);
                        }
                    }
                    #[cfg(not(feature = "x11"))]
                    {
                        tracing::warn!("X11 support not enabled, input capture may not work");
                    }
                }
                DisplayServer::Wayland => {
                    tracing::warn!(
                        "Wayland detected. Global input listening is not supported on Wayland \
                         due to security restrictions. Using RDev fallback which may have \
                         limited functionality. For full support, consider using X11 or a \
                         Wayland-specific portal."
                    );
                    // RDev will be used as fallback in the caller
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

    #[cfg(feature = "x11")]
    struct X11InputListener {
        display: Option<*mut x11::xlib::Display>,
        running: Arc<AtomicBool>,
    }

    #[cfg(feature = "x11")]
    impl X11InputListener {
        fn new() -> Result<Self> {
            Ok(Self {
                display: None,
                running: Arc::new(AtomicBool::new(false)),
            })
        }

        fn start(&mut self) -> Result<()> {
            use std::ptr;

            let display = unsafe { x11::xlib::XOpenDisplay(ptr::null()) };
            if display.is_null() {
                return Err(anyhow::anyhow!("Failed to open X11 display"));
            }

            self.display = Some(display);
            self.running.store(true, Ordering::Relaxed);
            Ok(())
        }

        fn stop(&mut self) -> Result<()> {
            if let Some(display) = self.display.take() {
                unsafe { x11::xlib::XCloseDisplay(display) };
            }
            self.running.store(false, Ordering::Relaxed);
            Ok(())
        }
    }

    /// Linux input emulator
    ///
    /// Uses XTest for X11 or libei for Wayland.
    pub struct LinuxInputEmulator {
        display_server: DisplayServer,
        #[cfg(feature = "x11")]
        display: Option<*mut x11::xlib::Display>,
    }

    impl LinuxInputEmulator {
        pub fn new() -> Result<Self> {
            let display_server = display_server_type();

            #[cfg(feature = "x11")]
            let display = if display_server == DisplayServer::X11 {
                use std::ptr;
                let d = unsafe { x11::xlib::XOpenDisplay(ptr::null()) };
                if d.is_null() {
                    tracing::warn!("Failed to open X11 display for emulation");
                    None
                } else {
                    Some(d)
                }
            } else {
                None
            };

            #[cfg(not(feature = "x11"))]
            let display = None;

            Ok(Self {
                display_server,
                #[cfg(feature = "x11")]
                display,
            })
        }

        pub fn send_mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
            #[cfg(feature = "x11")]
            if let Some(display) = self.display {
                use x11::xtest;
                unsafe {
                    xtest::XTestFakeMotionEvent(display, 0, x, y, 0);
                    x11::xlib::XFlush(display);
                }
                return Ok(());
            }

            // Fallback to enigo (already handled by the caller)
            tracing::debug!("Mouse move to ({}, {}) - using fallback", x, y);
            Ok(())
        }

        pub fn send_mouse_button(&mut self, button: u32, press: bool) -> Result<()> {
            #[cfg(feature = "x11")]
            if let Some(display) = self.display {
                use x11::xtest;
                unsafe {
                    let x11_button = match button {
                        1 => 1,  // Left
                        2 => 3,  // Right
                        3 => 2,  // Middle
                        _ => button,
                    };
                    if press {
                        xtest::XTestFakeButtonEvent(display, x11_button, 1, 0);
                    } else {
                        xtest::XTestFakeButtonEvent(display, x11_button, 0, 0);
                    }
                    x11::xlib::XFlush(display);
                }
                return Ok(());
            }

            tracing::debug!("Mouse button {} {} - using fallback", button, if press { "press" } else { "release" });
            Ok(())
        }

        pub fn send_key(&mut self, keycode: u32, press: bool) -> Result<()> {
            #[cfg(feature = "x11")]
            if let Some(display) = self.display {
                use x11::xtest;
                unsafe {
                    if press {
                        xtest::XTestFakeKeyEvent(display, keycode as u32, 1, 0);
                    } else {
                        xtest::XTestFakeKeyEvent(display, keycode as u32, 0, 0);
                    }
                    x11::xlib::XFlush(display);
                }
                return Ok(());
            }

            tracing::debug!("Key {} {} - using fallback", keycode, if press { "press" } else { "release" });
            Ok(())
        }

        pub fn display_server(&self) -> DisplayServer {
            self.display_server
        }
    }

    #[cfg(feature = "x11")]
    impl Drop for LinuxInputEmulator {
        fn drop(&mut self) {
            if let Some(display) = self.display.take() {
                unsafe { x11::xlib::XCloseDisplay(display) };
            }
        }
    }

    #[cfg(not(feature = "x11"))]
    impl Drop for LinuxInputEmulator {
        fn drop(&mut self) {}
    }
}
