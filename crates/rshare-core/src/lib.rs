//! R-ShareMouse core library
//!
//! This crate contains the core business logic for the R-ShareMouse application,
//! including protocol definitions, device management, configuration, and clipboard handling.

pub mod clipboard;
pub mod config;
pub mod daemon_client;
pub mod device;
pub mod engine;
pub mod input_mode;
pub mod ipc;
pub mod layout;
pub mod protocol;
pub mod runtime;
pub mod service;
pub mod session;

// Re-exports from protocol
pub use protocol::{
    heartbeat_message, hello_back_message, hello_message, timestamp_ms, ButtonState,
    DeviceCapabilities, DeviceId, Direction, GamepadButton, GamepadButtonState, GamepadDeviceInfo,
    GamepadState, KeyState, Message, MouseButton, Priority, ScreenInfo,
};

// Re-exports from device
pub use device::{Device, DevicePosition, DeviceRegistry, DeviceStatus, ScreenLayout};

// Re-exports from config
pub use config::Config;
pub use config::{GamepadConfig, GamepadRoutingMode};

// Re-exports from clipboard
pub use clipboard::ClipboardContent;

// Re-exports from local daemon IPC
pub use ipc::{
    default_ipc_addr, read_json_line, write_json_line, DaemonDeviceSnapshot, DaemonRequest,
    DaemonResponse, ServiceStatusSnapshot,
};

// Re-exports from input_mode
pub use input_mode::{
    BackendFailureReason, BackendHealth, BackendKind, PrivilegeState, ResolvedInputMode,
};

// Re-exports from runtime
pub use runtime::{
    BackendRuntimeState, BackgroundProcessOwner, BackgroundRunMode, ConnectionState,
    ControlSessionState, DiscoveryState, PeerDirectoryEntry, SuspendReason, TrayRuntimeState,
};

// Re-exports from layout
pub use layout::{DisplayNode, LayoutGraph, LayoutLink, LayoutNode};

// Re-exports from session
pub use session::{CaptureSessionStateMachine, TransitionError};
