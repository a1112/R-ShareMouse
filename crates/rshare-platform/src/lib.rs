//! R-ShareMouse platform-specific implementations
//!
//! This crate provides platform-specific implementations for input handling
//! on Windows, macOS, and Linux.

use tokio::sync::mpsc;
use rshare_core::clipboard::ClipboardContent;

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

// Re-exports
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
pub use linux::*;

pub use file_drop::*;
pub use clipboard::*;

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
