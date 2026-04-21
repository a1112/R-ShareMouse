//! R-ShareMouse networking layer
//!
//! This crate provides the networking functionality for R-ShareMouse,
//! including device discovery, QUIC transport, message encoding/decoding,
//! and connection management.

pub mod codec;
pub mod connection;
pub mod discovery;
pub mod encryption;
pub mod network_manager;
pub mod transport;

#[cfg(test)]
pub mod discovery_test;

pub use network_manager::*;

// Re-exports
pub use codec::*;
pub use discovery::*;
pub use encryption::*;
pub use transport::*;
