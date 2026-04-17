//! Device discovery using mDNS/UDP broadcast

use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::{interval, Instant};

use rshare_core::{DeviceId, Message, hello_message, hello_back_message};

/// Discovery configuration
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// UDP port for discovery broadcasts
    pub port: u16,
    /// Broadcast interval
    pub broadcast_interval: Duration,
    /// Device timeout (how long until a device is considered offline)
    pub device_timeout: Duration,
    /// Enable mDNS discovery
    pub mdns_enabled: bool,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            port: 27432,
            broadcast_interval: Duration::from_secs(5),
            device_timeout: Duration::from_secs(30),
            mdns_enabled: true,
        }
    }
}

/// Discovered device information
#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub id: DeviceId,
    pub name: String,
    pub hostname: String,
    pub addresses: Vec<SocketAddr>,
    pub last_seen: Instant,
}

impl DiscoveredDevice {
    fn from_message(addr: SocketAddr, msg: &Message) -> Option<Self> {
        match msg {
            Message::Hello {
                device_id,
                device_name,
                hostname,
                ..
            }
            | Message::HelloBack {
                device_id,
                device_name,
                hostname,
                ..
            } => Some(Self {
                id: *device_id,
                name: device_name.clone(),
                hostname: hostname.clone(),
                addresses: vec![addr],
                last_seen: Instant::now(),
            }),
            _ => None,
        }
    }

    /// Check if this device is stale (not seen recently)
    fn is_stale(&self, timeout: Duration) -> bool {
        self.last_seen.elapsed() > timeout
    }
}

/// Event from the discovery service
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    /// A new device was discovered
    DeviceFound(DiscoveredDevice),
    /// A device was updated
    DeviceUpdated(DiscoveredDevice),
    /// A device went offline
    DeviceLost(DeviceId),
    /// Discovery error
    Error(String),
}

/// Service discovery for finding R-ShareMouse devices on the network
pub struct ServiceDiscovery {
    config: DiscoveryConfig,
    local_device_id: DeviceId,
    local_device_name: String,
    local_hostname: String,
    devices: HashMap<DeviceId, DiscoveredDevice>,
    event_tx: Option<mpsc::Sender<DiscoveryEvent>>,
    running: bool,
}

impl ServiceDiscovery {
    /// Create a new service discovery instance
    pub fn new(
        local_device_id: DeviceId,
        local_device_name: String,
        local_hostname: String,
    ) -> Self {
        Self {
            config: DiscoveryConfig::default(),
            local_device_id,
            local_device_name,
            local_hostname,
            devices: HashMap::new(),
            event_tx: None,
            running: false,
        }
    }

    /// Set the discovery configuration
    pub fn with_config(mut self, config: DiscoveryConfig) -> Self {
        self.config = config;
        self
    }

    /// Get all currently discovered devices
    pub fn devices(&self) -> Vec<&DiscoveredDevice> {
        self.devices.values().collect()
    }

    /// Get a specific device by ID
    pub fn get_device(&self, id: &DeviceId) -> Option<&DiscoveredDevice> {
        self.devices.get(id)
    }

    /// Start discovering devices with event channel
    pub async fn start_with_channel(
        &mut self,
        event_tx: mpsc::Sender<DiscoveryEvent>,
    ) -> Result<()> {
        self.event_tx = Some(event_tx);
        self.start().await
    }

    /// Start discovering devices
    pub async fn start(&mut self) -> Result<()> {
        if self.running {
            return Ok(());
        }

        self.running = true;

        let bind_addr = format!("0.0.0.0:{}", self.config.port);
        let socket = UdpSocket::bind(&bind_addr).await?;
        socket.set_broadcast(true)?;

        tracing::info!("Service discovery listening on {}", bind_addr);

        let mut buf = [0u8; 4096];
        let mut broadcast_interval = interval(self.config.broadcast_interval);
        let mut cleanup_interval = interval(Duration::from_secs(10));

        // Create the hello message once
        let hello_msg = hello_message(
            self.local_device_id,
            self.local_device_name.clone(),
            self.local_hostname.clone(),
        );
        let hello_bytes = serialize_message(&hello_msg)?;

        let broadcast_addr: SocketAddr = format!("255.255.255.255:{}", self.config.port).parse()?;

        while self.running {
            tokio::select! {
                // Handle incoming discovery messages
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, addr)) => {
                            if let Err(e) = self.handle_packet(&buf[..len], addr, &socket).await {
                                tracing::debug!("Error handling discovery packet: {}", e);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Error receiving discovery packet: {}", e);
                        }
                    }
                }

                // Send periodic broadcasts
                _ = broadcast_interval.tick() => {
                    if let Err(e) = socket.send_to(&hello_bytes, broadcast_addr).await {
                        tracing::warn!("Failed to send broadcast: {}", e);
                    }
                }

