use std::path::PathBuf;
use std::time::Duration;

use rshare_core::{hello_message, HandshakeRejectReason, Message};
use rshare_net::{
    connection::{ConnectionManager, ManagerEvent},
    discovery::{DiscoveredDevice, PeerProtocolCompatibility},
    encryption::{
        Encryption, PeerCertificateFingerprint, QuicIdentity, QuicTrustStore, TrustProvenance,
    },
    QuicTransport,
};
use tokio::time::timeout;
use uuid::Uuid;

struct TestNetwork {
    state_dir: PathBuf,
}

impl TestNetwork {
    fn new(name: &str) -> Self {
        Self {
            state_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("rshare-state")
                .join(format!("{name}-{}", Uuid::new_v4())),
        }
    }

    fn transport(&self, device_id: Uuid, role: &str, identity: QuicIdentity) -> QuicTransport {
        QuicTransport::with_identity(device_id, identity)
            .with_trust_store_path(self.state_dir.join(role).join("quic-trust.json"))
    }

    fn manager(&self, device_id: Uuid, role: &str, identity: QuicIdentity) -> ConnectionManager {
        ConnectionManager::with_transport(device_id, self.transport(device_id, role, identity))
    }

    fn trust_store_path(&self, role: &str) -> PathBuf {
        self.state_dir.join(role).join("quic-trust.json")
    }

    fn approve_identity(&self, role: &str, device_id: Uuid, identity: &QuicIdentity) {
        QuicTrustStore::approve_at(
            self.trust_store_path(role),
            device_id,
            PeerCertificateFingerprint::from_der(&identity.cert_der),
        )
        .unwrap();
    }
}

impl Drop for TestNetwork {
    fn drop(&mut self) {
        if self.state_dir.exists() {
            std::fs::remove_dir_all(&self.state_dir)
                .expect("failed to clean isolated peer-identity test state");
        }
    }
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
                ManagerEvent::Connected(auth) => return Some(auth.peer_id),
                _ => {}
            }
        }
    })
    .await
    .ok()
    .flatten()
}

async fn assert_no_connected_event(
    events: &mut tokio::sync::mpsc::Receiver<ManagerEvent>,
    device_id: Uuid,
) {
    let no_connected = timeout(Duration::from_millis(350), async {
        while let Some(event) = events.recv().await {
            assert!(
                !matches!(event, ManagerEvent::Connected(auth) if auth.peer_id == device_id),
                "unauthorized peer entered the canonical registry"
            );
        }
    })
    .await;
    assert!(no_connected.is_err(), "event channel closed unexpectedly");
}

