//! Network manager - unified discovery and connection management

use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as TokioMutex, RwLock};
use tokio::task::JoinHandle;

use crate::{
    connection::{ConnectionInfo, ConnectionManager, ConnectionView, ManagerEvent},
    discovery::{DiscoveredDevice, DiscoveryEvent, PeerProtocolCompatibility, ServiceDiscovery},
    handshake::PeerAuthContext,
    qos::{
        ClassifiedMessage, ConnectionRegistry, ControlFrame, TerminalReleaseEvent,
        TransportSendError,
    },
    transport::PeerInbound,
};
use rshare_core::{ControlConnectionId, DeviceId, Message};

/// Network event
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    /// Device discovered
    DeviceFound(DiscoveredDevice),
    /// Device connected
    DeviceConnected(PeerAuthContext),
    /// Device disconnected
    DeviceDisconnected {
        peer_id: DeviceId,
        control_connection_id: ControlConnectionId,
    },
    ControlReceived {
        auth: Arc<PeerAuthContext>,
        frame: ControlFrame,
    },
    /// Connection error
    ConnectionError {
        peer_id: Option<DeviceId>,
        control_connection_id: Option<ControlConnectionId>,
        error: String,
    },
}

pub struct NetworkReceivers {
    /// Yields one isolated receiver set per authenticated connection generation.
    pub authenticated_peers: mpsc::Receiver<PeerInbound>,
    pub events: mpsc::Receiver<NetworkEvent>,
}

/// Network manager configuration
#[derive(Debug, Clone)]
pub struct NetworkManagerConfig {
    /// Discovery port
    pub discovery_port: u16,
    /// Transport bind address
    pub bind_address: String,
    /// Retained for configuration compatibility; unpaired discovery is observational only.
    pub auto_connect: bool,
    /// Discovery broadcast interval
    pub broadcast_interval: Duration,
    /// Device timeout
    pub device_timeout: Duration,
}

impl Default for NetworkManagerConfig {
    fn default() -> Self {
        Self {
            discovery_port: 27432,
            bind_address: "0.0.0.0:27431".to_string(),
            auto_connect: false,
            broadcast_interval: Duration::from_secs(5),
            device_timeout: Duration::from_secs(30),
        }
    }
}

/// Unified network manager for discovery and connection management
pub struct NetworkManager {
    local_device_id: DeviceId,
    local_device_name: String,
    local_hostname: String,
    config: NetworkManagerConfig,

    discovery: ServiceDiscovery,
    connection: Arc<TokioMutex<ConnectionManager>>,
    connection_view: ConnectionView,
    qos_registry: Arc<ConnectionRegistry>,

    event_tx: mpsc::Sender<NetworkEvent>,
    event_rx: Option<mpsc::Receiver<NetworkEvent>>,
    authenticated_peer_rx: Option<mpsc::Receiver<PeerInbound>>,

    discovered_devices: Arc<RwLock<HashMap<DeviceId, DiscoveredDevice>>>,
    running: bool,
    discovery_task: Option<JoinHandle<()>>,
}

fn spawn_connection_event_forwarder(
    mut manager_events: mpsc::Receiver<ManagerEvent>,
    network_tx: mpsc::Sender<NetworkEvent>,
) {
    tokio::spawn(async move {
        while let Some(event) = manager_events.recv().await {
            let network_event = match event {
                ManagerEvent::Connected(auth) => NetworkEvent::DeviceConnected(auth),
                ManagerEvent::Disconnected {
                    peer_id,
                    control_connection_id,
                } => NetworkEvent::DeviceDisconnected {
                    peer_id,
                    control_connection_id,
                },
                ManagerEvent::ControlReceived { auth, frame } => {
                    NetworkEvent::ControlReceived { auth, frame }
                }
                ManagerEvent::ProtocolError { auth, error } => NetworkEvent::ConnectionError {
                    peer_id: Some(auth.peer_id),
                    control_connection_id: Some(auth.control_connection_id),
                    error,
                },
                ManagerEvent::Error {
                    peer_id,
                    control_connection_id,
                    error,
                } => NetworkEvent::ConnectionError {
                    peer_id,
                    control_connection_id,
                    error,
                },
                ManagerEvent::MessageReceived { message, .. } => {
                    let _ = ClassifiedMessage::try_from(message);
                    continue;
                }
            };

            if network_tx.send(network_event).await.is_err() {
                break;
            }
        }
    });
}

