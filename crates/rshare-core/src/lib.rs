//! R-ShareMouse core library
//!
//! This crate contains the core business logic for the R-ShareMouse application,
//! including protocol definitions, device management, configuration, and clipboard handling.

pub mod protocol;
pub mod device;
pub mod config;
pub mod clipboard;
pub mod engine;
pub mod service;

// Re-exports from protocol
pub use protocol::{
    DeviceId, Direction, ButtonState, MouseButton, KeyState, ScreenInfo,
    DeviceCapabilities, Message, Priority,
    hello_message, hello_back_message, heartbeat_message, timestamp_ms,
};

// Re-exports from device
pub use device::{Device, DeviceStatus, DevicePosition, ScreenLayout, DeviceRegistry};

// Re-exports from config
pub use config::Config;

// Re-exports from clipboard
pub use clipboard::ClipboardContent;
