use std::sync::{Arc, RwLock};

use futures_util::future::BoxFuture;
use rshare_core::{
    daemon_client::subscribe_ui_state_at, CapabilityRegistrySnapshot, DaemonDeviceSnapshot,
    DeviceId, LayoutGraph, LocalDisplayState, ServiceStatusSnapshot, UiActiveSessions, UiChange,
    UiDynamicState, UiEnvelope, UiSnapshot, UI_STATE_PROTOCOL_VERSION,
};
use rshare_daemon::{
    input_state::input_state_channel,
    ipc_server::{read_json_request, stream_ui_state, ui_state_subscriber_for_request},
    state_aggregator::{StateAggregator, UiProjectionSource},
};
use tokio::net::TcpListener;

#[derive(Clone)]
struct TestProjection {
    snapshot: Arc<RwLock<UiSnapshot>>,
}

impl UiProjectionSource for TestProjection {
    fn project(&self) -> BoxFuture<'_, anyhow::Result<UiSnapshot>> {
        let snapshot = self.snapshot.read().unwrap().clone();
        Box::pin(async move { Ok(snapshot) })
    }
}

fn snapshot() -> UiSnapshot {
    let local_id = DeviceId::from_u128(1);
    UiSnapshot {
        protocol_version: UI_STATE_PROTOCOL_VERSION,
        boot_id: DeviceId::nil(),
        revision: 0,
        status: ServiceStatusSnapshot::new(
            local_id,
            "local".into(),
            "host".into(),
            "127.0.0.1:0".into(),
            27432,
            1,
        ),
        devices: Vec::new(),
        layout: LayoutGraph::new(local_id),
        capabilities: CapabilityRegistrySnapshot {
            local_device_id: local_id,
            generated_at_ms: 0,
            devices: Vec::new(),
        },
        display_inventory: LocalDisplayState::default(),
        dynamic_state: UiDynamicState::default(),
        active_sessions: UiActiveSessions::default(),
    }
}

#[tokio::test]
async fn one_ephemeral_connection_receives_snapshot_then_reconciled_delta() {
    let initial = snapshot();
    let projection = TestProjection {
        snapshot: Arc::new(RwLock::new(initial.clone())),
    };
    let (_input, feeds) = input_state_channel(4);
    let aggregator =
        StateAggregator::with_projection(initial, 16, feeds, Arc::new(projection.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    assert_ne!(address.port(), rshare_core::ipc::DEFAULT_IPC_PORT);

    let server_state = aggregator.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_json_request(&mut stream).await.unwrap().unwrap();
        let subscriber = ui_state_subscriber_for_request(&request, &server_state)
            .await
            .unwrap()
            .expect("first request must switch this connection to UI streaming");
        stream_ui_state(&mut stream, subscriber).await
    });

    let mut client = subscribe_ui_state_at(address, None).await.unwrap();
    let Some(UiEnvelope::Snapshot(initial)) = client.recv().await.unwrap() else {
        panic!("first stream envelope must be a snapshot");
    };

    projection
        .snapshot
        .write()
        .unwrap()
        .devices
        .push(DaemonDeviceSnapshot {
            id: DeviceId::from_u128(2),
            name: "peer".into(),
            hostname: "peer-host".into(),
            addresses: vec!["127.0.0.1:27432".into()],
            connected: true,
            last_seen_secs: Some(0),
        });
    aggregator.reconcile_from_projection().await.unwrap();

    let Some(UiEnvelope::Delta(delta)) = client.recv().await.unwrap() else {
        panic!("same connection must receive the live delta");
    };
    assert_eq!(delta.boot_id, initial.boot_id);
    assert_eq!(delta.revision, initial.revision + 1);
    assert!(matches!(delta.change, UiChange::DeviceUpsert(_)));

    drop(client);
    server.abort();
    let _ = server.await;
}
