//! Device discovery using mDNS/UDP broadcast

use anyhow::Result;
use if_addrs::{get_if_addrs, IfAddr, Interface};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{interval, Instant};

use rshare_core::{hello_back_message, hello_message, DeviceId, Message};

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

        let mut broadcast_targets = discovery_broadcast_targets(self.config.port);
        tracing::info!("Discovery broadcast targets: {:?}", broadcast_targets);

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
                    broadcast_targets = discovery_broadcast_targets(self.config.port);
                    for target in &broadcast_targets {
                        if let Err(e) = socket.send_to(&hello_bytes, target).await {
                            tracing::warn!("Failed to send discovery packet to {}: {}", target, e);
                        }
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

/// Background discovery task handle.
///
/// `ServiceDiscovery::start_with_channel` runs until the service is stopped, so
/// callers that need to consume events concurrently should use this helper
/// instead of awaiting `start_with_channel` inline.
pub struct DiscoveryTask {
    handle: JoinHandle<()>,
}

impl DiscoveryTask {
    /// Abort the background discovery loop and wait for the task to finish.
    pub async fn stop(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }

    /// Abort the background discovery loop without waiting.
    pub fn abort(&self) {
        self.handle.abort();
    }

    /// Check whether the discovery task has already finished.
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

/// Spawn a discovery service in the background and forward startup/runtime
/// errors to the same discovery event stream.
pub fn spawn_discovery(
    mut discovery: ServiceDiscovery,
    event_tx: mpsc::Sender<DiscoveryEvent>,
) -> DiscoveryTask {
    let error_tx = event_tx.clone();
    let handle = tokio::spawn(async move {
        if let Err(err) = discovery.start_with_channel(event_tx).await {
            let _ = error_tx.send(DiscoveryEvent::Error(err.to_string())).await;
        }
    });

    DiscoveryTask { handle }
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

fn discovery_broadcast_targets(port: u16) -> Vec<SocketAddr> {
    let mut targets = HashSet::new();

    match get_if_addrs() {
        Ok(interfaces) => {
            for interface in interfaces {
                if let Some(addr) = interface_broadcast_address(&interface) {
                    targets.insert(SocketAddr::from((addr, port)));
                }
            }
        }
        Err(err) => {
            tracing::warn!("Failed to enumerate network interfaces: {}", err);
        }
    }

    targets.insert(broadcast_address(port));

    let mut sorted: Vec<_> = targets.into_iter().collect();
    sorted.sort_by_key(|addr| addr.ip().to_string());
    sorted
}

fn interface_broadcast_address(interface: &Interface) -> Option<Ipv4Addr> {
    if !is_candidate_interface(interface) {
        return None;
    }

    match &interface.addr {
        IfAddr::V4(addr) => addr
            .broadcast
            .or_else(|| Some(compute_directed_broadcast(addr.ip, addr.netmask))),
        IfAddr::V6(_) => None,
    }
}

fn is_candidate_interface(interface: &Interface) -> bool {
    if interface.is_loopback() {
        return false;
    }
    if is_ignored_interface_name(&interface.name) {
        return false;
    }

    match &interface.addr {
        IfAddr::V4(addr) => is_candidate_ipv4(addr.ip),
        IfAddr::V6(_) => false,
    }
}

fn is_candidate_ipv4(ip: Ipv4Addr) -> bool {
    !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_broadcast()
}

fn is_ignored_interface_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    const IGNORED_MARKERS: &[&str] = &[
        "vmware", "virtual", "vbox", "hyper-v", "wintun", "wireguard", "tailscale", "zerotier",
        "docker", "podman", "vnic", "loopback", "npcap", "tap", "tun",
    ];

    IGNORED_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn compute_directed_broadcast(ip: Ipv4Addr, netmask: Ipv4Addr) -> Ipv4Addr {
    let ip = u32::from(ip);
    let netmask = u32::from(netmask);
    Ipv4Addr::from((ip & netmask) | !netmask)
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
    fn test_compute_directed_broadcast() {
        assert_eq!(
            compute_directed_broadcast(
                Ipv4Addr::new(192, 168, 1, 52),
                Ipv4Addr::new(255, 255, 255, 0)
            ),
            Ipv4Addr::new(192, 168, 1, 255)
        );
    }

    #[test]
    fn test_candidate_ipv4_filters_link_local() {
        assert!(is_candidate_ipv4(Ipv4Addr::new(192, 168, 1, 52)));
        assert!(!is_candidate_ipv4(Ipv4Addr::new(169, 254, 14, 146)));
        assert!(!is_candidate_ipv4(Ipv4Addr::new(127, 0, 0, 1)));
    }

    #[test]
    fn test_ignored_interface_name_filters_virtual_adapters() {
        assert!(is_ignored_interface_name("VMware Network Adapter VMnet8"));
        assert!(is_ignored_interface_name("Wintun Userspace Tunnel"));
        assert!(!is_ignored_interface_name("WLAN 3"));
    }

    #[test]
    fn test_message_serialize() {
        let msg = hello_message(Uuid::new_v4(), "Test".to_string(), "test-host".to_string());
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
        let device = DiscoveredDevice {
            id: Uuid::new_v4(),
            name: "Test".to_string(),
            hostname: "test".to_string(),
            addresses: vec![],
            last_seen: Instant::now(),
        };

        assert!(!device.is_stale(Duration::from_secs(10)));
        assert!(device.is_stale(Duration::from_secs(0)));
    }

    #[tokio::test]
    async fn spawn_discovery_returns_without_blocking_event_consumer() {
        let discovery =
            ServiceDiscovery::new(Uuid::new_v4(), "Test".to_string(), "test-host".to_string())
                .with_config(DiscoveryConfig {
                    port: 0,
                    broadcast_interval: Duration::from_secs(60),
                    device_timeout: Duration::from_secs(60),
                    mdns_enabled: false,
                });

        let (tx, mut rx) = mpsc::channel(4);
        let task = spawn_discovery(discovery, tx);

        let no_event = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
        assert!(
            no_event.is_err(),
            "event consumer should remain independently pollable"
        );

        task.stop().await;
    }
}