fn record_qos_broadcast_successes(
    connection_view: &ConnectionView,
    results: &[(
        DeviceId,
        ControlConnectionId,
        std::result::Result<(), TransportSendError>,
    )],
) {
    for (device_id, generation, result) in results {
        if result.is_ok() {
            connection_view.record_send_success(device_id, *generation);
        }
    }
}

async fn handle_discovery_event(
    event: DiscoveryEvent,
    config: &NetworkManagerConfig,
    discovered_devices: &Arc<RwLock<HashMap<DeviceId, DiscoveredDevice>>>,
    discovery_tx: &mpsc::Sender<NetworkEvent>,
    connection_view: &ConnectionView,
) {
    if config.auto_connect
        && matches!(
            &event,
            DiscoveryEvent::DeviceFound(_) | DiscoveryEvent::DeviceUpdated(_)
        )
    {
        tracing::debug!(
            "Ignoring legacy auto_connect=true; unpaired discovery remains observational"
        );
    }

    match event {
        DiscoveryEvent::DeviceFound(device) | DiscoveryEvent::DeviceUpdated(device) => {
            let device_id = device.id;
            {
                let mut devices = discovered_devices.write().await;
                devices.insert(device_id, device.clone());
            }
            let _ = discovery_tx.try_send(NetworkEvent::DeviceFound(device));
        }
        DiscoveryEvent::DeviceLost(id) => {
            {
                let mut devices = discovered_devices.write().await;
                devices.remove(&id);
            }

            let transport_connected = connection_view.is_connected(&id).await;
            if let Some(event) = discovery_lost_network_event(id, transport_connected) {
                let _ = discovery_tx.try_send(event);
            }
        }
        DiscoveryEvent::Error(error) => {
            tracing::error!("Discovery error: {}", error);
        }
    }
}

impl NetworkManager {
    /// Create a new network manager
    pub fn new(
        local_device_id: DeviceId,
        local_device_name: String,
        local_hostname: String,
    ) -> Self {
        Self::with_connection_manager(
            local_device_id,
            local_device_name,
            local_hostname,
            ConnectionManager::new(local_device_id),
        )
    }

    #[cfg(test)]
    fn isolated_for_test(
        local_device_id: DeviceId,
        local_device_name: String,
        local_hostname: String,
    ) -> Self {
        Self::with_connection_manager(
            local_device_id,
            local_device_name,
            local_hostname,
            ConnectionManager::isolated_for_test(local_device_id),
        )
    }

    fn with_connection_manager(
        local_device_id: DeviceId,
        local_device_name: String,
        local_hostname: String,
        mut connection_manager: ConnectionManager,
    ) -> Self {
        let config = NetworkManagerConfig::default();
        let (event_tx, event_rx) = mpsc::channel(100);
        let authenticated_peer_rx = connection_manager
            .authenticated_peers()
            .expect("new connection manager must expose its authenticated peer receiver");

        let discovery = ServiceDiscovery::new(
            local_device_id,
            local_device_name.clone(),
            local_hostname.clone(),
        );

        let qos_registry = connection_manager.qos_registry();
        let connection_view = connection_manager.connection_view();
        let connection = Arc::new(TokioMutex::new(connection_manager));

        Self {
            local_device_id,
            local_device_name,
            local_hostname,
            config,
            discovery,
            connection,
            connection_view,
            qos_registry,
            event_tx,
            event_rx: Some(event_rx),
            authenticated_peer_rx: Some(authenticated_peer_rx),
            discovered_devices: Arc::new(RwLock::new(HashMap::new())),
            running: false,
            discovery_task: None,
        }
    }

    /// Set the configuration
    pub fn with_config(mut self, config: NetworkManagerConfig) -> Self {
        self.config = config;
        self
    }

    /// Get the event receiver
    pub fn events(&mut self) -> mpsc::Receiver<NetworkEvent> {
        self.event_rx.take().expect("Event receiver already taken")
    }

    pub fn receivers(&mut self) -> NetworkReceivers {
        NetworkReceivers {
            authenticated_peers: self
                .authenticated_peer_rx
                .take()
                .expect("Authenticated peer receiver already taken"),
            events: self.event_rx.take().expect("Event receiver already taken"),
        }
    }

