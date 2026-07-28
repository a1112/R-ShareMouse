use std::time::Duration;

use rshare_core::{
    hello_message, DeviceCapabilities, HandshakeRejectReason, Message, PeerTransportCapabilities,
    ScreenInfo, DISCOVERY_APP_ID, PROTOCOL_VERSION,
};
use rshare_net::{
    connection::{ConnectionManager, ManagerEvent},
    handshake::{is_bootstrap_message, BOOTSTRAP_MAX_MESSAGE_SIZE},
    QuicTransport,
};
use tokio::time::timeout;
use uuid::Uuid;

fn v2_hello(device_id: Uuid) -> Message {
    Message::Hello {
        app_id: DISCOVERY_APP_ID.into(),
        device_id,
        device_name: "old".into(),
        hostname: "old-host".into(),
        protocol_version: 2,
        capabilities: DeviceCapabilities::default(),
        transport_capabilities: PeerTransportCapabilities::default(),
    }
}

async fn assert_no_connected(events: &mut tokio::sync::mpsc::Receiver<ManagerEvent>) {
    let result = timeout(Duration::from_millis(350), async {
        while let Some(event) = events.recv().await {
            assert!(
                !matches!(event, ManagerEvent::Connected(_)),
                "unauthenticated peer emitted Connected"
            );
        }
    })
    .await;
    assert!(result.is_err(), "manager event channel unexpectedly closed");
}

#[tokio::test]
async fn inbound_v2_hello_is_rejected_before_registration() {
    assert_v2_rejected_after_tls().await;
}

async fn assert_v2_rejected_after_tls() {
    let local_id = Uuid::new_v4();
    let remote_id = Uuid::new_v4();
    let mut manager = ConnectionManager::new(local_id);
    let mut events = manager.events().unwrap();
    manager.start_server("127.0.0.1:0").await.unwrap();

    let mut old = QuicTransport::new(remote_id).without_client_certificate();
    let mut connection = old
        .connect(
            &manager.transport_local_addr().unwrap().to_string(),
            local_id,
        )
        .await
        .unwrap();
    connection.send_message(&v2_hello(remote_id)).await.unwrap();

    assert!(matches!(
        connection.receive_message().await.unwrap(),
        Message::HelloRejected {
            reason: HandshakeRejectReason::ProtocolMismatch {
                required: PROTOCOL_VERSION,
                received: 2
            },
            ..
        }
    ));
    assert!(manager.connections().is_empty());
    assert_no_connected(&mut events).await;
}

#[tokio::test]
async fn timed_out_hello_never_emits_connected() {
    let local_id = Uuid::new_v4();
    let remote_id = Uuid::new_v4();
    let mut manager = ConnectionManager::new(local_id);
    let mut events = manager.events().unwrap();
    manager.start_server("127.0.0.1:0").await.unwrap();

    let mut silent = QuicTransport::new(remote_id);
    let _connection = silent
        .connect(
            &manager.transport_local_addr().unwrap().to_string(),
            local_id,
        )
        .await
        .unwrap();

    assert_no_connected(&mut events).await;
    assert!(manager.connections().is_empty());
}

#[tokio::test]
async fn non_hello_first_message_never_enters_registry() {
    let local_id = Uuid::new_v4();
    let remote_id = Uuid::new_v4();
    let mut manager = ConnectionManager::new(local_id);
    let mut events = manager.events().unwrap();
    manager.start_server("127.0.0.1:0").await.unwrap();

    let mut peer = QuicTransport::new(remote_id);
    let connection = peer
        .connect(
            &manager.transport_local_addr().unwrap().to_string(),
            local_id,
        )
        .await
        .unwrap();
    connection
        .send_message(&Message::Heartbeat {
            sequence: 1,
            timestamp: 1,
        })
        .await
        .unwrap();

    assert_no_connected(&mut events).await;
    assert!(manager.connections().is_empty());
}

#[tokio::test]
async fn outbound_surfaces_peer_rejection_reason() {
    let client_id = Uuid::new_v4();
    let server_id = Uuid::new_v4();
    let mut server = QuicTransport::new(server_id);
    server.start_server("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let mut incoming = server.incoming();
    let server_task = tokio::spawn(async move {
        let mut connection = incoming.recv().await.unwrap().connection;
        assert!(matches!(
            connection.receive_message().await.unwrap(),
            Message::Hello { .. }
        ));
        connection
            .send_message(&Message::HelloRejected {
                app_id: DISCOVERY_APP_ID.into(),
                device_id: server_id,
                reason: HandshakeRejectReason::ApplicationMismatch,
            })
            .await
            .unwrap();
    });

    let mut manager = ConnectionManager::new(client_id);
    let error = manager
        .connect(server_id, &address.to_string())
        .await
        .expect_err("peer rejection must fail the outbound connection");
    assert!(error.to_string().contains("ApplicationMismatch"));
    assert!(manager.connections().is_empty());
    server_task.await.unwrap();
}

#[tokio::test]
async fn old_peer_can_complete_tls_then_receive_explicit_version_rejection() {
    assert_v2_rejected_after_tls().await;
}

#[tokio::test]
async fn bootstrap_is_bounded_and_has_a_closed_allowlist() {
    assert!(BOOTSTRAP_MAX_MESSAGE_SIZE <= 64 * 1024);
    assert!(is_bootstrap_message(&hello_message(
        Uuid::new_v4(),
        "node".into(),
        "host".into()
    )));
    assert!(is_bootstrap_message(&Message::HelloBack {
        app_id: DISCOVERY_APP_ID.into(),
        device_id: Uuid::new_v4(),
        device_name: "node".into(),
        hostname: "host".into(),
        protocol_version: PROTOCOL_VERSION,
        capabilities: DeviceCapabilities::default(),
        transport_capabilities: PeerTransportCapabilities::required_v3(),
        screen_info: ScreenInfo::primary(),
    }));
    assert!(is_bootstrap_message(&Message::HelloRejected {
        app_id: DISCOVERY_APP_ID.into(),
        device_id: Uuid::new_v4(),
        reason: HandshakeRejectReason::IdentityUnavailable,
    }));
    assert!(!is_bootstrap_message(&Message::MouseMove { x: 1, y: 2 }));
}
