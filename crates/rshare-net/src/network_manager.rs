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
    discovery::{DiscoveredDevice, ServiceDiscovery},
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
    /// Auto-connect to discovered devices
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
            auto_connect: true,
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

        let connection = Arc::new(TokioMutex::new(ConnectionManager::new(local_device_id)));

        Self {
            local_device_id,
            local_device_name,
            local_hostname,
            config,
            discovery,
            connection,
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

    /// Check if a device is connected
    pub async fn is_connected(&self, device_id: &DeviceId) -> bool {
        let conn = self.connection.lock().await;
        conn.is_connected(device_id).await
    }

    /// Send a message to a device
    pub async fn send_to(&mut self, device_id: &DeviceId, message: Message) -> Result<()> {
        let mut conn = self.connection.lock().await;
        conn.send_to(device_id, message).await
    }

    /// Broadcast a message to all connected devices
    pub async fn broadcast(&mut self, message: Message) -> Result<()> {
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
        let auto_connect_config = self.config.clone();
        let auto_connect_connection = self.connection.clone();
        let discovery_lost_connection = self.connection.clone();
        let local_device_id = self.local_device_id;

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
                match event {
                    crate::discovery::DiscoveryEvent::DeviceFound(device) => {
                        let device_id = device.id;

                        {
                            let mut devices = discovered_devices.write().await;
                            devices.insert(device_id, device.clone());
                        }

                        let _ = discovery_tx.try_send(NetworkEvent::DeviceFound(device.clone()));
                        spawn_auto_connect_discovered_device(
                            auto_connect_connection.clone(),
                            auto_connect_config.clone(),
                            local_device_id,
                            device,
                        );
                    }
                    crate::discovery::DiscoveryEvent::DeviceUpdated(device) => {
                        let device_id = device.id;

                        {
                            let mut devices = discovered_devices.write().await;
                            devices.insert(device_id, device.clone());
                        }

                        let _ = discovery_tx.try_send(NetworkEvent::DeviceFound(device.clone()));
                        spawn_auto_connect_discovered_device(
                            auto_connect_connection.clone(),
                            auto_connect_config.clone(),
                            local_device_id,
                            device,
                        );
                    }
                    crate::discovery::DiscoveryEvent::DeviceLost(id) => {
                        {
                            let mut devices = discovered_devices.write().await;
                            devices.remove(&id);
                        }

                        let transport_connected = {
                            let manager = discovery_lost_connection.lock().await;
                            manager.is_connected(&id).await
                        };
                        if let Some(event) = discovery_lost_network_event(id, transport_connected) {
                            let _ = discovery_tx.try_send(event);
                        }
                    }
                    crate::discovery::DiscoveryEvent::Error(err) => {
                        tracing::error!("Discovery error: {}", err);
                    }
                }
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

fn auto_connect_address_for_device(
    config: &NetworkManagerConfig,
    local_device_id: DeviceId,
    device: &DiscoveredDevice,
) -> Option<String> {
    if !config.auto_connect {
        return None;
    }
    if local_device_id > device.id {
        return None;
    }

    let address = device.addresses.first()?;
    Some(normalize_discovered_connection_address(
        &address.to_string(),
        config.discovery_port,
        connection_port(&config.bind_address),
    ))
}

fn spawn_auto_connect_discovered_device(
    connection: Arc<TokioMutex<ConnectionManager>>,
    config: NetworkManagerConfig,
    local_device_id: DeviceId,
    device: DiscoveredDevice,
) {
    let Some(address) = auto_connect_address_for_device(&config, local_device_id, &device) else {
        return;
    };

    tokio::spawn(async move {
        let already_connecting_or_connected = {
            let manager = connection.lock().await;
            manager.connection_infos().await.into_iter().any(|info| {
                info.device_id == device.id
                    && matches!(
                        info.state,
                        crate::connection::ConnectionState::Connecting
                            | crate::connection::ConnectionState::Connected
                    )
            })
        };
        if already_connecting_or_connected {
            return;
        }

        let result = {
            let mut manager = connection.lock().await;
            manager.connect(device.id, &address).await
        };
        if let Err(error) = result {
            tracing::debug!(
                "Auto-connect to discovered device {} at {} failed: {}",
                device.id,
                address,
                error
            );
        }
    });
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

    #[test]
    fn test_network_manager_config_default() {
        let config = NetworkManagerConfig::default();
        assert_eq!(config.discovery_port, 27432);
        assert!(config.auto_connect);
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

    fn deterministic_device_id(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 16])
    }

    fn discovered_device_with_id_and_address(
        device_id: DeviceId,
        address: &str,
    ) -> DiscoveredDevice {
        DiscoveredDevice {
            id: device_id,
            name: "remote".to_string(),
            hostname: "remote-host".to_string(),
            addresses: vec![address.parse().unwrap()],
            screen_info: None,
            capabilities: rshare_core::DeviceCapabilities::default(),
            last_seen: tokio::time::Instant::now(),
        }
    }

    fn discovered_device_with_address(address: &str) -> DiscoveredDevice {
        discovered_device_with_id_and_address(DeviceId::new_v4(), address)
    }

    #[test]
    fn auto_connect_address_uses_discovered_connection_address_when_enabled() {
        let config = NetworkManagerConfig::default();
        let device = discovered_device_with_id_and_address(
            deterministic_device_id(0xF0),
            "192.168.1.241:27432",
        );

        assert_eq!(
            auto_connect_address_for_device(&config, deterministic_device_id(0x10), &device),
            Some("192.168.1.241:27431".to_string())
        );
    }

    #[test]
    fn auto_connect_address_is_none_when_local_device_wins_tie_breaker() {
        let config = NetworkManagerConfig::default();
        let local_id = deterministic_device_id(0xF0);
        let remote = discovered_device_with_id_and_address(
            deterministic_device_id(0x10),
            "192.168.1.241:27432",
        );

        assert_eq!(
            auto_connect_address_for_device(&config, local_id, &remote),
            None
        );
    }

    #[test]
    fn auto_connect_address_is_none_when_auto_connect_is_disabled() {
        let mut config = NetworkManagerConfig::default();
        config.auto_connect = false;
        let device = discovered_device_with_address("192.168.1.241:27432");

        assert_eq!(
            auto_connect_address_for_device(&config, deterministic_device_id(0x10), &device),
            None
        );
    }

    #[test]
    fn auto_connect_address_is_none_without_discovered_addresses() {
        let config = NetworkManagerConfig::default();
        let mut device = discovered_device_with_address("192.168.1.241:27432");
        device.addresses.clear();

        assert_eq!(
            auto_connect_address_for_device(&config, deterministic_device_id(0x10), &device),
            None
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
