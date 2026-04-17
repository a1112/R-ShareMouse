//! Network manager - unified discovery and connection management

use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as TokioMutex, RwLock};
use tokio::task::JoinHandle;

use crate::{
    connection::{ConnectionManager, ManagerEvent},
    discovery::{
        spawn_discovery, DiscoveredDevice, DiscoveryConfig, DiscoveryEvent, DiscoveryTask,
        ServiceDiscovery,
    },
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

    connection: Arc<TokioMutex<ConnectionManager>>,

    event_tx: mpsc::Sender<NetworkEvent>,
    event_rx: Option<mpsc::Receiver<NetworkEvent>>,

    discovered_devices: Arc<RwLock<HashMap<DeviceId, DiscoveredDevice>>>,
    discovery_task: Option<DiscoveryTask>,
    discovery_events_task: Option<JoinHandle<()>>,
    connection_events_task: Option<JoinHandle<()>>,
    running: bool,
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

        let connection = Arc::new(TokioMutex::new(ConnectionManager::new(local_device_id)));

        Self {
            local_device_id,
            local_device_name,
            local_hostname,
            config,
            connection,
            event_tx,
            event_rx: Some(event_rx),
            discovered_devices: Arc::new(RwLock::new(HashMap::new())),
            discovery_task: None,
            discovery_events_task: None,
            connection_events_task: None,
            running: false,
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
        // For now, return empty list since ConnectionManager doesn't expose all connections
        // In a real implementation, ConnectionManager would have a connections() method
        Vec::new()
    }

    /// Check if a device is connected
    pub async fn is_connected(&self, device_id: &DeviceId) -> bool {
        let conn = self.connection.lock().await;
        conn.is_connected(device_id)
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
        {
            let mut conn = self.connection.lock().await;
            conn.start_server(&self.config.bind_address).await?;

            if let Some(mut manager_events) = conn.events() {
                let network_tx = self.event_tx.clone();
                self.connection_events_task = Some(tokio::spawn(async move {
                    while let Some(event) = manager_events.recv().await {
                        let network_event = match event {
                            ManagerEvent::Connected(device_id) => {
                                NetworkEvent::DeviceConnected(device_id)
                            }
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
                }));
            }
        }

        // Start discovery with event channel
        let discovery_tx = self.event_tx.clone();
        let discovered_devices = self.discovered_devices.clone();

        let mut discovery = ServiceDiscovery::new(
            self.local_device_id,
            self.local_device_name.clone(),
            self.local_hostname.clone(),
        );

        let discovery_config = DiscoveryConfig {
            port: self.config.discovery_port,
            broadcast_interval: self.config.broadcast_interval,
            device_timeout: self.config.device_timeout,
            mdns_enabled: false,
        };

        discovery = discovery.with_config(discovery_config);

        let (tx, mut rx) = mpsc::channel(100);
        self.discovery_task = Some(spawn_discovery(discovery, tx));

        let connection = self.connection.clone();
        let auto_connect = self.config.auto_connect;
        let transport_port = self.transport_port();

        self.discovery_events_task = Some(tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    DiscoveryEvent::DeviceFound(device) => {
                        let device_id = device.id;

                        {
                            let mut devices = discovered_devices.write().await;
                            devices.insert(device_id, device.clone());
                        }

                        let _ = discovery_tx
                            .send(NetworkEvent::DeviceFound(device.clone()))
                            .await;

                        if auto_connect {
                            if let Some(address) = transport_address_for(&device, transport_port) {
                                let connection = connection.clone();
                                tokio::spawn(async move {
                                    let mut conn = connection.lock().await;
                                    if !conn.is_connected(&device_id) {
                                        if let Err(err) = conn.connect(device_id, &address).await {
                                            tracing::debug!(
                                                "Auto-connect to {} at {} failed: {}",
                                                device_id,
                                                address,
                                                err
                                            );
                                        }
                                    }
                                });
                            }
                        }
                    }
                    DiscoveryEvent::DeviceUpdated(device) => {
                        let device_id = device.id;

                        {
                            let mut devices = discovered_devices.write().await;
                            devices.insert(device_id, device.clone());
                        }

                        let _ = discovery_tx.send(NetworkEvent::DeviceFound(device)).await;
                    }
                    DiscoveryEvent::DeviceLost(id) => {
                        {
                            let mut devices = discovered_devices.write().await;
                            devices.remove(&id);
                        }

                        let _ = discovery_tx
                            .send(NetworkEvent::DeviceDisconnected(id))
                            .await;
                    }
                    DiscoveryEvent::Error(err) => {
                        tracing::error!("Discovery error: {}", err);
                    }
                }
            }
        }));

        // Spawn connection event handler
        // Note: We need to handle this differently since ConnectionManager is inside Mutex
        // For now, we'll skip this part and handle events differently

        tracing::info!("Network manager started");
        Ok(())
    }

    /// Stop the network manager
    pub async fn stop(&mut self) -> Result<()> {
        if !self.running {
            return Ok(());
        }

        self.running = false;
        if let Some(task) = self.discovery_task.take() {
            task.stop().await;
        }
        if let Some(task) = self.discovery_events_task.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(task) = self.connection_events_task.take() {
            task.abort();
            let _ = task.await;
        }
        {
            let mut conn = self.connection.lock().await;
            let _ = conn.cleanup_stale(Duration::from_secs(0)).await;
        }
        tracing::info!("Network manager stopped");
        Ok(())
    }

    /// Connect to a previously discovered device using the configured transport port.
    pub async fn connect_to_discovered(&mut self, device_id: DeviceId) -> Result<()> {
        let device = {
            let devices = self.discovered_devices.read().await;
            devices
                .get(&device_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Device {} has not been discovered", device_id))?
        };

        let address = transport_address_for(&device, self.transport_port())
            .ok_or_else(|| anyhow::anyhow!("Device {} has no usable address", device_id))?;

        self.connect_to(device_id, &address).await
    }

    /// Connect to a specific device
    pub async fn connect_to(&mut self, device_id: DeviceId, address: &str) -> Result<()> {
        let mut conn = self.connection.lock().await;
        conn.connect(device_id, address).await
    }

    /// Disconnect from a device
    pub async fn disconnect_from(&mut self, device_id: &DeviceId) -> Result<()> {
        let mut conn = self.connection.lock().await;
        conn.disconnect(device_id).await
    }

    fn transport_port(&self) -> u16 {
        self.config
            .bind_address
            .parse::<SocketAddr>()
            .map(|addr| addr.port())
            .unwrap_or(27431)
    }
}

fn transport_address_for(device: &DiscoveredDevice, port: u16) -> Option<String> {
    device
        .addresses
        .first()
        .map(|addr| SocketAddr::new(addr.ip(), port).to_string())
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
        assert!(!config.auto_connect);
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

    #[test]
    fn maps_discovery_address_to_transport_port() {
        let device = DiscoveredDevice {
            id: DeviceId::new_v4(),
            name: "Test".to_string(),
            hostname: "test".to_string(),
            addresses: vec!["192.168.1.52:27432".parse().unwrap()],
            last_seen: tokio::time::Instant::now(),
        };

        assert_eq!(
            transport_address_for(&device, 27431).unwrap(),
            "192.168.1.52:27431"
        );
    }
}
