//! Network manager - unified discovery and connection management

use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex as TokioMutex, RwLock};
use tokio::task::JoinHandle;

use crate::{
    connection::{
        ConnectionInfo, ConnectionManager, ConnectionView, ManagerEvent, PeerApprovalHandle,
        PeerConnectionPolicy,
    },
    discovery::{DiscoveredDevice, DiscoveryEvent, PeerProtocolCompatibility, ServiceDiscovery},
    handshake::PeerAuthContext,
    qos::{
        BulkFrame, ClassifiedMessage, ConnectionRegistry, ControlFrame, TelemetryFrame,
        TerminalReleaseEvent, TransportSendError,
    },
    transport::PeerInbound,
};
pub use rshare_core::PendingPeerApproval;
use rshare_core::{ControlConnectionId, DeviceId, Message};

/// Network event
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    /// Device discovered
    DeviceFound(DiscoveredDevice),
    /// Discovery expired or a peer sent Goodbye. This is not a transport
    /// disconnect and intentionally carries no connection generation.
    DeviceLost(DeviceId),
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
    TelemetryReceived {
        auth: Arc<PeerAuthContext>,
        frame: TelemetryFrame,
    },
    BulkReceived {
        auth: Arc<PeerAuthContext>,
        frame: BulkFrame,
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
    /// Enables automatic connection attempts for compatible operator-approved peers.
    pub auto_connect: bool,
    /// Discovery broadcast interval
    pub broadcast_interval: Duration,
    /// Device timeout
    pub device_timeout: Duration,
    /// Legacy mDNS switch. It is retained solely to warn for old configurations;
    /// discovery continues to use UDP broadcast regardless of this value.
    pub mdns_enabled: bool,
}

