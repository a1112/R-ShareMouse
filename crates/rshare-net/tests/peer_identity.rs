use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rshare_core::{hello_message, HandshakeRejectReason, Message};
use rshare_net::{
    connection::{ConnectionManager, ManagerEvent},
    discovery::{DiscoveredDevice, PeerProtocolCompatibility},
    encryption::{Encryption, QuicIdentity},
    QuicTransport,
};
use tokio::time::timeout;
use uuid::Uuid;

fn temp_trust_store_path(name: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("rshare-v3-{name}-{suffix}.json"))
}

fn generated_identity() -> QuicIdentity {
    let (cert_der, key_der) = Encryption::generate_cert().unwrap();
    QuicIdentity { cert_der, key_der }
}

async fn event_until_connected(
    events: &mut tokio::sync::mpsc::Receiver<ManagerEvent>,
) -> Option<Uuid> {
    timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await? {
                ManagerEvent::Connected(id) => return Some(id),
                _ => {}
            }
        }
    })
    .await
    .ok()
    .flatten()
}

#[tokio::test]
async fn v3_peers_exchange_certificate_identity() {
    let server_id = Uuid::new_v4();
    let client_id = Uuid::new_v4();
    let trust = temp_trust_store_path("mutual");
    let mut server = ConnectionManager::with_transport(
        server_id,
        QuicTransport::with_identity(server_id, generated_identity()).with_trust_store_path(&trust),
    );
    let mut events = server.events().unwrap();
    server.start_server("127.0.0.1:0").await.unwrap();

    let mut client = ConnectionManager::with_transport(
        client_id,
        QuicTransport::with_identity(client_id, generated_identity()).with_trust_store_path(&trust),
    );
    client
        .connect(
            server_id,
            &server.transport_local_addr().unwrap().to_string(),
        )
        .await
        .unwrap();

    assert_eq!(event_until_connected(&mut events).await, Some(client_id));
    assert_eq!(
        server.connections()[0].cert_trust_state.as_deref(),
        Some("trusted")
    );
}

#[tokio::test]
async fn v3_peer_without_client_certificate_is_rejected_after_hello() {
    let server_id = Uuid::new_v4();
    let client_id = Uuid::new_v4();
    let mut server = ConnectionManager::new(server_id);
    let mut events = server.events().unwrap();
    server.start_server("127.0.0.1:0").await.unwrap();

    let mut client = QuicTransport::new(client_id).without_client_certificate();
    let mut connection = client
        .connect(
            &server.transport_local_addr().unwrap().to_string(),
            server_id,
        )
        .await
        .unwrap();
    connection
        .send_message(&hello_message(client_id, "client".into(), "host".into()))
        .await
        .unwrap();
    assert!(matches!(
        connection.receive_message().await.unwrap(),
        Message::HelloRejected {
            reason: HandshakeRejectReason::IdentityUnavailable,
            ..
        }
    ));
    assert!(event_until_connected(&mut events).await.is_none());
    assert!(server.connections().is_empty());
}

#[tokio::test]
async fn changed_fingerprint_never_enters_registry() {
    let server_id = Uuid::new_v4();
    let claimed_id = Uuid::new_v4();
    let trust = temp_trust_store_path("changed");
    let mut server = ConnectionManager::with_transport(
        server_id,
        QuicTransport::with_identity(server_id, generated_identity()).with_trust_store_path(&trust),
    );
    let mut events = server.events().unwrap();
    server.start_server("127.0.0.1:0").await.unwrap();
    let address = server.transport_local_addr().unwrap().to_string();

    let mut first = ConnectionManager::with_transport(
        claimed_id,
        QuicTransport::with_identity(claimed_id, generated_identity())
            .with_trust_store_path(&trust),
    );
    first.connect(server_id, &address).await.unwrap();
    assert_eq!(event_until_connected(&mut events).await, Some(claimed_id));
    server.disconnect(&claimed_id).await.unwrap();
    assert!(server.connections().is_empty());

    let mut changed = QuicTransport::with_identity(claimed_id, generated_identity())
        .with_trust_store_path(&trust);
    let mut connection = changed.connect(&address, server_id).await.unwrap();
    connection
        .send_message(&hello_message(claimed_id, "changed".into(), "host".into()))
        .await
        .unwrap();
    assert!(matches!(
        connection.receive_message().await.unwrap(),
        Message::HelloRejected { .. }
    ));
    let no_connected = timeout(Duration::from_millis(350), async {
        while let Some(event) = events.recv().await {
            assert!(
                !matches!(event, ManagerEvent::Connected(id) if id == claimed_id),
                "changed fingerprint entered the canonical registry"
            );
        }
    })
    .await;
    assert!(no_connected.is_err());
    assert!(server.connections().is_empty());
}

#[test]
fn discovery_surfaces_old_peer_as_incompatible_without_connecting() {
    let remote_id = Uuid::new_v4();
    let message = serde_json::from_value(serde_json::json!({
        "Hello": {
            "app_id": "rsharemouse",
            "device_id": remote_id,
            "device_name": "old",
            "hostname": "old-host",
            "protocol_version": 2,
            "capabilities": {}
        }
    }))
    .unwrap();
    let discovered =
        DiscoveredDevice::from_announcement("127.0.0.1:27432".parse().unwrap(), &message).unwrap();
    assert_eq!(
        discovered.protocol_compatibility,
        PeerProtocolCompatibility::Incompatible {
            local: 3,
            remote: 2
        }
    );
}

#[tokio::test]
async fn sequential_reconnect_assigns_new_control_connection_id() {
    let server_id = Uuid::new_v4();
    let client_id = Uuid::new_v4();
    let mut server = ConnectionManager::new(server_id);
    let mut events = server.events().unwrap();
    server.start_server("127.0.0.1:0").await.unwrap();
    let address = server.transport_local_addr().unwrap().to_string();

    let mut first = ConnectionManager::new(client_id);
    first.connect(server_id, &address).await.unwrap();
    assert_eq!(event_until_connected(&mut events).await, Some(client_id));
    let old_control_id = server.connections()[0]
        .control_connection_id
        .expect("first negotiated connection id");

    server.disconnect(&client_id).await.unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(1), events.recv()).await,
        Ok(Some(ManagerEvent::Disconnected(id))) if id == client_id
    ));
    assert!(server.connections().is_empty());

    let mut second = ConnectionManager::new(client_id);
    second.connect(server_id, &address).await.unwrap();
    assert_eq!(event_until_connected(&mut events).await, Some(client_id));
    let replacement_control_id = server.connections()[0]
        .control_connection_id
        .expect("replacement negotiated connection id");
    assert_ne!(old_control_id, replacement_control_id);

    let replacement = server
        .connections()
        .into_iter()
        .find(|connection| connection.device_id == client_id)
        .expect("replacement generation must remain registered");
    assert_eq!(
        replacement.control_connection_id,
        Some(replacement_control_id)
    );
    assert_eq!(server.connected_count().await, 1);
}
