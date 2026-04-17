//! macOS platform-specific implementations

cfg_if::cfg_if! {
    if #[cfg(target_os = "macos")] {
        pub use macos_impl::*;
    }
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use anyhow::Result;

    /// macOS input listener using CGEventTap
    pub struct MacosInputListener {
        // TODO: Implement using CGEventTap
    }

    impl MacosInputListener {
        pub fn new() -> Self {
            Self {}
        }

        pub fn start(&mut self) -> Result<()> {
            tracing::info!("macOS input listener starting (not yet implemented)");
            Ok(())
        }

        pub fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    /// macOS input emulator using CGEvent
    pub struct MacosInputEmulator {
        // TODO: Implement using CGEventCreateMouseEvent
    }

    impl MacosInputEmulator {
        pub fn new() -> Self {
            Self {}
        }

        pub fn send_mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
            tracing::debug!("macOS: mouse move to ({}, {})", x, y);
            Ok(())
        }
    }
}
