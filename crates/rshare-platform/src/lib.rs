//! R-ShareMouse platform-specific implementations
//!
//! This crate provides platform-specific implementations for input handling
//! on Windows, macOS, and Linux.

use rshare_core::clipboard::ClipboardContent;
use tokio::sync::mpsc;

// Re-export anyhow context for display module convenience
pub use anyhow::Context;

// Platform modules
#[cfg(windows)]
pub mod windows;

#[cfg(any(target_os = "macos", test))]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(all(target_os = "linux", feature = "x11"))]
pub mod linux_uinput;

#[cfg(all(target_os = "linux", feature = "x11"))]
pub mod linux_evdev;

// Cross-platform modules
pub mod file_drop;

// Clipboard listener module
pub mod clipboard;

// Firewall configuration module
pub mod firewall;

// Cross-platform system integration helpers
pub mod system;

// Cross-platform session lock and system suspend safety events
pub mod system_events;

// Cross-platform display settings helpers
pub mod display;
pub mod display_capture;

// Cross-platform virtual display control helpers
pub mod virtual_display;

// Experimental USB forwarding host runtime
pub mod usb_forwarding;

// Re-exports
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
pub use linux_evdev::{
    enumerate_input_devices, EvdevDriverEvent, EvdevInputListener, UInputInjector,
};

pub use clipboard::*;
pub use file_drop::*;
pub use firewall::*;
pub use system::*;
pub use system_events::*;
pub use usb_forwarding::*;

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

#[cfg(test)]
mod platform_source_contract_tests {
    use crate::macos::{macos_text_operation_plan, MacosTextOperation};

    #[test]
    fn macos_text_plan_chunks_by_unicode_scalar_without_splitting_emoji() {
        let text = format!("{}🙂Z", "a".repeat(19));

        let plan = macos_text_operation_plan(&text);

        assert_eq!(
            plan,
            vec![
                MacosTextOperation::Unicode(&text[..text.len() - 1]),
                MacosTextOperation::Unicode("Z"),
            ]
        );
        assert_eq!(
            plan.iter()
                .map(MacosTextOperation::unicode_scalar_count)
                .sum::<usize>(),
            21
        );
    }

    #[test]
    fn macos_text_plan_limits_every_unicode_chunk_to_twenty_scalars() {
        let text = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNO";

        let plan = macos_text_operation_plan(text);

        assert_eq!(
            plan,
            vec![
                MacosTextOperation::Unicode("abcdefghijklmnopqrst"),
                MacosTextOperation::Unicode("uvwxyzABCDEFGHIJKLMN"),
                MacosTextOperation::Unicode("O"),
            ]
        );
        assert!(plan
            .iter()
            .all(|operation| operation.unicode_scalar_count() <= 20));
    }

    #[test]
    fn macos_text_plan_handles_leading_controls_at_chunk_boundaries() {
        let text = format!("{}\t\r\nnext", "x".repeat(20));

        let plan = macos_text_operation_plan(&text);

        assert_eq!(
            plan,
            vec![
                MacosTextOperation::Unicode(&text[..20]),
                MacosTextOperation::TabClick,
                MacosTextOperation::GuardedControl('\r'),
                MacosTextOperation::GuardedControl('\n'),
                MacosTextOperation::Unicode("next"),
            ]
        );
    }

    #[test]
    fn macos_text_plan_keeps_multiline_content_and_peels_only_leading_controls() {
        let plan = macos_text_operation_plan("one\ntwo\tend");

        assert_eq!(plan, vec![MacosTextOperation::Unicode("one\ntwo\tend")]);
    }

    #[test]
    fn macos_text_commit_uses_core_graphics_unicode_events() {
        let source = include_str!("macos.rs");
        let start = source
            .find("pub fn send_text(&mut self, text: &str)")
            .expect("missing macOS text commit implementation");
        let end = source[start..]
            .find("impl Default for MacosInputEmulator")
            .map(|offset| start + offset)
            .expect("missing end of macOS input emulator implementation");
        let send_text = &source[start..end];

        assert!(send_text.contains("if !self.active"));
        assert!(send_text.contains("if text.is_empty()"));
        assert!(send_text.contains("CGEvent::new_keyboard_event"));
        assert!(send_text.contains("event.set_string(text);"));
        assert!(send_text.contains("post_rshare_injected_event(&event)?;"));
    }

    #[test]
    fn macos_relative_mouse_uses_current_point_plus_delta() {
        let source = include_str!("macos.rs");
        let start = source
            .find("pub fn send_mouse_move_relative")
            .expect("missing macOS relative mouse implementation");
        let end = source[start..]
            .find("pub fn send_button")
            .map(|offset| start + offset)
            .expect("missing end of macOS relative mouse implementation");
        let relative = &source[start..end];

        assert!(relative.contains("current_mouse_position()?"));
        assert!(relative.contains("position.x + dx as f64"));
        assert!(relative.contains("position.y + dy as f64"));
        assert!(relative.contains("CGEventType::MouseMoved"));
    }

    #[test]
    fn linux_uinput_mouse_declares_every_button_it_can_emit() {
        let source = include_str!("linux_evdev.rs");
        let start = source
            .find("fn create_virtual_mouse()")
            .expect("missing Linux UInput mouse builder");
        let end = source[start..]
            .find("/// Send mouse move event")
            .map(|offset| start + offset)
            .expect("missing end of Linux UInput mouse builder");
        let builder = &source[start..end];

        for button in [
            "Key::BTN_LEFT",
            "Key::BTN_RIGHT",
            "Key::BTN_MIDDLE",
            "Key::BTN_SIDE",
            "Key::BTN_EXTRA",
        ] {
            assert!(builder.contains(button), "missing {button} capability");
        }
        assert!(builder.contains(".with_keys(&buttons)?"));

        let sender_start = source
            .find("pub fn send_mouse_button")
            .expect("missing Linux UInput button sender");
        let sender_end = source[sender_start..]
            .find("/// Send mouse wheel event")
            .map(|offset| sender_start + offset)
            .expect("missing end of Linux UInput button sender");
        let sender = &source[sender_start..sender_end];
        assert!(sender.contains("2 => Key(0x112)"));
        assert!(sender.contains("3 => Key(0x111)"));
        assert!(sender.contains("unsupported UInput mouse button code"));
    }
}

/// Platform-specific clipboard listener type alias
#[cfg(windows)]
pub type PlatformClipboardListener = clipboard::WindowsClipboardListener;

#[cfg(target_os = "macos")]
pub type PlatformClipboardListener = clipboard::MacosClipboardListener;

#[cfg(target_os = "linux")]
pub type PlatformClipboardListener = clipboard::LinuxClipboardListener;
