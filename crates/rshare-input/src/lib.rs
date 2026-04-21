//! R-ShareMouse input handling layer
//!
//! This crate provides the abstraction for input handling,
//! including input listening, input emulation, screen edge detection,
//! and event transformation.

pub mod backend;
pub mod edge_detection;
pub mod emulator;
pub mod events;
pub mod listener;
pub mod privilege;
pub mod selection;

// Re-exports
pub use edge_detection::*;
pub use emulator::*;
pub use events::*;
pub use listener::*;

// Backend re-exports
pub use backend::{
    CaptureBackend, InjectBackend, NoopPrivilegeBackend, PortableCaptureBackend,
    PortableInjectBackend, PrivilegeBackend,
};
pub use selection::{BackendCandidate, BackendSelector, SelectionResult};
