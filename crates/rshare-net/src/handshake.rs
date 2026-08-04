use std::time::Duration;

use anyhow::{anyhow, Result};
use rshare_core::{
    hello_back_message, ControlConnectionId, DeviceId, HandshakeRejectReason, Message,
    PeerTransportCapabilities, ScreenInfo, DISCOVERY_APP_ID, PROTOCOL_VERSION,
};

use crate::{
    encryption::{PeerCertificateFingerprint, QuicTrustDecision},
    transport::QuicConnection,
};

pub const BOOTSTRAP_MAX_MESSAGE_SIZE: usize = 64 * 1024;
pub const BOOTSTRAP_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAuthContext {
    pub peer_id: DeviceId,
    pub certificate_fingerprint: PeerCertificateFingerprint,
    pub control_connection_id: ControlConnectionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientCertificatePolicy {
    OptionalForControlBootstrap,
    RequiredForMedia,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedPeer {
    pub auth: PeerAuthContext,
    pub transport_capabilities: PeerTransportCapabilities,
    pub inbound_trust_decision: Option<QuicTrustDecision>,
}

pub fn is_bootstrap_message(message: &Message) -> bool {
    matches!(
        message,
        Message::Hello { .. } | Message::HelloBack { .. } | Message::HelloRejected { .. }
    )
}

fn validate_bootstrap_message(message: &Message) -> Result<()> {
    if !is_bootstrap_message(message) {
        anyhow::bail!("message is not legal on the compatibility bootstrap stream");
    }
    let encoded = serde_json::to_vec(message)?;
    if encoded.len() > BOOTSTRAP_MAX_MESSAGE_SIZE {
        anyhow::bail!(
            "compatibility bootstrap message exceeds {} bytes",
            BOOTSTRAP_MAX_MESSAGE_SIZE
        );
    }
    Ok(())
}

fn validate_hello(
    app_id: &str,
    protocol_version: u32,
    transport_capabilities: &PeerTransportCapabilities,
) -> std::result::Result<(), HandshakeRejectReason> {
    if !app_id.eq_ignore_ascii_case(DISCOVERY_APP_ID) {
        return Err(HandshakeRejectReason::ApplicationMismatch);
    }
    if protocol_version != PROTOCOL_VERSION {
        return Err(HandshakeRejectReason::ProtocolMismatch {
            required: PROTOCOL_VERSION,
            received: protocol_version,
        });
    }
    let missing = transport_capabilities.missing_required_v3_capabilities();
    if !missing.is_empty() {
        return Err(HandshakeRejectReason::MissingCapabilities { missing });
    }
    Ok(())
}

async fn receive_bootstrap(conn: &mut QuicConnection) -> Result<Message> {
    let message = tokio::time::timeout(BOOTSTRAP_TIMEOUT, conn.receive_message())
        .await
        .map_err(|_| {
            anyhow!("peer identity handshake timed out during compatibility bootstrap")
        })??;
    validate_bootstrap_message(&message)?;
    Ok(message)
}

async fn send_rejection(
    conn: &QuicConnection,
    local_device_id: DeviceId,
    reason: HandshakeRejectReason,
) -> Result<()> {
    conn.send_message(&Message::HelloRejected {
        app_id: DISCOVERY_APP_ID.to_string(),
        device_id: local_device_id,
        reason,
    })
    .await
}

pub(crate) async fn receive_incoming_handshake(
    conn: &mut QuicConnection,
    local_device_id: DeviceId,
) -> Result<NegotiatedPeer> {
    let message = receive_bootstrap(conn).await?;
    let (app_id, peer_id, protocol_version, transport_capabilities) = match message {
        Message::Hello {
            app_id,
            device_id,
            protocol_version,
            transport_capabilities,
            ..
        } => (app_id, device_id, protocol_version, transport_capabilities),
        _ => anyhow::bail!("first compatibility-bootstrap message must be Hello"),
    };

    if let Err(reason) = validate_hello(&app_id, protocol_version, &transport_capabilities) {
        send_rejection(conn, local_device_id, reason.clone()).await?;
        anyhow::bail!("peer handshake rejected: {reason:?}");
    }

    let (certificate_fingerprint, inbound_trust_decision) =
        match conn.inspect_inbound_peer_identity(peer_id) {
            Ok(identity) => identity,
            Err(error) => {
                send_rejection(
                    conn,
                    local_device_id,
                    HandshakeRejectReason::IdentityUnavailable,
                )
                .await?;
                return Err(error.context("peer identity unavailable"));
            }
        };

    Ok(NegotiatedPeer {
        auth: PeerAuthContext {
            peer_id,
            certificate_fingerprint,
            control_connection_id: ControlConnectionId::new(),
        },
        transport_capabilities,
        inbound_trust_decision: Some(inbound_trust_decision),
    })
}

pub(crate) async fn complete_incoming_handshake(
    conn: &QuicConnection,
    local_device_id: DeviceId,
) -> Result<()> {
    conn.send_message(&hello_back_message(
        local_device_id,
        "R-ShareMouse".to_string(),
        hostname::get()
            .unwrap_or_else(|_| "unknown".into())
            .to_string_lossy()
            .to_string(),
        ScreenInfo::primary(),
    ))
    .await?;
    conn.complete_peer_protocol_handshake().await?;

    Ok(())
}

pub(crate) async fn perform_outbound_handshake(
    conn: &mut QuicConnection,
    local_device_id: DeviceId,
) -> Result<NegotiatedPeer> {
    conn.send_message(&rshare_core::hello_message(
        local_device_id,
        "R-ShareMouse".to_string(),
        hostname::get()
            .unwrap_or_else(|_| "unknown".into())
            .to_string_lossy()
            .to_string(),
    ))
    .await?;

    let response = receive_bootstrap(conn).await?;
    let (peer_id, transport_capabilities) = match response {
        Message::HelloRejected { reason, .. } => {
            anyhow::bail!("peer rejected compatibility bootstrap: {reason:?}")
        }
        Message::HelloBack {
            app_id,
            device_id,
            protocol_version,
            transport_capabilities,
            ..
        } => {
            if let Err(reason) = validate_hello(&app_id, protocol_version, &transport_capabilities)
            {
                anyhow::bail!("peer returned incompatible HelloBack: {reason:?}");
            }
            (device_id, transport_capabilities)
        }
        _ => anyhow::bail!("peer did not return HelloBack or HelloRejected"),
    };

    let certificate_fingerprint = conn.confirm_peer_identity(peer_id)?;
    conn.complete_peer_protocol_handshake().await?;
    Ok(NegotiatedPeer {
        auth: PeerAuthContext {
            peer_id,
            certificate_fingerprint,
            control_connection_id: ControlConnectionId::new(),
        },
        transport_capabilities,
        inbound_trust_decision: None,
    })
}