    /// Shared generation-aware registry used by the daemon's input actor.
    pub fn input_registry(&self) -> Arc<ConnectionRegistry> {
        self.qos_registry.clone()
    }

    /// Get all discovered devices
    pub async fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        self.discovered_devices
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    /// Get connected devices
    pub async fn connected_devices(&self) -> Vec<DeviceId> {
        self.connection_infos()
            .await
            .into_iter()
            .filter(|info| info.state == crate::connection::ConnectionState::Connected)
            .map(|info| info.device_id)
            .collect()
    }

    /// Get current connection information snapshots.
    pub async fn connection_infos(&self) -> Vec<ConnectionInfo> {
        self.connection_view.connection_infos().await
    }

    /// Takes the typed terminal-release stream used by the input-plane
    /// integration. It is intentionally separate from legacy `Message`.
    pub async fn terminal_release_events(&self) -> Option<mpsc::Receiver<TerminalReleaseEvent>> {
        self.connection.lock().await.terminal_release_events()
    }

    /// Check if a device is connected
    pub async fn is_connected(&self, device_id: &DeviceId) -> bool {
        self.connection_view.is_connected(device_id).await
    }

    /// Send a message to a device
    pub async fn send_to(&mut self, device_id: &DeviceId, message: Message) -> Result<()> {
        if let Some(peer) = self.qos_registry.peer(device_id) {
            let generation = peer.auth.control_connection_id;
            match ClassifiedMessage::try_from(message.clone())
                .map_err(|error| anyhow::anyhow!(error))?
            {
                ClassifiedMessage::Control(frame) => {
                    peer.transport.send_control(frame).await?;
                    self.connection_view
                        .record_send_success(device_id, generation);
                    return Ok(());
                }
                ClassifiedMessage::Bulk(frame) => {
                    peer.transport.send_bulk(frame).await?;
                    self.connection_view
                        .record_send_success(device_id, generation);
                    return Ok(());
                }
                ClassifiedMessage::Telemetry(frame) => {
                    peer.transport.try_send_telemetry(frame)?;
                    self.connection_view
                        .record_send_success(device_id, generation);
                    return Ok(());
                }
                ClassifiedMessage::Unsupported => {}
            }
        }
        self.connection_view.send_legacy(device_id, &message).await
    }

    /// Broadcast a message to all connected devices
    pub async fn broadcast(&mut self, message: Message) -> Result<()> {
        match ClassifiedMessage::try_from(message.clone())
            .map_err(|error| anyhow::anyhow!(error))?
        {
            ClassifiedMessage::Control(frame) if !self.qos_registry.is_empty() => {
                let results = self
                    .qos_registry
                    .broadcast_control_with_generation(frame)
                    .await;
                record_qos_broadcast_successes(&self.connection_view, &results);
                if let Some((id, error)) = results
                    .into_iter()
                    .find_map(|(id, _, result)| result.err().map(|error| (id, error)))
                {
                    anyhow::bail!("QoS broadcast to {id} failed: {error}");
                }
                return Ok(());
            }
            ClassifiedMessage::Bulk(frame) if !self.qos_registry.is_empty() => {
                let results = self
                    .qos_registry
                    .broadcast_bulk_with_generation(frame)
                    .await;
                record_qos_broadcast_successes(&self.connection_view, &results);
                if let Some((id, error)) = results
                    .into_iter()
                    .find_map(|(id, _, result)| result.err().map(|error| (id, error)))
                {
                    anyhow::bail!("QoS broadcast to {id} failed: {error}");
                }
                return Ok(());
            }
            ClassifiedMessage::Telemetry(frame) if !self.qos_registry.is_empty() => {
                let results = self.qos_registry.broadcast_telemetry_with_generation(frame);
                record_qos_broadcast_successes(&self.connection_view, &results);
                if let Some((id, error)) = results
                    .into_iter()
                    .find_map(|(id, _, result)| result.err().map(|error| (id, error)))
                {
                    anyhow::bail!("QoS broadcast to {id} failed: {error}");
                }
                return Ok(());
            }
            _ => {}
        }
        self.connection_view.broadcast_legacy(&message).await
    }

