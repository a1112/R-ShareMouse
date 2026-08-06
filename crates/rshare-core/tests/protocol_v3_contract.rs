use rshare_core::{
    hello_message, ControlConnectionId, HandshakeRejectReason, Message, PeerTransportCapabilities,
    PROTOCOL_VERSION,
};
use uuid::Uuid;

#[test]
fn protocol_v4_is_the_only_accepted_peer_version() {
    assert_eq!(PROTOCOL_VERSION, 4);
    let hello = hello_message(Uuid::new_v4(), "node".into(), "host".into());
    assert!(hello.advertises_required_v3_transport_capabilities());
}

#[test]
fn media_transport_is_optional_and_disabled_by_zero() {
    let required = PeerTransportCapabilities::required_v3();
    assert!(required.advertises_required_v3_transport_capabilities());
    assert_eq!(required.separate_media_quic_version, 0);
    assert_eq!(required.negotiated_media_version(&required), None);

    let mut local = required.clone();
    local.separate_media_quic_version = 2;
    let mut remote = required;
    remote.separate_media_quic_version = 2;
    assert_eq!(local.negotiated_media_version(&remote), Some(2));
    remote.separate_media_quic_version = 1;
    assert_eq!(local.negotiated_media_version(&remote), None);
}

#[test]
fn legacy_hello_without_transport_capabilities_remains_parseable() {
    let id = Uuid::new_v4();
    let legacy = format!(
        r#"{{"Hello":{{"app_id":"rsharemouse","device_id":"{id}","device_name":"old","hostname":"old-host","protocol_version":2,"capabilities":{{}}}}}}"#
    );
    let message: Message =
        serde_json::from_str(&legacy).expect("legacy Hello must remain parseable");
    assert!(!message.advertises_required_v3_transport_capabilities());
}

#[test]
fn rejection_reason_and_control_connection_id_round_trip() {
    let reason = HandshakeRejectReason::ProtocolMismatch {
        required: PROTOCOL_VERSION,
        received: 2,
    };
    let message = Message::HelloRejected {
        app_id: "rsharemouse".into(),
        device_id: Uuid::new_v4(),
        reason: reason.clone(),
    };
    let json = serde_json::to_vec(&message).unwrap();
    let decoded: Message = serde_json::from_slice(&json).unwrap();
    assert!(matches!(
        decoded,
        Message::HelloRejected {
            reason: HandshakeRejectReason::ProtocolMismatch {
                required: 4,
                received: 2
            },
            ..
        }
    ));

    assert_ne!(ControlConnectionId::new(), ControlConnectionId::new());
}
