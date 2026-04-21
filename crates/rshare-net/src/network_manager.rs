//! Network manager - unified discovery and connection management

use anyhow::Result;
use std::collections::HashMap;
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
        conn.connections()
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

                        let _ = discovery_tx.try_send(NetworkEvent::DeviceFound(device));
                    }
                    crate::discovery::DiscoveryEvent::DeviceUpdated(device) => {
                        let device_id = device.id;

                        {
                            let mut devices = discovered_devices.write().await;
                            devices.insert(device_id, device.clone());
                        }

                        let _ = discovery_tx.try_send(NetworkEvent::DeviceFound(device));
                    }
                    crate::discovery::DiscoveryEvent::DeviceLost(id) => {
                        {
                            let mut devices = discovered_devices.write().await;
                            devices.remove(&id);
                        }

                        let _ = discovery_tx.try_send(NetworkEvent::DeviceDisconnected(id));
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
        let mut conn = self.connection.lock().await;
        conn.connect(device_id, address).await
    }

    /// Disconnect from a device
    pub async fn disconnect_from(&mut self, device_id: &DeviceId) -> Result<()> {
        let mut conn = self.connection.lock().await;
        conn.disconnect(device_id).await
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