    /// Start the network manager
    pub async fn start(&mut self) -> Result<()> {
        if self.running {
            return Ok(());
        }

        self.running = true;

        // Start connection manager (server)
        let connection_events = {
            let mut conn = self.connection.lock().await;
            conn.start_server(&self.config.bind_address).await?;
            conn.events()
        };

        if let Some(connection_events) = connection_events {
            spawn_connection_event_forwarder(connection_events, self.event_tx.clone());
        }

        // Start discovery with event channel
        let discovery_tx = self.event_tx.clone();
        let discovered_devices = self.discovered_devices.clone();
        let discovery_lost_connection = self.connection_view.clone();
        let discovery_event_config = self.config.clone();

        let mut discovery = ServiceDiscovery::new(
            self.local_device_id,
            self.local_device_name.clone(),
            self.local_hostname.clone(),
        );

        let discovery_config = crate::discovery::DiscoveryConfig {
            port: self.config.discovery_port,
            initial_broadcast_interval: Duration::from_millis(500),
            broadcast_interval: self.config.broadcast_interval,
            initial_broadcast_count: 6,
            device_timeout: self.config.device_timeout,
            mdns_enabled: false,
        };

        discovery = discovery.with_config(discovery_config);

        // Spawn discovery and consume its events independently. ServiceDiscovery::start
        // is the long-running receive loop, so awaiting it before reading rx would
        // prevent DeviceFound/DeviceUpdated from ever reaching NetworkManager.
        self.discovery_task = Some(tokio::spawn(async move {
            let (tx, mut rx) = mpsc::channel(100);
            let discovery_task = tokio::spawn(async move {
                if let Err(e) = discovery.start_with_channel(tx).await {
                    tracing::error!("Discovery failed to start: {}", e);
                }
            });

            while let Some(event) = rx.recv().await {
                handle_discovery_event(
                    event,
                    &discovery_event_config,
                    &discovered_devices,
                    &discovery_tx,
                    &discovery_lost_connection,
                )
                .await;
            }

            discovery_task.abort();
        }));

        tracing::info!("Network manager started");
        Ok(())
    }

    /// Stop the network manager
    pub async fn stop(&mut self) -> Result<()> {
        if !self.running {
            return Ok(());
        }

        self.running = false;
        if let Err(error) = ServiceDiscovery::broadcast_goodbye(
            self.local_device_id,
            self.config.discovery_port,
            "service stopped",
        )
        .await
        {
            tracing::warn!("Failed to broadcast Goodbye during network stop: {}", error);
        }
        if let Some(task) = self.discovery_task.take() {
            task.abort();
            let _ = task.await;
        }
        self.discovery.stop().await?;
        tracing::info!("Network manager stopped");
        Ok(())
    }

    /// Connect to a specific device
    pub async fn connect_to(&mut self, device_id: DeviceId, address: &str) -> Result<()> {
        if let Some(device) = self.discovered_devices.read().await.get(&device_id) {
            if let PeerProtocolCompatibility::Incompatible { local, remote } =
                device.protocol_compatibility
            {
                anyhow::bail!(
                    "Peer protocol is incompatible: local version {}, remote version {}",
                    local,
                    remote
                );
            }
        }
        let address = normalize_discovered_connection_address(
            address,
            self.config.discovery_port,
            connection_port(&self.config.bind_address),
        );
        let mut conn = self.connection.lock().await;
        conn.connect(device_id, &address).await
    }

    /// Disconnect from a device
    pub async fn disconnect_from(&mut self, device_id: &DeviceId) -> Result<()> {
        let mut conn = self.connection.lock().await;
        conn.disconnect(device_id).await
    }
}

fn connection_port(bind_address: &str) -> Option<u16> {
    bind_address
        .parse::<SocketAddr>()
        .ok()
        .map(|address| address.port())
}

fn normalize_discovered_connection_address(
    address: &str,
    discovery_port: u16,
    connection_port: Option<u16>,
) -> String {
    let Some(connection_port) = connection_port else {
        return address.to_string();
    };
    let Ok(mut socket_addr) = address.parse::<SocketAddr>() else {
        return address.to_string();
    };
    if socket_addr.port() == discovery_port {
        socket_addr.set_port(connection_port);
    }
    socket_addr.to_string()
}

fn discovery_lost_network_event(
    _device_id: DeviceId,
    _transport_connected: bool,
) -> Option<NetworkEvent> {
    // Discovery expiry is not an authenticated transport generation and must
    // not synthesize a generation-less disconnect event.
    None
}

