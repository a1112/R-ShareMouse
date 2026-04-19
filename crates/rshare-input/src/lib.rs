//! R-ShareMouse input handling layer
//!
//! This crate provides the abstraction for input handling,
//! including input listening, input emulation, screen edge detection,
//! and event transformation.

pub mod listener;
pub mod emulator;
pub mod events;
pub mod edge_detection;
pub mod backend;
pub mod selection;
pub mod privilege;

// Re-exports
pub use listener::*;
pub use emulator::*;
pub use events::*;
pub use edge_detection::*;

// Backend re-exports
pub use backend::{
    CaptureBackend, InjectBackend, PrivilegeBackend, NoopPrivilegeBackend,
    PortableCaptureBackend, PortableInjectBackend,
};
pub use selection::{BackendCandidate, BackendSelector, SelectionResult};
