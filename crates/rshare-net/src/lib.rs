//! R-ShareMouse networking layer
//!
//! This crate provides the networking functionality for R-ShareMouse,
//! including device discovery, QUIC transport, message encoding/decoding,
//! and connection management.

pub mod codec;
pub mod connection;
pub mod discovery;
pub mod encryption;
pub mod handshake;
pub mod network_manager;
pub mod qos;
pub mod transport;

#[cfg(test)]
pub mod discovery_test;

pub use network_manager::*;

// Re-exports
pub use codec::*;
pub use discovery::*;
pub use encryption::*;
pub use qos::{BulkFrame, ControlFrame, TelemetryFrame};
pub use transport::*;

#[cfg(test)]
mod public_api_tests {
    use super::{
        BulkFrame, ControlFrame, NetworkEvent, NetworkReceivers, PeerInbound, TelemetryFrame,
    };

    #[test]
    fn typed_inbound_api_is_reexported_from_the_crate_root() {
        fn assert_send<T: Send>() {}
        assert_send::<PeerInbound>();
        assert_send::<NetworkReceivers>();
        assert_send::<NetworkEvent>();
        assert_send::<ControlFrame>();
        assert_send::<TelemetryFrame>();
        assert_send::<BulkFrame>();
    }
}
