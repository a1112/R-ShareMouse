use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The peer protocol version supported by this build.
pub const PROTOCOL_VERSION: u32 = 4;

/// Versions and lane support advertised during the compatibility bootstrap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PeerTransportCapabilities {
    pub realtime_input_version: u16,
    pub reliable_input_version: u16,
    pub qos_lanes: bool,
    /// Optional capability. Zero means media is disabled or unsupported.
    pub separate_media_quic_version: u16,
    /// Optional bulk file-drop protocol. Missing on older compatible peers.
    #[serde(default)]
    pub file_transfer_version: u16,
    #[serde(default)]
    pub folder_drop_version: u16,
    /// Both endpoints acknowledge their committed receive guards before use.
    #[serde(default)]
    pub bootstrap_commit_ack: bool,
}

impl PeerTransportCapabilities {
    pub const REALTIME_INPUT_VERSION: u16 = 1;
    pub const RELIABLE_INPUT_VERSION: u16 = 1;

    pub fn required_v3() -> Self {
        Self {
            realtime_input_version: Self::REALTIME_INPUT_VERSION,
            reliable_input_version: Self::RELIABLE_INPUT_VERSION,
            qos_lanes: true,
            separate_media_quic_version: 0,
            file_transfer_version: 1,
            folder_drop_version: if cfg!(any(windows, target_os = "macos")) {
                1
            } else {
                0
            },
            bootstrap_commit_ack: true,
        }
    }

    pub fn advertises_required_v3_transport_capabilities(&self) -> bool {
        self.realtime_input_version == Self::REALTIME_INPUT_VERSION
            && self.reliable_input_version == Self::RELIABLE_INPUT_VERSION
            && self.qos_lanes
            && self.bootstrap_commit_ack
    }

    pub fn missing_required_v3_capabilities(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.realtime_input_version != Self::REALTIME_INPUT_VERSION {
            missing.push("realtime-input".to_string());
        }
        if self.reliable_input_version != Self::RELIABLE_INPUT_VERSION {
            missing.push("reliable-input".to_string());
        }
        if !self.qos_lanes {
            missing.push("qos-lanes".to_string());
        }
        if !self.bootstrap_commit_ack {
            missing.push("bootstrap-commit-ack".to_string());
        }
        missing
    }

    pub fn negotiated_media_version(&self, remote: &Self) -> Option<u16> {
        (self.separate_media_quic_version != 0
            && self.separate_media_quic_version == remote.separate_media_quic_version)
            .then_some(self.separate_media_quic_version)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HandshakeRejectReason {
    ProtocolMismatch { required: u32, received: u32 },
    ApplicationMismatch,
    MissingCapabilities { missing: Vec<String> },
    IdentityUnavailable,
}

/// Unique identity for one authenticated control connection.
///
/// UUID v4 values are generated independently for every successful
/// negotiation and are never recycled by the process.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ControlConnectionId(Uuid);

impl ControlConnectionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ControlConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_capabilities_do_not_claim_mutual_commit_acknowledgement() {
        let legacy: PeerTransportCapabilities = serde_json::from_str(
            r#"{"realtime_input_version":1,"reliable_input_version":1,"qos_lanes":true,"separate_media_quic_version":0}"#,
        ).unwrap();
        assert!(!legacy.bootstrap_commit_ack);
        assert!(legacy
            .missing_required_v3_capabilities()
            .contains(&"bootstrap-commit-ack".to_string()));
        assert!(PeerTransportCapabilities::required_v3().bootstrap_commit_ack);
    }
}