// Note: NetworkManager intentionally doesn't implement Clone
// because it contains runtime resources like channels and connections

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::QuicTransport;

    fn discovered_device(device_id: DeviceId, address: SocketAddr, name: &str) -> DiscoveredDevice {
        DiscoveredDevice {
            id: device_id,
            name: name.to_string(),
            hostname: "remote-host".to_string(),
            addresses: vec![address],
            screen_info: None,
            capabilities: rshare_core::DeviceCapabilities::default(),
            transport_capabilities: rshare_core::PeerTransportCapabilities::required_v3(),
            protocol_compatibility: PeerProtocolCompatibility::Compatible,
            last_seen: tokio::time::Instant::now(),
        }
    }

    async fn connected_network_manager_for_fallback_test(
    ) -> (NetworkManager, ConnectionManager, DeviceId, String) {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut remote = ConnectionManager::with_transport(
            remote_id,
            QuicTransport::isolated_for_test(remote_id),
        );
        remote.start_server("127.0.0.1:0").await.unwrap();
        let address = remote.transport_local_addr().unwrap().to_string();
        let local_connection =
            ConnectionManager::with_transport(local_id, QuicTransport::isolated_for_test(local_id));
        let mut manager = NetworkManager::isolated_for_test(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
        );
        manager.qos_registry = local_connection.qos_registry();
        manager.connection_view = local_connection.connection_view();
        manager.connection = Arc::new(TokioMutex::new(local_connection));
        manager.connect_to(remote_id, &address).await.unwrap();
        (manager, remote, remote_id, address)
    }

    async fn run_network_fallback_replacement_race(
        old_control_id: Option<ControlConnectionId>,
        replacement_control_id: Option<ControlConnectionId>,
        completion: std::result::Result<(), String>,
    ) -> (Result<()>, u64) {
        let (mut manager, _remote, remote_id, _address) =
            connected_network_manager_for_fallback_test().await;
        let view = manager.connection_view.clone();
        let old_generation = view.pool_generation_for_test(&remote_id).await;
        view.replace_canonical_identity_for_test(remote_id, old_generation, old_control_id);
        let (blocked_tx, mut blocked_rx) = mpsc::channel(1);
        view.replace_pool_generation_and_outbound_for_test(
            remote_id,
            old_generation,
            old_control_id,
            blocked_tx,
        )
        .await;

        let send = tokio::spawn(async move {
            manager
                .send_to(
                    &remote_id,
                    Message::HelloRejected {
                        app_id: rshare_core::DISCOVERY_APP_ID.into(),
                        device_id: remote_id,
                        reason: rshare_core::HandshakeRejectReason::IdentityUnavailable,
                    },
                )
                .await
        });
        let old_frame = tokio::time::timeout(Duration::from_secs(1), blocked_rx.recv())
            .await
            .expect("NetworkManager fallback must select the delayed old sender")
            .expect("old delayed sender must remain connected");

        let replacement_generation = old_generation
            .checked_add(1)
            .expect("test lifecycle generation must advance");
        let (replacement_tx, _replacement_rx) = mpsc::channel(1);
        view.replace_pool_generation_and_outbound_for_test(
            remote_id,
            replacement_generation,
            replacement_control_id,
            replacement_tx,
        )
        .await;
        view.replace_canonical_identity_for_test(
            remote_id,
            replacement_generation,
            replacement_control_id,
        );
        old_frame.complete_for_test(completion);
        let result = send.await.unwrap();
        let messages_sent = view
            .connection_infos()
            .await
            .into_iter()
            .find(|info| info.device_id == remote_id)
            .expect("replacement must remain visible")
            .messages_sent;
        (result, messages_sent)
    }

    #[test]
    fn test_network_manager_config_default() {
        let config = NetworkManagerConfig::default();
        assert_eq!(config.discovery_port, 27432);
        assert!(!config.auto_connect);
    }

    #[test]
    fn normalizes_discovery_source_port_to_connection_port() {
        assert_eq!(
            normalize_discovered_connection_address("192.168.1.241:27432", 27432, Some(27431)),
            "192.168.1.241:27431"
        );
        assert_eq!(
            normalize_discovered_connection_address("192.168.1.241:27431", 27432, Some(27431)),
            "192.168.1.241:27431"
        );
    }

    #[test]
    fn discovery_lost_does_not_emit_disconnect_while_transport_is_connected() {
        let device_id = DeviceId::new_v4();

        assert!(discovery_lost_network_event(device_id, true).is_none());
    }

    #[test]
    fn discovery_lost_does_not_synthesize_generationless_disconnect() {
        let device_id = DeviceId::new_v4();

        assert!(discovery_lost_network_event(device_id, false).is_none());
    }

    #[test]
    fn test_network_manager_new() {
        let manager = NetworkManager::isolated_for_test(
            DeviceId::new_v4(),
            "Test".to_string(),
            "test-host".to_string(),
        );
        assert!(!manager.running);
    }

    #[tokio::test]
    async fn status_query_does_not_wait_for_outer_connection_manager_lock() {
        let manager = NetworkManager::isolated_for_test(
            DeviceId::new_v4(),
            "Test".to_string(),
            "test-host".to_string(),
        );
        let _held = manager.connection.lock().await;

        tokio::time::timeout(Duration::from_millis(50), manager.connection_infos())
            .await
            .expect("status query must bypass the lifecycle manager lock");
    }

    #[tokio::test]
    async fn message_send_does_not_wait_for_outer_connection_manager_lock() {
        let mut manager = NetworkManager::isolated_for_test(
            DeviceId::new_v4(),
            "Test".to_string(),
            "test-host".to_string(),
        );
        let connection = manager.connection.clone();
        let _held = connection.lock().await;

        let result = tokio::time::timeout(
            Duration::from_millis(50),
            manager.send_to(
                &DeviceId::new_v4(),
                Message::Heartbeat {
                    sequence: 10,
                    timestamp: 20,
                },
            ),
        )
        .await
        .expect("message send must bypass the lifecycle manager lock");
        assert!(
            result.is_err(),
            "missing peer must still report send failure"
        );
    }

    #[tokio::test]
    async fn fallback_old_sender_with_absent_control_ids_cannot_increment_replacement_metrics() {
        let (result, messages_sent) =
            run_network_fallback_replacement_race(None, None, Ok(())).await;

        result.expect("the already selected old fallback sender completes successfully");
        assert_eq!(
            messages_sent, 0,
            "NetworkManager must match the selected lifecycle generation even when both control IDs are absent"
        );
    }

    #[tokio::test]
    async fn fallback_old_sender_with_control_ids_cannot_increment_replacement_metrics() {
        let (result, messages_sent) = run_network_fallback_replacement_race(
            Some(ControlConnectionId::new()),
            Some(ControlConnectionId::new()),
            Ok(()),
        )
        .await;

        result.expect("the already selected old fallback sender completes successfully");
        assert_eq!(messages_sent, 0);
    }

    #[tokio::test]
    async fn failed_fallback_old_sender_never_increments_replacement_metrics() {
        let (result, messages_sent) =
            run_network_fallback_replacement_race(None, None, Err("injected failure".into())).await;

        assert!(result.is_err());
        assert_eq!(messages_sent, 0);
    }

    #[tokio::test]
    async fn qos_direct_send_and_broadcast_update_connection_metrics() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut remote = ConnectionManager::with_transport(
            remote_id,
            crate::transport::QuicTransport::isolated_for_test(remote_id),
        );
        remote.start_server("127.0.0.1:0").await.unwrap();
        let address = remote.transport_local_addr().unwrap();

        let local_connection = ConnectionManager::with_transport(
            local_id,
            crate::transport::QuicTransport::isolated_for_test(local_id),
        );
        let mut manager = NetworkManager::isolated_for_test(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
        );
        manager.qos_registry = local_connection.qos_registry();
        manager.connection_view = local_connection.connection_view();
        manager.connection = Arc::new(TokioMutex::new(local_connection));
        manager
            .connect_to(remote_id, &address.to_string())
            .await
            .unwrap();

        let snapshot = |infos: Vec<ConnectionInfo>| {
            infos
                .into_iter()
                .find(|info| info.device_id == remote_id)
                .unwrap()
        };
        let before = snapshot(manager.connection_infos().await);
        manager
            .send_to(
                &remote_id,
                Message::Heartbeat {
                    sequence: 1,
                    timestamp: 1,
                },
            )
            .await
            .unwrap();
        let after_send = snapshot(manager.connection_infos().await);
        assert_eq!(after_send.messages_sent, before.messages_sent + 1);
        assert!(after_send.last_activity >= before.last_activity);

        manager
            .broadcast(Message::Heartbeat {
                sequence: 2,
                timestamp: 2,
            })
            .await
            .unwrap();
        let after_broadcast = snapshot(manager.connection_infos().await);
        assert_eq!(after_broadcast.messages_sent, after_send.messages_sent + 1);
        assert!(after_broadcast.last_activity >= after_send.last_activity);
    }

    #[tokio::test]
    async fn known_incompatible_discovery_fails_before_quic_connect() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut manager = NetworkManager::isolated_for_test(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
        );
        let mut incompatible = discovered_device(remote_id, "127.0.0.1:1".parse().unwrap(), "old");
        incompatible.protocol_compatibility = PeerProtocolCompatibility::Incompatible {
            local: rshare_core::PROTOCOL_VERSION,
            remote: 2,
        };
        manager
            .discovered_devices
            .write()
            .await
            .insert(remote_id, incompatible);

        let error = manager
            .connect_to(remote_id, "not-even-a-socket-address")
            .await
            .expect_err("known incompatible peer must fail before address or QUIC work");
        assert!(error.to_string().contains("remote version 2"));
        assert!(manager.connection_infos().await.is_empty());
    }

    #[tokio::test]
    async fn legacy_auto_connect_true_keeps_found_and_updated_devices_observational() {
        let local_id = DeviceId::from_bytes([0x10; 16]);
        let remote_id = DeviceId::from_bytes([0xf0; 16]);
        let probe = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = NetworkManagerConfig {
            discovery_port: 0,
            auto_connect: true,
            ..NetworkManagerConfig::default()
        };
        let mut manager = NetworkManager::isolated_for_test(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
        )
        .with_config(config);
        let mut events = manager.events();

        let found = discovered_device(remote_id, probe.local_addr().unwrap(), "first");
        handle_discovery_event(
            crate::discovery::DiscoveryEvent::DeviceFound(found.clone()),
            &manager.config,
            &manager.discovered_devices,
            &manager.event_tx,
            &manager.connection_view,
        )
        .await;
        let mut updated = found;
        updated.name = "updated".to_string();
        handle_discovery_event(
            crate::discovery::DiscoveryEvent::DeviceUpdated(updated.clone()),
            &manager.config,
            &manager.discovered_devices,
            &manager.event_tx,
            &manager.connection_view,
        )
        .await;

        for expected_name in ["first", "updated"] {
            let event = events.recv().await.unwrap();
            assert!(matches!(
                event,
                NetworkEvent::DeviceFound(device) if device.name == expected_name
            ));
        }
        assert_eq!(
            manager
                .discovered_devices()
                .await
                .into_iter()
                .find(|device| device.id == remote_id)
                .unwrap()
                .name,
            "updated"
        );
        let mut packet = [0u8; 2048];
        assert!(
            tokio::time::timeout(Duration::from_millis(300), probe.recv_from(&mut packet))
                .await
                .is_err(),
            "discovery must not send a QUIC connection attempt"
        );
        assert!(manager.connection_infos().await.is_empty());
    }

    #[tokio::test]
    async fn forwards_typed_control_and_protocol_errors_with_authenticated_generation() {
        let device_id = DeviceId::new_v4();
        let auth = Arc::new(crate::handshake::PeerAuthContext {
            peer_id: device_id,
            certificate_fingerprint: crate::encryption::PeerCertificateFingerprint::from_der(
                b"peer",
            ),
            control_connection_id: ControlConnectionId::new(),
        });
        let (manager_tx, manager_rx) = mpsc::channel(4);
        let (network_tx, mut network_rx) = mpsc::channel(4);

        spawn_connection_event_forwarder(manager_rx, network_tx);
        manager_tx
            .send(crate::connection::ManagerEvent::ControlReceived {
                auth: auth.clone(),
                frame: crate::qos::ControlFrame::heartbeat(1, 2),
            })
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(1), network_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match event {
            NetworkEvent::ControlReceived {
                auth: received,
                frame,
            } => {
                assert_eq!(received.control_connection_id, auth.control_connection_id);
                assert!(matches!(
                    frame.into_message(),
                    Message::Heartbeat {
                        sequence: 1,
                        timestamp: 2
                    }
                ));
            }
            _ => panic!("Wrong network event"),
        }

        manager_tx
            .send(crate::connection::ManagerEvent::ProtocolError {
                auth: auth.clone(),
                error: "unknown qos lane discriminator 255".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), network_rx.recv())
                .await
                .unwrap()
                .unwrap(),
            NetworkEvent::ConnectionError {
                peer_id: Some(id),
                control_connection_id: Some(generation),
                error,
            } if id == device_id
                && generation == auth.control_connection_id
                && error.contains("unknown qos lane")
        ));
    }

    #[test]
    fn receivers_are_taken_as_one_typed_set() {
        let mut manager =
            NetworkManager::isolated_for_test(DeviceId::new_v4(), "local".into(), "host".into());
        let NetworkReceivers {
            authenticated_peers,
            events,
        } = manager.receivers();
        assert!(!authenticated_peers.is_closed());
        assert!(!events.is_closed());
    }

    #[tokio::test]
    async fn public_authenticated_peer_capacity_fails_closed_without_hidden_backlog() {
        let server_id = DeviceId::new_v4();
        let mut manager =
            NetworkManager::isolated_for_test(server_id, "server".into(), "server-host".into());
        manager.config.bind_address = "127.0.0.1:0".into();
        manager.config.discovery_port = 0;
        let NetworkReceivers {
            mut authenticated_peers,
            mut events,
        } = manager.receivers();
        manager.start().await.unwrap();
        let address = manager
            .connection
            .lock()
            .await
            .transport_local_addr()
            .unwrap()
            .to_string();

        let mut retained_clients = Vec::new();
        let mut retained_generations = HashMap::new();
        for _ in 0..32 {
            let client_id = DeviceId::new_v4();
            let mut client = ConnectionManager::isolated_for_test(client_id);
            client.connect(server_id, &address).await.unwrap();
            let connected = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    match events.recv().await {
                        Some(NetworkEvent::DeviceConnected(auth)) if auth.peer_id == client_id => {
                            break auth;
                        }
                        Some(NetworkEvent::ConnectionError {
                            peer_id: Some(peer_id),
                            error,
                            ..
                        }) if peer_id == client_id => {
                            panic!("retained peer {client_id} failed before publication: {error}");
                        }
                        Some(_) => {}
                        None => panic!("network event channel closed before peer publication"),
                    }
                }
            })
            .await
            .expect("each retained peer must publish its generation-aware connected event");
            assert!(retained_generations
                .insert(client_id, connected.control_connection_id)
                .is_none());
            retained_clients.push(client);
        }
        assert_eq!(
            authenticated_peers.len(),
            32,
            "every connected publication must already have reserved and filled its public peer slot"
        );

        let overflow_id = DeviceId::new_v4();
        let mut overflow_client = ConnectionManager::isolated_for_test(overflow_id);
        overflow_client.connect(server_id, &address).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(NetworkEvent::ConnectionError {
                    peer_id: Some(peer_id),
                    control_connection_id: Some(control_connection_id),
                    error,
                }) = events.recv().await
                {
                    if peer_id == overflow_id {
                        break (control_connection_id, error);
                    }
                }
            }
        })
        .await
        .expect("overflow rejection must publish a generation-aware error");
        assert!(error.1.contains("queue is full"));

        assert!(!manager.is_connected(&overflow_id).await);
        assert!(
            manager
                .connection
                .lock()
                .await
                .qos_registry()
                .peer(&overflow_id)
                .is_none(),
            "overflow generation must fail before registry publication"
        );
        assert_eq!(authenticated_peers.len(), 32);
        let mut published_generations = HashMap::new();
        while let Ok(peer) = authenticated_peers.try_recv() {
            assert!(
                published_generations
                    .insert(peer.auth.peer_id, peer.auth.control_connection_id)
                    .is_none(),
                "one public entry is allowed per retained peer generation"
            );
        }
        assert_eq!(published_generations.len(), 32);
        assert!(!published_generations.contains_key(&overflow_id));
        for (retained_id, generation) in retained_generations {
            assert_eq!(published_generations.get(&retained_id), Some(&generation));
            assert!(manager.is_connected(&retained_id).await);
        }
        drop(retained_clients);
    }
}
