//! Network manager - unified discovery and connection management

use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as TokioMutex, RwLock};
use tokio::task::JoinHandle;

use crate::{
    connection::{ConnectionInfo, ConnectionManager, ManagerEvent},
    discovery::{DiscoveredDevice, DiscoveryEvent, PeerProtocolCompatibility, ServiceDiscovery},
    qos::{ClassifiedMessage, ConnectionRegistry, TerminalReleaseEvent},
};
use rshare_core::{DeviceId, Message};

/// Network event
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    /// Device discovered
    DeviceFound(DiscoveredDevice),
    /// Device connected
    DeviceConnected(DeviceId),
    /// Device disconnected
    DeviceDisconnected(DeviceId),
    /// Message received from device
    MessageReceived { from: DeviceId, message: Message },
    /// Connection error
    ConnectionError { device_id: DeviceId, error: String },
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
    qos_registry: Arc<ConnectionRegistry>,

    event_tx: mpsc::Sender<NetworkEvent>,
    event_rx: Option<mpsc::Receiver<NetworkEvent>>,

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
                ManagerEvent::Connected(device_id) => NetworkEvent::DeviceConnected(device_id),
                ManagerEvent::Disconnected(device_id) => {
                    NetworkEvent::DeviceDisconnected(device_id)
                }
                ManagerEvent::MessageReceived { from, message } => {
                    NetworkEvent::MessageReceived { from, message }
                }
                ManagerEvent::Error { device_id, error } => {
                    NetworkEvent::ConnectionError { device_id, error }
                }
            };

            if network_tx.send(network_event).await.is_err() {
                break;
            }
        }
    });
}