                // Clean up stale devices
                _ = cleanup_interval.tick() => {
                    self.cleanup_stale_devices();
                }
            }
        }

        Ok(())
    }

    /// Stop discovering devices
    pub async fn stop(&mut self) -> Result<()> {
        self.running = false;
        tracing::info!("Service discovery stopped");
        Ok(())
    }

    /// Handle an incoming discovery packet
    async fn handle_packet(
        &mut self,
        data: &[u8],
        addr: SocketAddr,
        socket: &UdpSocket,
    ) -> Result<()> {
        // Parse the incoming message
        let msg = deserialize_message(data)?;

        // Ignore messages from ourselves
        let sender_id = match &msg {
            Message::Hello { device_id, .. } => Some(*device_id),
            Message::HelloBack { device_id, .. } => Some(*device_id),
            _ => None,
        };

        if let Some(id) = sender_id {
            if id == self.local_device_id {
                return Ok(());
            }
        }

        match msg {
            Message::Hello { .. } => {
                // Someone is announcing themselves - respond with HelloBack
                if let Some(device) = DiscoveredDevice::from_message(addr, &msg) {
                    let device_id = device.id;
                    let is_new = !self.devices.contains_key(&device_id);

                    self.devices.insert(device_id, device.clone());

                    // Send HelloBack response
                    let hello_back = hello_back_message(
                        self.local_device_id,
                        self.local_device_name.clone(),
                        self.local_hostname.clone(),
                        rshare_core::ScreenInfo::primary(),
                    );
                    let bytes = serialize_message(&hello_back)?;

                    if let Err(e) = socket.send_to(&bytes, addr).await {
                        tracing::warn!("Failed to send HelloBack: {}", e);
                    }

                    // Notify about the device
                    if let Some(tx) = &self.event_tx {
                        let event = if is_new {
                            DiscoveryEvent::DeviceFound(device)
                        } else {
                            DiscoveryEvent::DeviceUpdated(device)
                        };
                        let _ = tx.try_send(event);
                    }
                }
            }
            Message::HelloBack { .. } => {
                // Response to our Hello - someone acknowledged us
                if let Some(device) = DiscoveredDevice::from_message(addr, &msg) {
                    let device_id = device.id;
                    let is_new = !self.devices.contains_key(&device_id);

                    self.devices.insert(device_id, device.clone());

                    if let Some(tx) = &self.event_tx {
                        let event = if is_new {
                            DiscoveryEvent::DeviceFound(device)
                        } else {
                            DiscoveryEvent::DeviceUpdated(device)
                        };
                        let _ = tx.try_send(event);
                    }
                }
            }
            _ => {
                // Ignore other message types in discovery
            }
        }

        Ok(())
    }

    /// Remove stale devices from the registry
    fn cleanup_stale_devices(&mut self) {
        let mut lost_devices = Vec::new();

        self.devices.retain(|id, device| {
            if device.is_stale(self.config.device_timeout) {
                lost_devices.push(*id);
                false
            } else {
                true
            }
        });

        // Notify about lost devices
        if let Some(tx) = &self.event_tx {
            for id in lost_devices {
                let _ = tx.try_send(DiscoveryEvent::DeviceLost(id));
            }
        }
    }
}

impl Drop for ServiceDiscovery {
    fn drop(&mut self) {
        self.running = false;
    }
}

/// Serialize a message to bytes
fn serialize_message(msg: &Message) -> Result<Vec<u8>> {
    serde_json::to_vec(msg).map_err(|e| anyhow::anyhow!("Serialization error: {}", e))
}

/// Deserialize a message from bytes
fn deserialize_message(data: &[u8]) -> Result<Message> {
    serde_json::from_slice(data).map_err(|e| anyhow::anyhow!("Deserialization error: {}", e))
}

/// Create a broadcast address for discovery
pub fn broadcast_address(port: u16) -> SocketAddr {
    format!("255.255.255.255:{}", port)
        .parse()
        .expect("valid broadcast address")
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_discovery_config_default() {
        let config = DiscoveryConfig::default();
        assert_eq!(config.port, 27432);
        assert!(config.mdns_enabled);
    }

    #[test]
    fn test_broadcast_address() {
        let addr = broadcast_address(27432);
        assert_eq!(addr.port(), 27432);
        assert_eq!(addr.ip().to_string(), "255.255.255.255");
    }

    #[test]
    fn test_message_serialize() {
        let msg = hello_message(
            Uuid::new_v4(),
            "Test".to_string(),
            "test-host".to_string(),
        );
        let bytes = serialize_message(&msg).unwrap();
        assert!(!bytes.is_empty());

        let decoded = deserialize_message(&bytes).unwrap();
        match decoded {
            Message::Hello { device_name, .. } => {
                assert_eq!(device_name, "Test");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_discovered_device_stale() {
        let mut device = DiscoveredDevice {
            id: Uuid::new_v4(),
            name: "Test".to_string(),
            hostname: "test".to_string(),
            addresses: vec![],
            last_seen: Instant::now(),
        };

        assert!(!device.is_stale(Duration::from_secs(10)));
        assert!(device.is_stale(Duration::from_secs(0)));
    }
}
