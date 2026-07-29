use std::{sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use rshare_core::{
    CapabilityRegistrySnapshot, DaemonResponse, DeviceId, LayoutGraph, LocalDisplayState,
    LocalInputDiagnosticEvent, ServiceStatusSnapshot, UiActiveSessions, UiDynamicState, UiEnvelope,
    UiSnapshot, UI_STATE_PROTOCOL_VERSION,
};
use rshare_daemon::{
    state_aggregator::{StateAggregator, StateChange},
    ui_state_server::{
        run_ui_state_server_on_listener, run_ui_state_server_on_listener_with_config,
        LocalControlsFeed, UiStateServerConfig,
    },
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
    sync::broadcast,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{HeaderValue, StatusCode},
        Error as WsError, Message as WsMessage,
    },
    MaybeTlsStream, WebSocketStream,
};

type ClientWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn fixture_snapshot() -> UiSnapshot {
    let local_id = DeviceId::from_u128(1);
    UiSnapshot {
        protocol_version: UI_STATE_PROTOCOL_VERSION,
        boot_id: DeviceId::nil(),
        revision: 99,
        status: ServiceStatusSnapshot::new(
            local_id,
            "local".into(),
            "host".into(),
            "127.0.0.1:27435".into(),
            27432,
            42,
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

fn local_controls_feed() -> LocalControlsFeed {
    let (events, _) = broadcast::channel::<LocalInputDiagnosticEvent>(8);
    LocalControlsFeed::new(
        Arc::new(|| Box::pin(async { Ok(Default::default()) })),
        events,
    )
}

async fn connect(
    address: std::net::SocketAddr,
    path: &str,
    origin: Option<&str>,
) -> Result<ClientWebSocket, WsError> {
    let mut request = format!("ws://{address}{path}")
        .into_client_request()
        .expect("valid websocket request");
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert("Origin", HeaderValue::from_str(origin).unwrap());
    }
    connect_async(request).await.map(|(socket, _)| socket)
}

async fn next_ui_envelope(socket: &mut ClientWebSocket) -> UiEnvelope {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(1), socket.next())
            .await
            .expect("websocket should respond")
            .expect("websocket should stay open")
            .expect("websocket message should decode");
        if let WsMessage::Text(text) = message {
            return serde_json::from_str(&text).expect("typed UI envelope");
        }
    }
}