async fn handle_discovery_event(
    event: DiscoveryEvent,
    config: &NetworkManagerConfig,
    discovered_devices: &Arc<RwLock<HashMap<DeviceId, DiscoveredDevice>>>,
    discovery_tx: &mpsc::Sender<NetworkEvent>,
    connection: &Arc<TokioMutex<ConnectionManager>>,
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

            let transport_connected = {
                let manager = connection.lock().await;
                manager.is_connected(&id).await
            };
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
        let config = NetworkManagerConfig::default();
        let (event_tx, event_rx) = mpsc::channel(100);

        let discovery = ServiceDiscovery::new(
            local_device_id,
            local_device_name.clone(),
            local_hostname.clone(),
        );

        let connection_manager = ConnectionManager::new(local_device_id);
        let qos_registry = connection_manager.qos_registry();
        let connection = Arc::new(TokioMutex::new(connection_manager));

        Self {
            local_device_id,
            local_device_name,
            local_hostname,
            config,
            discovery,
            connection,
            qos_registry,
            event_tx,
            event_rx: Some(event_rx),
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
        let conn = self.connection.lock().await;
        conn.connection_infos().await
    }

    /// Takes the typed terminal-release stream used by the input-plane
    /// integration. It is intentionally separate from legacy `Message`.
    pub async fn terminal_release_events(&self) -> Option<mpsc::Receiver<TerminalReleaseEvent>> {
        self.connection.lock().await.terminal_release_events()
    }

    /// Check if a device is connected
    pub async fn is_connected(&self, device_id: &DeviceId) -> bool {
        let conn = self.connection.lock().await;
        conn.is_connected(device_id).await
    }

    /// Send a message to a device
    pub async fn send_to(&mut self, device_id: &DeviceId, message: Message) -> Result<()> {
        if let Some(peer) = self.qos_registry.peer(device_id) {
            match ClassifiedMessage::try_from(message.clone())
                .map_err(|error| anyhow::anyhow!(error))?
            {
                ClassifiedMessage::Control(frame) => {
                    return peer.transport.send_control(frame).await.map_err(Into::into);
                }
                ClassifiedMessage::Bulk(frame) => {
                    return peer.transport.send_bulk(frame).await.map_err(Into::into);
                }
                ClassifiedMessage::Telemetry(frame) => {
                    return peer.transport.try_send_telemetry(frame).map_err(Into::into);
                }
                ClassifiedMessage::Unsupported => {}
            }
        }
        let mut conn = self.connection.lock().await;
        conn.send_to(device_id, message).await
    }

    /// Broadcast a message to all connected devices
    pub async fn broadcast(&mut self, message: Message) -> Result<()> {
        match ClassifiedMessage::try_from(message.clone())
            .map_err(|error| anyhow::anyhow!(error))?
        {
            ClassifiedMessage::Control(frame) if !self.qos_registry.is_empty() => {
                let results = self.qos_registry.broadcast_control(frame).await;
                if let Some((id, error)) = results
                    .into_iter()
                    .find_map(|(id, result)| result.err().map(|error| (id, error)))
                {
                    anyhow::bail!("QoS broadcast to {id} failed: {error}");
                }
                return Ok(());
            }
            ClassifiedMessage::Bulk(frame) if !self.qos_registry.is_empty() => {
                let results = self.qos_registry.broadcast_bulk(frame).await;
                if let Some((id, error)) = results
                    .into_iter()
                    .find_map(|(id, result)| result.err().map(|error| (id, error)))
                {
                    anyhow::bail!("QoS broadcast to {id} failed: {error}");
                }
                return Ok(());
            }
            ClassifiedMessage::Telemetry(frame) if !self.qos_registry.is_empty() => {
                if let Some((id, error)) = self
                    .qos_registry
                    .broadcast_telemetry(frame)
                    .into_iter()
                    .find_map(|(id, result)| result.err().map(|error| (id, error)))
                {
                    anyhow::bail!("QoS broadcast to {id} failed: {error}");
                }
                return Ok(());
            }
            _ => {}
        }
        let mut conn = self.connection.lock().await;
        conn.broadcast(message).await
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
        let discovery_lost_connection = self.connection.clone();
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
    device_id: DeviceId,
    transport_connected: bool,
) -> Option<NetworkEvent> {
    if transport_connected {
        None
    } else {
        Some(NetworkEvent::DeviceDisconnected(device_id))
    }
}

// Note: NetworkManager intentionally doesn't implement Clone
// because it contains runtime resources like channels and connections

#[cfg(test)]
mod tests {
    use super::*;

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
    fn discovery_lost_emits_disconnect_when_transport_is_not_connected() {
        let device_id = DeviceId::new_v4();

        assert!(matches!(
            discovery_lost_network_event(device_id, false),
            Some(NetworkEvent::DeviceDisconnected(id)) if id == device_id
        ));
    }

    #[test]
    fn test_network_manager_new() {
        let manager = NetworkManager::new(
            DeviceId::new_v4(),
            "Test".to_string(),
            "test-host".to_string(),
        );
        assert!(!manager.running);
    }

    #[tokio::test]
    async fn known_incompatible_discovery_fails_before_quic_connect() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut manager =
            NetworkManager::new(local_id, "local".to_string(), "local-host".to_string());
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
        let mut manager =
            NetworkManager::new(local_id, "local".to_string(), "local-host".to_string())
                .with_config(config);
        let mut events = manager.events();

        let found = discovered_device(remote_id, probe.local_addr().unwrap(), "first");
        handle_discovery_event(
            crate::discovery::DiscoveryEvent::DeviceFound(found.clone()),
            &manager.config,
            &manager.discovered_devices,
            &manager.event_tx,
            &manager.connection,
        )
        .await;
        let mut updated = found;
        updated.name = "updated".to_string();
        handle_discovery_event(
            crate::discovery::DiscoveryEvent::DeviceUpdated(updated.clone()),
            &manager.config,
            &manager.discovered_devices,
            &manager.event_tx,
            &manager.connection,
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
    async fn forwards_connection_message_events_to_network_events() {
        let device_id = DeviceId::new_v4();
        let (manager_tx, manager_rx) = mpsc::channel(4);
        let (network_tx, mut network_rx) = mpsc::channel(4);

        spawn_connection_event_forwarder(manager_rx, network_tx);
        manager_tx
            .send(crate::connection::ManagerEvent::MessageReceived {
                from: device_id,
                message: Message::MouseMove { x: 1, y: 2 },
            })
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(1), network_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match event {
            NetworkEvent::MessageReceived { from, message } => {
                assert_eq!(from, device_id);
                assert!(matches!(message, Message::MouseMove { x: 1, y: 2 }));
            }
            _ => panic!("Wrong network event"),
        }
    }
}