impl Default for NetworkManagerConfig {
    fn default() -> Self {
        Self {
            discovery_port: 27432,
            bind_address: "0.0.0.0:27431".to_string(),
            auto_connect: true,
            broadcast_interval: Duration::from_secs(5),
            device_timeout: Duration::from_secs(30),
            mdns_enabled: false,
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
    auto_connect_scheduler: Arc<TokioMutex<AutoConnectScheduler>>,
    peer_connection_policy: Arc<PeerConnectionPolicy>,
    peer_approval_handle: PeerApprovalHandle,
    local_certificate_fingerprint: String,
    running: bool,
    discovery_task: Option<JoinHandle<()>>,
    auto_connect_retry_task: Option<JoinHandle<()>>,
}

// Repeated UDP discovery broadcasts are expected. These bounds keep a bad or
// unavailable discovered peer from creating an unbounded stream of QUIC attempts.
const AUTO_CONNECT_RETRY_BASE: Duration = Duration::from_secs(1);
const AUTO_CONNECT_RETRY_MAX: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct AutoConnectAttempt {
    in_flight: bool,
    failures: u32,
    next_allowed_attempt: Option<Instant>,
}

#[derive(Debug, Default)]
struct AutoConnectScheduler {
    attempts: HashMap<DeviceId, AutoConnectAttempt>,
}

impl AutoConnectScheduler {
    fn start_attempt_if_disconnected(
        &mut self,
        device_id: DeviceId,
        now: Instant,
        is_connected: bool,
    ) -> bool {
        !is_connected && self.start_attempt(device_id, now)
    }

    fn start_attempt(&mut self, device_id: DeviceId, now: Instant) -> bool {
        let attempt = self.attempts.entry(device_id).or_default();
        if attempt.in_flight
            || attempt
                .next_allowed_attempt
                .is_some_and(|next_allowed| now < next_allowed)
        {
            return false;
        }
        attempt.in_flight = true;
        true
    }

    fn complete_attempt(&mut self, device_id: DeviceId, now: Instant, succeeded: bool) {
        if succeeded {
            self.attempts.remove(&device_id);
            return;
        }

        let attempt = self.attempts.entry(device_id).or_default();
        attempt.in_flight = false;
        attempt.failures = attempt.failures.saturating_add(1);
        attempt.next_allowed_attempt = Some(now + auto_connect_retry_delay(attempt.failures));
    }

    fn on_device_lost(&mut self, _device_id: DeviceId) {
        // Discovery visibility is not transport authentication. Retain retry
        // history across DeviceLost/Goodbye churn so a discovered peer cannot
        // turn bounded backoff into repeated connection attempts.
    }

    fn cancel_attempt(&mut self, device_id: DeviceId) {
        if let Some(attempt) = self.attempts.get_mut(&device_id) {
            attempt.in_flight = false;
        }
    }
}

fn auto_connect_retry_delay(failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(6);
    AUTO_CONNECT_RETRY_BASE
        .saturating_mul(1_u32 << exponent)
        .min(AUTO_CONNECT_RETRY_MAX)
}

fn auto_connect_address(
    device: &DiscoveredDevice,
    config: &NetworkManagerConfig,
) -> Option<String> {
    device.addresses.first().map(|address| {
        normalize_discovered_connection_address(
            &address.to_string(),
            config.discovery_port,
            connection_port(&config.bind_address),
        )
    })
}

async fn maybe_auto_connect_discovered_device(
    device: DiscoveredDevice,
    local_device_id: DeviceId,
    config: &NetworkManagerConfig,
    connection: &Arc<TokioMutex<ConnectionManager>>,
    connection_view: &ConnectionView,
    scheduler: &Arc<TokioMutex<AutoConnectScheduler>>,
    policy: &Arc<PeerConnectionPolicy>,
) {
    if device.id == local_device_id || !policy.can_auto_connect(device.id) {
        return;
    }
    if let PeerProtocolCompatibility::Incompatible { local, remote } =
        &device.protocol_compatibility
    {
        tracing::debug!(
            peer_id = %device.id,
            local_protocol = local,
            remote_protocol = remote,
            "Skipping discovery auto-connect for incompatible peer"
        );
        return;
    }

    let Some(address) = auto_connect_address(&device, config) else {
        tracing::debug!(peer_id = %device.id, "Discovered peer has no connection address");
        return;
    };
    if connection_view.is_connected(&device.id).await {
        return;
    }
    if !scheduler.lock().await.start_attempt_if_disconnected(
        device.id,
        Instant::now(),
        connection_view.is_connected(&device.id).await,
    ) {
        return;
    }

    let device_id = device.id;
    let connection = connection.clone();
    let scheduler = scheduler.clone();
    let policy = policy.clone();
    tokio::spawn(async move {
        // This is the connection-manager lifecycle mutex, not the daemon's outer
        // NetworkManager mutex. The subsequent QUIC handshake rechecks the pinned
        // certificate and rejects any changed fingerprint without overwriting it.
        let mut connection = connection.lock().await;
        // Settings and manual disconnect can change while this task is queued.
        // Recheck only after owning the lifecycle manager; never promote trust.
        if !policy.can_auto_connect(device_id) {
            scheduler.lock().await.cancel_attempt(device_id);
            return;
        }
        let result = connection.connect_trusted(device_id, &address).await;
        let succeeded = result.is_ok();
        scheduler
            .lock()
            .await
            .complete_attempt(device_id, Instant::now(), succeeded);
        if let Err(error) = result {
            tracing::debug!(peer_id = %device_id, error = %error, "Discovery auto-connect failed");
        }
    });
}

fn spawn_auto_connect_retry_task(
    local_device_id: DeviceId,
    config: NetworkManagerConfig,
    discovered_devices: Arc<RwLock<HashMap<DeviceId, DiscoveredDevice>>>,
    connection: Arc<TokioMutex<ConnectionManager>>,
    connection_view: ConnectionView,
    scheduler: Arc<TokioMutex<AutoConnectScheduler>>,
    policy: Arc<PeerConnectionPolicy>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut retry_tick = tokio::time::interval(Duration::from_secs(1));
        retry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut scan_offset = 0;
        loop {
            retry_tick.tick().await;
            // Retry known addresses after transport loss without requiring a new
            // discovery packet. Each tick scans at most 64 peers, round-robin.
            let peers = {
                let devices = discovered_devices.read().await;
                if devices.is_empty() {
                    Vec::new()
                } else {
                    scan_offset %= devices.len();
                    let count = devices.len().min(64);
                    let peers = devices
                        .values()
                        .cycle()
                        .skip(scan_offset)
                        .take(count)
                        .cloned()
                        .collect::<Vec<_>>();
                    scan_offset = (scan_offset + count) % devices.len();
                    peers
                }
            };
            for device in peers {
                maybe_auto_connect_discovered_device(
                    device,
                    local_device_id,
                    &config,
                    &connection,
                    &connection_view,
                    &scheduler,
                    &policy,
                )
                .await;
            }
        }
    })
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
                ManagerEvent::TelemetryReceived { auth, frame } => {
                    NetworkEvent::TelemetryReceived { auth, frame }
                }
                ManagerEvent::BulkReceived { auth, frame } => {
                    NetworkEvent::BulkReceived { auth, frame }
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
    local_device_id: DeviceId,
    discovered_devices: &Arc<RwLock<HashMap<DeviceId, DiscoveredDevice>>>,
    discovery_tx: &mpsc::Sender<NetworkEvent>,
    connection: &Arc<TokioMutex<ConnectionManager>>,
    connection_view: &ConnectionView,
    auto_connect_scheduler: &Arc<TokioMutex<AutoConnectScheduler>>,
    peer_connection_policy: &Arc<PeerConnectionPolicy>,
) {
    match event {
        DiscoveryEvent::DeviceFound(device) | DiscoveryEvent::DeviceUpdated(device) => {
            let device_id = device.id;
            {
                let mut devices = discovered_devices.write().await;
                devices.insert(device_id, device.clone());
            }
            let _ = discovery_tx.try_send(NetworkEvent::DeviceFound(device.clone()));
            maybe_auto_connect_discovered_device(
                device,
                local_device_id,
                config,
                connection,
                connection_view,
                auto_connect_scheduler,
                peer_connection_policy,
            )
            .await;
        }
        DiscoveryEvent::DeviceLost(id) => {
            {
                let mut devices = discovered_devices.write().await;
                devices.remove(&id);
            }
            auto_connect_scheduler.lock().await.on_device_lost(id);

            let _ = discovery_tx.try_send(discovery_lost_network_event(id));
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
        let peer_connection_policy = connection_manager.peer_connection_policy();
        let peer_approval_handle = connection_manager.peer_approval_handle();
        let local_certificate_fingerprint = connection_manager.local_certificate_fingerprint();
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
            auto_connect_scheduler: Arc::new(TokioMutex::new(AutoConnectScheduler::default())),
            peer_connection_policy,
            peer_approval_handle,
            local_certificate_fingerprint,
            running: false,
            discovery_task: None,
            auto_connect_retry_task: None,
        }
    }

    /// Set the configuration
    pub fn with_config(mut self, config: NetworkManagerConfig) -> Self {
        self.peer_connection_policy
            .set_auto_connect(config.auto_connect);
        self.config = config;
        self
    }

    /// Change the shared automatic dialing policy without restarting discovery.
    /// Established sessions remain connected; queued tasks recheck this switch.
    pub fn set_auto_connect(&mut self, enabled: bool) {
        self.config.auto_connect = enabled;
        self.peer_connection_policy.set_auto_connect(enabled);
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

    /// Read-only canonical connection projection for status/UI consumers.
    pub fn connection_snapshot_reader(&self) -> crate::connection::ConnectionSnapshotReader {
        self.connection_view.clone().into()
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

    /// List inbound peer approvals that are waiting for this target's local operator.
    pub async fn pending_peer_approvals(&self) -> Vec<PendingPeerApproval> {
        self.peer_approval_handle.list()
    }

    /// Mark one opaque approval ID as expected once. A matching retry still has to
    /// prove the same device ID and full certificate fingerprint before trust is saved.
    pub async fn approve_peer(&self, approval_id: &str) -> bool {
        self.peer_approval_handle.approve(approval_id)
    }

    /// Fingerprint of the certificate actually used by this daemon's transport.
    pub fn local_certificate_fingerprint(&self) -> String {
        self.local_certificate_fingerprint.clone()
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
        self.peer_connection_policy
            .set_auto_connect(self.config.auto_connect);

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
        let discovery_event_config = self.config.clone();
        let local_device_id = self.local_device_id;
        let connection = self.connection.clone();
        let connection_view = self.connection_view.clone();
        let auto_connect_scheduler = self.auto_connect_scheduler.clone();
        let peer_connection_policy = self.peer_connection_policy.clone();

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
            mdns_enabled: self.config.mdns_enabled,
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
                    local_device_id,
                    &discovered_devices,
                    &discovery_tx,
                    &connection,
                    &connection_view,
                    &auto_connect_scheduler,
                    &peer_connection_policy,
                )
                .await;
            }

            discovery_task.abort();
        }));

        self.auto_connect_retry_task = Some(spawn_auto_connect_retry_task(
            self.local_device_id,
            self.config.clone(),
            self.discovered_devices.clone(),
            self.connection.clone(),
            self.connection_view.clone(),
            self.auto_connect_scheduler.clone(),
            self.peer_connection_policy.clone(),
        ));

        tracing::info!("Network manager started");
        Ok(())
    }

    /// Stop the network manager
    pub async fn stop(&mut self) -> Result<()> {
        if !self.running {
            return Ok(());
        }

        self.running = false;
        self.peer_connection_policy.set_auto_connect(false);
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
        if let Some(task) = self.auto_connect_retry_task.take() {
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
        self.peer_connection_policy.suppress(*device_id);
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

fn discovery_lost_network_event(device_id: DeviceId) -> NetworkEvent {
    NetworkEvent::DeviceLost(device_id)
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
        remote.approve_inbound_peer_for_test(&local_connection);
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
        assert!(config.auto_connect);
        assert!(!config.mdns_enabled);
    }

    #[test]
    fn auto_connect_scheduler_starts_once_and_suppresses_connected_or_in_flight_duplicates() {
        let device_id = DeviceId::new_v4();
        let now = Instant::now();
        let mut scheduler = AutoConnectScheduler::default();

        assert!(scheduler.start_attempt_if_disconnected(device_id, now, false));
        assert!(
            !scheduler.start_attempt_if_disconnected(device_id, now, false),
            "an in-flight attempt must suppress duplicate discovery"
        );
        scheduler.complete_attempt(device_id, now, true);
        assert!(
            !scheduler.start_attempt_if_disconnected(device_id, now, true),
            "a canonical connection must suppress auto-connect"
        );
    }

    #[test]
    fn auto_connect_scheduler_failure_backoff_grows_and_caps() {
        let device_id = DeviceId::new_v4();
        let mut now = Instant::now();
        let mut scheduler = AutoConnectScheduler::default();
        let mut delays = Vec::new();

        for _ in 0..8 {
            assert!(scheduler.start_attempt(device_id, now));
            scheduler.complete_attempt(device_id, now, false);
            let attempt = scheduler.attempts.get(&device_id).unwrap();
            let next_allowed_attempt = attempt.next_allowed_attempt.unwrap();
            delays.push(next_allowed_attempt.duration_since(now));
            assert!(
                !scheduler.start_attempt(device_id, now),
                "backoff must suppress repeated discovery"
            );
            now = next_allowed_attempt;
        }

        assert_eq!(delays[0], Duration::from_secs(1));
        assert_eq!(delays[1], Duration::from_secs(2));
        assert_eq!(*delays.last().unwrap(), AUTO_CONNECT_RETRY_MAX);
    }

    #[test]
    fn auto_connect_scheduler_keeps_backoff_across_device_lost_and_rediscovery() {
        let device_id = DeviceId::new_v4();
        let now = Instant::now();
        let mut scheduler = AutoConnectScheduler::default();

        assert!(scheduler.start_attempt(device_id, now));
        scheduler.complete_attempt(device_id, now, false);
        scheduler.on_device_lost(device_id);

        assert!(
            !scheduler.start_attempt_if_disconnected(device_id, now, false),
            "rediscovery during the retry window must not bypass backoff"
        );
        let retry_at = scheduler.attempts[&device_id].next_allowed_attempt.unwrap();
        assert!(scheduler.start_attempt_if_disconnected(device_id, retry_at, false));
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
    fn discovery_lost_has_its_own_lifecycle_event_when_transport_is_connected() {
        let device_id = DeviceId::new_v4();
        assert!(matches!(
            discovery_lost_network_event(device_id),
            NetworkEvent::DeviceLost(id) if id == device_id
        ));
    }

    #[test]
    fn discovery_lost_has_its_own_lifecycle_event_when_transport_is_disconnected() {
        let device_id = DeviceId::new_v4();
        assert!(matches!(
            discovery_lost_network_event(device_id),
            NetworkEvent::DeviceLost(id) if id == device_id
        ));
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
    async fn peer_approval_and_local_identity_bypass_connection_lifecycle_mutex() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let manager = NetworkManager::isolated_for_test(local_id, "local".into(), "host".into());
        let (address, actual_fingerprint) = {
            let mut connection = manager.connection.lock().await;
            connection.start_server("127.0.0.1:0").await.unwrap();
            (
                connection.transport_local_addr().unwrap().to_string(),
                connection.local_certificate_fingerprint(),
            )
        };
        let mut peer = ConnectionManager::isolated_for_test(remote_id);
        assert!(peer.connect(local_id, &address).await.is_err());
        let _held = manager.connection.lock().await;
        tokio::time::timeout(Duration::from_millis(50), async {
            let approval = manager.pending_peer_approvals().await.pop().unwrap();
            assert_eq!(approval.device_id, remote_id);
            assert!(manager.approve_peer(&approval.approval_id).await);
            assert!(manager.pending_peer_approvals().await.is_empty());
            assert_eq!(manager.local_certificate_fingerprint(), actual_fingerprint);
        })
        .await
        .expect("local approval and identity queries must not wait for an automatic handshake");
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
        remote.approve_inbound_peer_for_test(&local_connection);
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
    async fn discovery_does_not_dial_unknown_legacy_or_corrupt_trust_store() {
        use crate::encryption::{PeerCertificateFingerprint, QuicTrustStore};

        for trust_state in ["unknown", "legacy", "corrupt"] {
            let local_id = DeviceId::new_v4();
            let remote_id = DeviceId::new_v4();
            let state_dir =
                std::env::temp_dir().join(format!("rshare-auto-{}", uuid::Uuid::new_v4()));
            let trust_path = state_dir.join("trust.json");
            std::fs::create_dir_all(&state_dir).unwrap();
            match trust_state {
                "legacy" => {
                    QuicTrustStore::trust_first_seen_at(
                        &trust_path,
                        remote_id,
                        PeerCertificateFingerprint::from_der(b"legacy certificate"),
                    )
                    .unwrap();
                }
                "corrupt" => std::fs::write(&trust_path, b"invalid json").unwrap(),
                _ => {}
            }
            let probe = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let manager = NetworkManager::with_connection_manager(
                local_id,
                "local".into(),
                "local-host".into(),
                ConnectionManager::with_transport(
                    local_id,
                    crate::transport::QuicTransport::isolated_for_test(local_id)
                        .with_trust_store_path(&trust_path),
                ),
            );
            handle_discovery_event(
                DiscoveryEvent::DeviceFound(discovered_device(
                    remote_id,
                    probe.local_addr().unwrap(),
                    "unapproved",
                )),
                &manager.config,
                manager.local_device_id,
                &manager.discovered_devices,
                &manager.event_tx,
                &manager.connection,
                &manager.connection_view,
                &manager.auto_connect_scheduler,
                &manager.peer_connection_policy,
            )
            .await;
            let mut packet = [0; 2048];
            assert!(
                tokio::time::timeout(Duration::from_millis(100), probe.recv_from(&mut packet))
                    .await
                    .is_err(),
                "{trust_state} peer must remain visible without an automatic dial"
            );
            assert_eq!(manager.discovered_devices().await.len(), 1);
            assert!(manager
                .auto_connect_scheduler
                .lock()
                .await
                .attempts
                .is_empty());
            std::fs::remove_dir_all(state_dir).unwrap();
        }
    }

    #[tokio::test]
    async fn approved_retry_restores_transport_without_rediscovery_and_respects_hot_toggle() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let local = ConnectionManager::isolated_for_test(local_id);
        let mut remote = ConnectionManager::isolated_for_test(remote_id);
        local.approve_inbound_peer_for_test(&remote);
        remote.approve_inbound_peer_for_test(&local);
        remote.start_server("127.0.0.1:0").await.unwrap();
        let address = remote.transport_local_addr().unwrap();
        let mut manager =
            NetworkManager::with_connection_manager(local_id, "local".into(), "host".into(), local)
                .with_config(NetworkManagerConfig {
                    auto_connect: false,
                    ..NetworkManagerConfig::default()
                });
        manager
            .discovered_devices
            .write()
            .await
            .insert(remote_id, discovered_device(remote_id, address, "approved"));
        let retry_task = spawn_auto_connect_retry_task(
            local_id,
            manager.config.clone(),
            manager.discovered_devices.clone(),
            manager.connection.clone(),
            manager.connection_view.clone(),
            manager.auto_connect_scheduler.clone(),
            manager.peer_connection_policy.clone(),
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!manager.is_connected(&remote_id).await);
        manager.set_auto_connect(true);
        tokio::time::timeout(Duration::from_secs(2), async {
            while !manager.is_connected(&remote_id).await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("enabling should connect already discovered approved devices");
        let first_generation = manager.connection_infos().await[0].control_connection_id;
        tokio::time::timeout(Duration::from_secs(1), async {
            while !remote.is_connected(&local_id).await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("target must publish its inbound transport before fault injection");
        manager.set_auto_connect(false);
        assert!(
            manager.is_connected(&remote_id).await,
            "disabling auto-connect preserves active sessions"
        );
        remote.close_peer_transport_for_test(&local_id).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while manager.is_connected(&remote_id).await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert!(
            !manager.is_connected(&remote_id).await,
            "disabled policy must prevent automatic reconnection"
        );
        manager.set_auto_connect(true);
        tokio::time::timeout(Duration::from_secs(2), async {
            while !manager.is_connected(&remote_id).await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cached discovery address must reconnect without another discovery packet");
        assert_ne!(
            manager.connection_infos().await[0].control_connection_id,
            first_generation
        );
        assert!(manager.peer_connection_policy.can_auto_connect(remote_id));
        retry_task.abort();
        manager.disconnect_from(&remote_id).await.unwrap();
    }

    #[tokio::test]
    async fn queued_auto_connect_rechecks_hot_disable_and_manual_suppression() {
        for manual_disconnect in [false, true] {
            let local_id = DeviceId::new_v4();
            let remote_id = DeviceId::new_v4();
            let mut manager =
                NetworkManager::isolated_for_test(local_id, "local".into(), "host".into());
            let trust_path = manager
                .connection
                .lock()
                .await
                .transport_trust_store_path_for_test();
            crate::encryption::QuicTrustStore::approve_at(
                trust_path,
                remote_id,
                crate::encryption::PeerCertificateFingerprint::from_der(b"approved certificate"),
            )
            .unwrap();
            let probe = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let connection = manager.connection.clone();
            let held = connection.lock().await;
            maybe_auto_connect_discovered_device(
                discovered_device(remote_id, probe.local_addr().unwrap(), "approved"),
                local_id,
                &manager.config,
                &manager.connection,
                &manager.connection_view,
                &manager.auto_connect_scheduler,
                &manager.peer_connection_policy,
            )
            .await;
            assert!(manager.auto_connect_scheduler.lock().await.attempts[&remote_id].in_flight);
            if manual_disconnect {
                // The lifecycle mutex is held: suppression must become visible
                // before disconnect can acquire it, including for queued dials.
                assert!(tokio::time::timeout(
                    Duration::from_millis(20),
                    manager.disconnect_from(&remote_id)
                )
                .await
                .is_err());
            } else {
                manager.set_auto_connect(false);
            }
            drop(held);
            tokio::time::timeout(Duration::from_secs(1), async {
                while manager.auto_connect_scheduler.lock().await.attempts[&remote_id].in_flight {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let mut packet = [0; 2048];
            assert!(
                tokio::time::timeout(Duration::from_millis(100), probe.recv_from(&mut packet))
                    .await
                    .is_err()
            );
            assert!(!manager.peer_connection_policy.can_auto_connect(remote_id));
            manager.set_auto_connect(true);
            assert_eq!(
                manager.peer_connection_policy.can_auto_connect(remote_id),
                !manual_disconnect
            );
            if manual_disconnect {
                assert!(manager
                    .connect_to(remote_id, "invalid address")
                    .await
                    .is_err());
                assert!(manager.peer_connection_policy.can_auto_connect(remote_id));
            }
        }
    }

    #[tokio::test]
    async fn auto_connect_attempts_found_and_updated_compatible_devices() {
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
        let trust_path = manager
            .connection
            .lock()
            .await
            .transport_trust_store_path_for_test();
        crate::encryption::QuicTrustStore::approve_at(
            trust_path,
            remote_id,
            crate::encryption::PeerCertificateFingerprint::from_der(b"approved peer"),
        )
        .unwrap();

        let found = discovered_device(remote_id, probe.local_addr().unwrap(), "first");
        handle_discovery_event(
            crate::discovery::DiscoveryEvent::DeviceFound(found.clone()),
            &manager.config,
            manager.local_device_id,
            &manager.discovered_devices,
            &manager.event_tx,
            &manager.connection,
            &manager.connection_view,
            &manager.auto_connect_scheduler,
            &manager.peer_connection_policy,
        )
        .await;
        let mut updated = found;
        updated.name = "updated".to_string();
        handle_discovery_event(
            crate::discovery::DiscoveryEvent::DeviceUpdated(updated.clone()),
            &manager.config,
            manager.local_device_id,
            &manager.discovered_devices,
            &manager.event_tx,
            &manager.connection,
            &manager.connection_view,
            &manager.auto_connect_scheduler,
            &manager.peer_connection_policy,
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
            tokio::time::timeout(Duration::from_secs(1), probe.recv_from(&mut packet))
                .await
                .is_ok(),
            "automatic discovery must start a QUIC connection attempt"
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
            .send(crate::connection::ManagerEvent::TelemetryReceived {
                auth: auth.clone(),
                frame: crate::qos::TelemetryFrame::latency_probe(3, 4, false, None),
            })
            .await
            .unwrap();
        match tokio::time::timeout(Duration::from_secs(1), network_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            NetworkEvent::TelemetryReceived {
                auth: received,
                frame,
            } => {
                assert_eq!(received.control_connection_id, auth.control_connection_id);
                assert!(matches!(
                    frame.into_message(),
                    Message::LatencyProbe {
                        sequence: 3,
                        timestamp_ms: 4,
                        ..
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
            manager
                .connection
                .lock()
                .await
                .approve_inbound_peer_for_test(&client);
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
        manager
            .connection
            .lock()
            .await
            .approve_inbound_peer_for_test(&overflow_client);
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