#[tokio::test]
async fn ui_state_websocket_streams_snapshot_delta_and_full_resync() {
    let aggregator = StateAggregator::new(fixture_snapshot(), 16);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let server = tokio::spawn(run_ui_state_server_on_listener(
        listener,
        aggregator.clone(),
        local_controls_feed(),
        shutdown_rx,
    ));

    let mut socket = connect(address, "/ui-state", Some("http://127.0.0.1:5176"))
        .await
        .expect("loopback origin should be accepted");
    socket
        .send(WsMessage::Text(
            r#"{"type":"subscribe","cursor":null}"#.into(),
        ))
        .await
        .unwrap();

    let initial = next_ui_envelope(&mut socket).await;
    let UiEnvelope::Snapshot(initial) = initial else {
        panic!("first UI envelope must be a snapshot");
    };
    assert_eq!(initial.revision, 0);

    let mut updated_status = initial.status.clone();
    updated_status.started_by_desktop = true;
    aggregator
        .publish(StateChange::Status(updated_status.clone()))
        .await
        .unwrap();
    let delta = next_ui_envelope(&mut socket).await;
    assert!(matches!(
        delta,
        UiEnvelope::Delta(ref delta)
            if delta.revision == 1
                && matches!(&delta.change, rshare_core::UiChange::Status(status) if status == &updated_status)
    ));

    socket
        .send(WsMessage::Text(r#"{"type":"resync"}"#.into()))
        .await
        .unwrap();
    let resynced = next_ui_envelope(&mut socket).await;
    assert!(matches!(
        resynced,
        UiEnvelope::Snapshot(ref snapshot) if snapshot.revision == 1
    ));

    let _ = shutdown_tx.send(());
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn websocket_rejects_unknown_path_and_non_loopback_origin_before_state() {
    let aggregator = StateAggregator::new(fixture_snapshot(), 8);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let server = tokio::spawn(run_ui_state_server_on_listener(
        listener,
        aggregator,
        local_controls_feed(),
        shutdown_rx,
    ));

    let unknown = connect(address, "/private-state", Some("http://127.0.0.1:5176"))
        .await
        .unwrap_err();
    assert!(matches!(
        unknown,
        WsError::Http(ref response) if response.status() == StatusCode::NOT_FOUND
    ));

    let hostile = connect(address, "/ui-state", Some("https://attacker.example"))
        .await
        .unwrap_err();
    assert!(matches!(
        hostile,
        WsError::Http(ref response) if response.status() == StatusCode::FORBIDDEN
    ));

    let _ = shutdown_tx.send(());
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn existing_local_controls_route_remains_available_to_native_clients() {
    let aggregator = StateAggregator::new(fixture_snapshot(), 8);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let server = tokio::spawn(run_ui_state_server_on_listener(
        listener,
        aggregator,
        local_controls_feed(),
        shutdown_rx,
    ));

    let mut socket = connect(address, "/local-controls", None)
        .await
        .expect("native local-controls client should remain compatible");
    let response = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let WsMessage::Text(text) = response else {
        panic!("local controls initial response should be JSON text");
    };
    let response: DaemonResponse = serde_json::from_str(&text).unwrap();
    assert!(matches!(response, DaemonResponse::LocalControls(_)));

    let _ = shutdown_tx.send(());
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn websocket_handshake_and_first_subscribe_have_hard_timeouts() {
    let aggregator = StateAggregator::new(fixture_snapshot(), 8);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let config = UiStateServerConfig {
        handshake_timeout: Duration::from_millis(30),
        subscribe_timeout: Duration::from_millis(30),
        ..UiStateServerConfig::default()
    };
    let server = tokio::spawn(run_ui_state_server_on_listener_with_config(
        listener,
        aggregator,
        local_controls_feed(),
        shutdown_rx,
        config,
    ));

    let mut idle_tcp = TcpStream::connect(address).await.unwrap();
    let mut byte = [0_u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(1), idle_tcp.read(&mut byte))
        .await
        .expect("handshake timeout must close the idle TCP peer")
        .unwrap();
    assert_eq!(read, 0);

    let mut idle_websocket = connect(address, "/ui-state", Some("http://localhost:5176"))
        .await
        .unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(1), idle_websocket.next())
        .await
        .expect("subscribe timeout must close the upgraded websocket");
    assert!(
        ended.is_none()
            || matches!(ended, Some(Ok(WsMessage::Close(_))))
            || matches!(ended, Some(Err(_)))
    );

    let _ = shutdown_tx.send(());
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn websocket_connection_count_and_message_bytes_are_bounded() {
    let aggregator = StateAggregator::new(fixture_snapshot(), 8);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let config = UiStateServerConfig {
        max_connections: 1,
        handshake_timeout: Duration::from_secs(1),
        subscribe_timeout: Duration::from_secs(1),
        max_message_bytes: 64,
    };
    let server = tokio::spawn(run_ui_state_server_on_listener_with_config(
        listener,
        aggregator,
        local_controls_feed(),
        shutdown_rx,
        config,
    ));

    let first = TcpStream::connect(address).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let capped = tokio::time::timeout(
        Duration::from_secs(1),
        connect(address, "/ui-state", Some("http://localhost:5176")),
    )
    .await
    .expect("connection above the cap must be rejected promptly");
    assert!(capped.is_err());
    drop(first);

    tokio::time::sleep(Duration::from_millis(30)).await;
    let mut socket = connect(address, "/ui-state", Some("http://localhost:5176"))
        .await
        .expect("permit must be released after the first connection closes");
    socket.send(WsMessage::Text("x".repeat(256))).await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .expect("oversized first message must close the websocket");
    assert!(
        ended.is_none()
            || matches!(ended, Some(Ok(WsMessage::Close(_))))
            || matches!(ended, Some(Err(_)))
    );

    let _ = shutdown_tx.send(());
    server.await.unwrap().unwrap();
}