#[tokio::test]
async fn unknown_peer_requires_approval_before_entering_registry() {
    let server_id = Uuid::new_v4();
    let client_id = Uuid::new_v4();
    let network = TestNetwork::new("mutual");
    let mut server = network.manager(server_id, "server", generated_identity());
    let mut events = server.events().unwrap();
    let mut authenticated_peers = server.authenticated_peers().unwrap();
    server.start_server("127.0.0.1:0").await.unwrap();

    let client_identity = generated_identity();
    let client_fingerprint = PeerCertificateFingerprint::from_der(&client_identity.cert_der);
    let mut client = network.manager(client_id, "client", client_identity);
    let address = server.transport_local_addr().unwrap().to_string();
    assert!(client.connect(server_id, &address).await.is_err());
    assert_no_connected_event(&mut events, client_id).await;
    assert!(server.connections().is_empty());
    assert!(server.qos_registry().peer(&client_id).is_none());
    assert!(matches!(
        authenticated_peers.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    assert!(QuicTrustStore::load(network.trust_store_path("server"))
        .unwrap()
        .fingerprint_for(&client_id)
        .is_none());

    let approvals = server.pending_peer_approvals();
    assert_eq!(approvals.len(), 1);
    let approval = &approvals[0];
    assert_eq!(approval.device_id, client_id);
    assert_eq!(approval.fingerprint, client_fingerprint.as_str());
    assert!(server.approve_peer(&approval.approval_id));
    assert!(!server.approve_peer(&approval.approval_id));
    assert!(server.connections().is_empty());
    assert!(server.qos_registry().peer(&client_id).is_none());
    assert!(matches!(
        authenticated_peers.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    assert!(QuicTrustStore::load(network.trust_store_path("server"))
        .unwrap()
        .fingerprint_for(&client_id)
        .is_none());

    client.connect(server_id, &address).await.unwrap();

    assert_eq!(event_until_connected(&mut events).await, Some(client_id));
    assert_eq!(
        server.connections()[0].cert_trust_state.as_deref(),
        Some("trusted")
    );
    assert!(server.pending_peer_approvals().is_empty());
    assert_eq!(
        QuicTrustStore::load(network.trust_store_path("server"))
            .unwrap()
            .provenance_for(&client_id),
        Some(TrustProvenance::OperatorApproved)
    );
}

#[tokio::test]
async fn approval_for_one_fingerprint_cannot_be_consumed_by_another() {
    let server_id = Uuid::new_v4();
    let client_id = Uuid::new_v4();
    let network = TestNetwork::new("approval-fingerprint");
    let mut server = network.manager(server_id, "server", generated_identity());
    let mut events = server.events().unwrap();
    server.start_server("127.0.0.1:0").await.unwrap();
    let address = server.transport_local_addr().unwrap().to_string();

    let client_identity = generated_identity();
    let client_fingerprint = PeerCertificateFingerprint::from_der(&client_identity.cert_der);
    let mut client = network.manager(client_id, "client", client_identity);
    assert!(client.connect(server_id, &address).await.is_err());
    let approval = server.pending_peer_approvals().pop().unwrap();
    assert!(server.approve_peer(&approval.approval_id));

    let mut imposter = network.manager(client_id, "imposter", generated_identity());
    assert!(imposter.connect(server_id, &address).await.is_err());
    assert_no_connected_event(&mut events, client_id).await;
    assert!(server.connections().is_empty());
    assert!(server.qos_registry().peer(&client_id).is_none());
    assert!(QuicTrustStore::load(network.trust_store_path("server"))
        .unwrap()
        .fingerprint_for(&client_id)
        .is_none());

    client.connect(server_id, &address).await.unwrap();
    assert_eq!(event_until_connected(&mut events).await, Some(client_id));
    let trust_store = QuicTrustStore::load(network.trust_store_path("server")).unwrap();
    assert_eq!(
        trust_store.fingerprint_for(&client_id),
        Some(&client_fingerprint)
    );
}

#[tokio::test]
async fn peer_without_client_certificate_is_rejected_after_hello() {
    let server_id = Uuid::new_v4();
    let client_id = Uuid::new_v4();
    let network = TestNetwork::new("missing-client-cert");
    let mut server = network.manager(server_id, "server", generated_identity());
    let mut events = server.events().unwrap();
    server.start_server("127.0.0.1:0").await.unwrap();

    let mut client = network
        .transport(client_id, "client", generated_identity())
        .without_client_certificate();
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
    assert!(server.pending_peer_approvals().is_empty());
}

#[tokio::test]
async fn changed_fingerprint_never_enters_registry() {
    let server_id = Uuid::new_v4();
    let claimed_id = Uuid::new_v4();
    let network = TestNetwork::new("changed");
    let mut server = network.manager(server_id, "server", generated_identity());
    let mut events = server.events().unwrap();
    server.start_server("127.0.0.1:0").await.unwrap();
    let address = server.transport_local_addr().unwrap().to_string();

    let first_identity = generated_identity();
    let first_fingerprint = PeerCertificateFingerprint::from_der(&first_identity.cert_der);
    network.approve_identity("server", claimed_id, &first_identity);
    let mut first = network.manager(claimed_id, "client", first_identity);
    first.connect(server_id, &address).await.unwrap();
    assert_eq!(event_until_connected(&mut events).await, Some(claimed_id));
    first.disconnect(&server_id).await.unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(1), events.recv()).await,
        Ok(Some(ManagerEvent::Disconnected { peer_id, .. })) if peer_id == claimed_id
    ));
    assert!(server.connections().is_empty());

    let changed_identity = generated_identity();
    let changed_fingerprint = PeerCertificateFingerprint::from_der(&changed_identity.cert_der);
    let mut changed = network.manager(claimed_id, "changed", changed_identity);
    assert!(changed.connect(server_id, &address).await.is_err());
    assert_no_connected_event(&mut events, claimed_id).await;
    assert!(server.connections().is_empty());
    assert!(server.pending_peer_approvals().is_empty());
    let trust_store = QuicTrustStore::load(network.trust_store_path("server")).unwrap();
    assert_eq!(
        trust_store.fingerprint_for(&claimed_id),
        Some(&first_fingerprint),
        "a changed fingerprint must not replace the persisted pin"
    );
    assert_ne!(
        trust_store.fingerprint_for(&claimed_id),
        Some(&changed_fingerprint),
        "a changed fingerprint must never enter the trusted registry"
    );
    assert_eq!(
        trust_store.provenance_for(&claimed_id),
        Some(TrustProvenance::OperatorApproved)
    );
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
            local: rshare_core::PROTOCOL_VERSION,
            remote: 2
        }
    );
}

#[tokio::test]
async fn sequential_reconnect_assigns_new_control_connection_id() {
    let server_id = Uuid::new_v4();
    let client_id = Uuid::new_v4();
    let network = TestNetwork::new("sequential-reconnect");
    let client_identity = generated_identity();
    network.approve_identity("server", client_id, &client_identity);
    let mut server = network.manager(server_id, "server", generated_identity());
    let mut events = server.events().unwrap();
    server.start_server("127.0.0.1:0").await.unwrap();
    let address = server.transport_local_addr().unwrap().to_string();

    let mut first = network.manager(client_id, "client", client_identity.clone());
    first.connect(server_id, &address).await.unwrap();
    assert_eq!(event_until_connected(&mut events).await, Some(client_id));
    let old_control_id = server.connections()[0]
        .control_connection_id
        .expect("first negotiated connection id");

    // A remote close exercises transport recovery without suppressing inbound
    // reconnection as an explicit local disconnect would.
    first.disconnect(&server_id).await.unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(1), events.recv()).await,
        Ok(Some(ManagerEvent::Disconnected { peer_id, .. })) if peer_id == client_id
    ));
    assert!(server.connections().is_empty());

    let mut second = network.manager(client_id, "client", client_identity);
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
