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
pub mod ipc;
pub mod daemon_client;
pub mod input_mode;
pub mod layout;
pub mod runtime;
pub mod session;

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

// Re-exports from local daemon IPC
pub use ipc::{
    default_ipc_addr, read_json_line, write_json_line, DaemonDeviceSnapshot,
    DaemonRequest, DaemonResponse, ServiceStatusSnapshot,
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
