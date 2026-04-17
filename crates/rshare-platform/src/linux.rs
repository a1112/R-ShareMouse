//! Linux platform-specific implementations (X11 and Wayland)

cfg_if::cfg_if! {
    if #[cfg(target_os = "linux")] {
        pub use linux_impl::*;
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use anyhow::Result;

    /// Linux input listener
    pub struct LinuxInputListener {
        // TODO: Implement X11 and Wayland support
    }

    impl LinuxInputListener {
        pub fn new() -> Self {
            Self {}
        }

        pub fn start(&mut self) -> Result<()> {
            tracing::info!("Linux input listener starting (not yet implemented)");
            Ok(())
        }

        pub fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    /// Linux input emulator
    pub struct LinuxInputEmulator {
        // TODO: Implement XTest (X11) and Wayland support
    }

    impl LinuxInputEmulator {
        pub fn new() -> Self {
            Self {}
        }

        pub fn send_mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
            tracing::debug!("Linux: mouse move to ({}, {})", x, y);
            Ok(())
        }
    }
}
