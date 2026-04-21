//! R-ShareMouse platform-specific implementations
//!
//! This crate provides platform-specific implementations for input handling
//! on Windows, macOS, and Linux.

use tokio::sync::mpsc;
use rshare_core::clipboard::ClipboardContent;

// Re-export anyhow context for display module convenience
pub use anyhow::Context;

// Platform modules
#[cfg(windows)]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

// Cross-platform modules
pub mod file_drop;

// Clipboard listener module
pub mod clipboard;

// Firewall configuration module
pub mod firewall;

// Re-exports
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
pub use linux::*;

pub use file_drop::*;
pub use clipboard::*;
pub use firewall::*;

/// Clipboard listener configuration
#[derive(Debug, Clone)]
pub struct ClipboardListenerConfig {
    /// Poll interval in milliseconds (for polling-based implementations)
    pub poll_interval_ms: u64,

    /// Maximum content size to transfer (bytes)
    pub max_size: usize,
}

impl Default for ClipboardListenerConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 250,
            max_size: 10 * 1024 * 1024, // 10MB
        }
    }
}

/// Trait for platform-specific clipboard listeners
#[async_trait::async_trait]
pub trait ClipboardListener: Send + Sync {
    /// Start listening for clipboard changes
    async fn start(&mut self) -> anyhow::Result<()>;

    /// Stop listening
    async fn stop(&mut self) -> anyhow::Result<()>;

    /// Check if listener is running
    fn is_running(&self) -> bool;

    /// Get the event receiver
    fn receiver(&mut self) -> mpsc::UnboundedReceiver<ClipboardContent>;

    /// Get current clipboard content
    async fn get_current_clipboard(&self) -> anyhow::Result<ClipboardContent>;
}

/// Platform-specific clipboard listener type alias
#[cfg(windows)]
pub type PlatformClipboardListener = clipboard::WindowsClipboardListener;

#[cfg(target_os = "macos")]
pub type PlatformClipboardListener = clipboard::MacosClipboardListener;

#[cfg(target_os = "linux")]
pub type PlatformClipboardListener = clipboard::LinuxClipboardListener;

/// Platform-specific display settings functions
#[cfg(windows)]
pub mod display {
    pub use super::windows::open_display_settings;
    pub use super::windows::get_dpi_scaling;
}

#[cfg(target_os = "macos")]
pub mod display {
    /// Open macOS display settings (System Preferences > Displays)
    pub fn open_display_settings() -> anyhow::Result<()> {
        use std::process::Command;
        Command::new("open")
            .args(["x-apple.systempreferences:com.apple.preference.displays"])
            .spawn()
            .context("Failed to open display settings")?;
        Ok(())
    }

    /// Get DPI scaling factor (always 1.0 on macOS as it handles scaling differently)
    pub fn get_dpi_scaling() -> f64 {
        1.0
    }
}

#[cfg(target_os = "linux")]
pub mod display {
    /// Open Linux display settings (varies by desktop environment)
    pub fn open_display_settings() -> anyhow::Result<()> {
        use std::process::Command;

        // Try common desktop environments' display settings commands
        let commands = [
            ["gnome-control-center", "display"],
            ["systemsettings", "5"], // KDE Plasma Display settings
            ["xfce4-display-settings"],
            ["lxrandr"],
        ];

        for cmd in &commands {
            if Command::new(cmd[0]).args(&cmd[1..]).spawn().is_ok() {
                return Ok(());
            }
        }

        anyhow::bail!("No supported display settings command found")
    }

    /// Get DPI scaling factor
    pub fn get_dpi_scaling() -> f64 {
        1.0
    }
}
