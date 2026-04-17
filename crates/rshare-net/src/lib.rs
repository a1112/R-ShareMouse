//! R-ShareMouse networking layer
//!
//! This crate provides the networking functionality for R-ShareMouse,
//! including device discovery, QUIC transport, message encoding/decoding,
//! and connection management.

pub mod connection;
pub mod discovery;
pub mod transport;
pub mod codec;
pub mod encryption;
pub mod network_manager;

pub use network_manager::*;

// Re-exports
pub use discovery::*;
pub use transport::*;
pub use codec::*;
pub use encryption::*;
