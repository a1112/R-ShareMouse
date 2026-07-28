//! Connection management

use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as TokioMutex, RwLock};
use tokio::time::Instant;

use rshare_core::{ControlConnectionId, DeviceId, Message};

use super::handshake::{perform_outbound_handshake, receive_incoming_handshake};
use super::qos::{
    ClassifiedMessage, ConnectionRegistry, RegisteredPeer, TerminalReleaseEvent, TransportSendError,
};
use super::transport::{ConnectionPool, QuicTransport};

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Error,
}

/// Connection information
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub device_id: DeviceId,
    pub address: String,
    pub state: ConnectionState,
    pub last_activity: Instant,
    pub messages_sent: u64,
    pub messages_received: u64,
    pub transport: String,
    pub datagram_available: bool,
    pub rtt_ms: Option<u64>,
    pub last_datagram_rx_ms: Option<u64>,
    pub datagram_tx_dropped: u64,
    pub reliable_stream_reset_count: u64,
    pub cert_trust_state: Option<String>,
    pub control_connection_id: Option<ControlConnectionId>,
}

impl ConnectionInfo {
    pub fn new(device_id: DeviceId, address: String) -> Self {
        Self {
            device_id,
            address,
            state: ConnectionState::Connecting,
            last_activity: Instant::now(),
            messages_sent: 0,
            messages_received: 0,
            transport: "quic".to_string(),
            datagram_available: false,
            rtt_ms: None,
            last_datagram_rx_ms: None,
            datagram_tx_dropped: 0,
            reliable_stream_reset_count: 0,
            cert_trust_state: None,
            control_connection_id: None,
        }
    }

    pub fn is_stale(&self, timeout: Duration) -> bool {
        self.state != ConnectionState::Connected && self.last_activity.elapsed() > timeout
    }
}

/// Connection manager event
#[derive(Debug, Clone)]
pub enum ManagerEvent {
    Connected(DeviceId),
    Disconnected(DeviceId),
    MessageReceived { from: DeviceId, message: Message },
    Error { device_id: DeviceId, error: String },
}

#[derive(Clone)]
struct CanonicalConnection {
    generation: u64,
    info: ConnectionInfo,
}

type CanonicalConnections = Arc<StdRwLock<HashMap<DeviceId, CanonicalConnection>>>;

#[derive(Clone)]
pub(crate) struct ConnectionView {
    connections: CanonicalConnections,
    pool: Arc<ConnectionPool>,
}

impl ConnectionView {
    pub(crate) async fn connection_infos(&self) -> Vec<ConnectionInfo> {
        collect_connection_infos(&self.connections, &self.pool).await
    }

    pub(crate) async fn is_connected(&self, device_id: &DeviceId) -> bool {
        self.pool.diagnostics_for(device_id).await.is_some()
    }

    pub(crate) async fn send_legacy(&self, device_id: &DeviceId, message: &Message) -> Result<()> {
        self.pool.send_to(device_id, message).await?;
        if let Some(connection) = self
            .connections
            .write()
            .expect("canonical connection registry poisoned")
            .get_mut(device_id)
        {
            connection.info.messages_sent += 1;
            connection.info.last_activity = Instant::now();
        }
        Ok(())
    }

    pub(crate) async fn broadcast_legacy(&self, message: &Message) -> Result<()> {
        self.pool.broadcast(message).await
    }
}

async fn collect_connection_infos(
    connections: &CanonicalConnections,
    pool: &ConnectionPool,
) -> Vec<ConnectionInfo> {
    let mut infos_by_id: HashMap<_, _> = connections
        .read()
        .expect("canonical connection registry poisoned")
        .iter()
        .map(|(device_id, connection)| (*device_id, connection.info.clone()))
        .collect();
    let active_diagnostics = pool.diagnostics_all().await;
    let active_device_ids: std::collections::HashSet<_> = active_diagnostics
        .iter()
        .map(|(device_id, _)| *device_id)
        .collect();

    for (device_id, info) in &mut infos_by_id {
        if info.state == ConnectionState::Connected && !active_device_ids.contains(device_id) {
            info.state = ConnectionState::Disconnected;
            info.datagram_available = false;
            info.rtt_ms = None;
            info.last_datagram_rx_ms = None;
            info.cert_trust_state = None;
        }
    }

    for (device_id, diagnostics) in active_diagnostics {
        let info = infos_by_id
            .entry(device_id)
            .or_insert_with(|| ConnectionInfo::new(device_id, diagnostics.address.clone()));
        info.state = ConnectionState::Connected;
        info.address = diagnostics.address;
        info.transport = diagnostics.transport.to_string();
        info.datagram_available = diagnostics.datagram_available;
        info.rtt_ms = diagnostics.rtt_ms;
        info.last_datagram_rx_ms = diagnostics.last_datagram_rx_ms;
        info.datagram_tx_dropped = diagnostics.datagram_tx_dropped;
        info.reliable_stream_reset_count = diagnostics.reliable_stream_reset_count;
        info.cert_trust_state = diagnostics.cert_trust_state;
    }
    infos_by_id.into_values().collect()
}

fn forward_terminal_releases(
    mut releases: mpsc::Receiver<TerminalReleaseEvent>,
    target: mpsc::Sender<TerminalReleaseEvent>,
) {
    tokio::spawn(async move {
        while let Some(release) = releases.recv().await {
            if target.send(release).await.is_err() {
                break;
            }
        }
    });
}

fn ensure_broadcast_success(
    results: Vec<(DeviceId, std::result::Result<(), TransportSendError>)>,
) -> Result<()> {
    if let Some((device_id, error)) = results
        .into_iter()
        .find_map(|(device_id, result)| result.err().map(|error| (device_id, error)))
    {
        anyhow::bail!("QoS broadcast to {device_id} failed: {error}");
    }
    Ok(())
}

/// Connection manager for handling multiple device connections
pub struct ConnectionManager {
    local_device_id: DeviceId,
    connections: CanonicalConnections,
    next_generation: Arc<AtomicU64>,
    lifecycle_lock: Arc<TokioMutex<()>>,
    qos_registry: Arc<ConnectionRegistry>,
    transport: QuicTransport,
    pool: Arc<ConnectionPool>,
    event_tx: mpsc::Sender<ManagerEvent>,
    event_rx: Option<mpsc::Receiver<ManagerEvent>>,
    terminal_release_tx: mpsc::Sender<TerminalReleaseEvent>,
    terminal_release_rx: Option<mpsc::Receiver<TerminalReleaseEvent>>,
}

fn spawn_message_reader(
    device_id: DeviceId,
    generation: u64,
    mut messages: mpsc::Receiver<Message>,
    first_message: Option<Message>,
    event_tx: mpsc::Sender<ManagerEvent>,
    connections: CanonicalConnections,
    pool: Arc<ConnectionPool>,
    lifecycle_lock: Arc<TokioMutex<()>>,
    qos_registry: Arc<ConnectionRegistry>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        'forwarding: {
            if let Some(message) = first_message {
                if !forward_message_for_generation(
                    &connections,
                    &lifecycle_lock,
                    &event_tx,
                    device_id,
                    generation,
                    message,
                )
                .await
                {
                    break 'forwarding;
                }
            }

            while let Some(message) = messages.recv().await {
                if !forward_message_for_generation(
                    &connections,
                    &lifecycle_lock,
                    &event_tx,
                    device_id,
                    generation,
                    message,
                )
                .await
                {
                    break 'forwarding;
                }
            }
        }

        retire_generation(
            &connections,
            &pool,
            &lifecycle_lock,
            &event_tx,
            device_id,
            generation,
            &qos_registry,
        )
        .await;
    })
}

fn allocate_generation(next_generation: &AtomicU64) -> u64 {
    next_generation
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
            generation.checked_add(1)
        })
        .expect("connection generation counter exhausted")
}

fn is_current_generation(
    connections: &CanonicalConnections,
    device_id: DeviceId,
    generation: u64,
) -> bool {
    connections
        .read()
        .expect("canonical connection registry poisoned")
        .get(&device_id)
        .is_some_and(|connection| connection.generation == generation)
}

async fn forward_message_for_generation(
    connections: &CanonicalConnections,
    lifecycle_lock: &TokioMutex<()>,
    event_tx: &mpsc::Sender<ManagerEvent>,
    device_id: DeviceId,
    generation: u64,
    message: Message,
) -> bool {
    let permit = event_tx.reserve().await.ok();
    let _lifecycle = lifecycle_lock.lock().await;
    if !is_current_generation(connections, device_id, generation) {
        return false;
    }
    if let Some(permit) = permit {
        permit.send(ManagerEvent::MessageReceived {
            from: device_id,
            message,
        });
        true
    } else {
        false
    }
}

async fn retire_generation(
    connections: &CanonicalConnections,
    pool: &ConnectionPool,
    lifecycle_lock: &TokioMutex<()>,
    event_tx: &mpsc::Sender<ManagerEvent>,
    device_id: DeviceId,
    generation: u64,
    qos_registry: &ConnectionRegistry,
) -> bool {
    if !is_current_generation(connections, device_id, generation) {
        return false;
    }
    let permit = event_tx.reserve().await.ok();
    let _lifecycle = lifecycle_lock.lock().await;
    let (removed_current, connection_id) = {
        let mut canonical = connections
            .write()
            .expect("canonical connection registry poisoned");
        if canonical
            .get(&device_id)
            .is_some_and(|connection| connection.generation == generation)
        {
            let removed = canonical.remove(&device_id);
            (
                true,
                removed.and_then(|connection| connection.info.control_connection_id),
            )
        } else {
            (false, None)
        }
    };

    if removed_current {
        if let Some(connection_id) = connection_id {
            qos_registry.remove_if_generation(device_id, connection_id);
        }
        pool.remove_generation(&device_id, generation).await;
        if let Some(permit) = permit {
            permit.send(ManagerEvent::Disconnected(device_id));
        }
    }
    removed_current
}

impl ConnectionManager {
    pub fn new(local_device_id: DeviceId) -> Self {
        Self::with_transport(local_device_id, QuicTransport::new(local_device_id))
    }

    pub fn with_transport(local_device_id: DeviceId, mut transport: QuicTransport) -> Self {
        let (event_tx, event_rx) = mpsc::channel(100);
        let (terminal_release_tx, terminal_release_rx) = mpsc::channel(32);
        transport.require_peer_protocol_handshake();
        let pool = Arc::new(ConnectionPool::new(local_device_id));

        Self {
            local_device_id,
            connections: Arc::new(StdRwLock::new(HashMap::new())),
            next_generation: Arc::new(AtomicU64::new(1)),
            lifecycle_lock: Arc::new(TokioMutex::new(())),
            qos_registry: Arc::new(ConnectionRegistry::new()),
            transport,
            pool,
            event_tx,
            event_rx: Some(event_rx),
            terminal_release_tx,
            terminal_release_rx: Some(terminal_release_rx),
        }
    }

    pub async fn start_server(&mut self, bind_addr: &str) -> Result<()> {
        self.transport.start_server(bind_addr).await?;

        let mut incoming = self.transport.incoming();
        let event_tx = self.event_tx.clone();
        let pool = self.pool.clone();
        let connections = self.connections.clone();
        let next_generation = self.next_generation.clone();
        let lifecycle_lock = self.lifecycle_lock.clone();
        let qos_registry = self.qos_registry.clone();
        let terminal_release_tx = self.terminal_release_tx.clone();
        let local_device_id = self.local_device_id;

        tokio::spawn(async move {
            while let Some(mut incoming) = incoming.recv().await {
                let negotiated =
                    match receive_incoming_handshake(&mut incoming.connection, local_device_id)
                        .await
                    {
                        Ok(negotiated) => negotiated,
                        Err(error) => {
                            tracing::warn!(
                                "Rejecting unauthenticated incoming connection from {}: {}",
                                incoming.address,
                                error
                            );
                            incoming.connection.close().await;
                            continue;
                        }
                    };
                let device_id = negotiated.auth.peer_id;
                let address = incoming.address.to_string();
                incoming.connection.set_device_id(device_id);
                let mut event_permits = event_tx.reserve_many(2).await.ok();

                let installed = {
                    let _lifecycle = lifecycle_lock.lock().await;
                    if pool.diagnostics_for(&device_id).await.is_some() {
                        incoming.connection.close().await;
                        None
                    } else {
                        let removed_canonical = connections
                            .write()
                            .expect("canonical connection registry poisoned")
                            .remove(&device_id)
                            .is_some();
                        let removed_pool = pool.remove(&device_id).await.is_some();
                        if removed_canonical || removed_pool {
                            if let Some(permit) =
                                event_permits.as_mut().and_then(|permits| permits.next())
                            {
                                permit.send(ManagerEvent::Disconnected(device_id));
                            }
                        }

                        let generation = allocate_generation(&next_generation);
                        let auth = Arc::new(negotiated.auth.clone());
                        let (qos_transport, releases) =
                            incoming.connection.install_qos(auth.clone());
                        qos_registry.insert(
                            device_id,
                            RegisteredPeer {
                                auth,
                                transport: qos_transport,
                            },
                        );
                        forward_terminal_releases(releases, terminal_release_tx.clone());
                        let messages = incoming.connection.message_channel();
                        pool.insert_with_generation(device_id, generation, incoming.connection)
                            .await;
                        let mut info = ConnectionInfo::new(device_id, address);
                        info.state = ConnectionState::Connected;
                        info.cert_trust_state = Some("trusted".to_string());
                        info.control_connection_id = Some(negotiated.auth.control_connection_id);
                        connections
                            .write()
                            .expect("canonical connection registry poisoned")
                            .insert(device_id, CanonicalConnection { generation, info });
                        let connected_event_sent = if let Some(permit) =
                            event_permits.as_mut().and_then(|permits| permits.next())
                        {
                            permit.send(ManagerEvent::Connected(device_id));
                            true
                        } else {
                            false
                        };
                        Some((generation, messages, connected_event_sent))
                    }
                };

                let Some((generation, messages, connected_event_sent)) = installed else {
                    continue;
                };

                let _reader_task = spawn_message_reader(
                    device_id,
                    generation,
                    messages,
                    None,
                    event_tx.clone(),
                    connections.clone(),
                    pool.clone(),
                    lifecycle_lock.clone(),
                    qos_registry.clone(),
                );

                if !connected_event_sent {
                    break;
                }
            }
        });

        Ok(())
    }

    pub async fn connect(&mut self, device_id: DeviceId, address: &str) -> Result<()> {
        {
            let _lifecycle = self.lifecycle_lock.lock().await;
            if self.pool.diagnostics_for(&device_id).await.is_some() {
                anyhow::bail!("Already connected to device {}", device_id);
            }
        }

        let mut conn = match self.transport.connect(address, device_id).await {
            Ok(conn) => conn,
            Err(error) => {
                let _ = self
                    .event_tx
                    .send(ManagerEvent::Error {
                        device_id,
                        error: error.to_string(),
                    })
                    .await;
                return Err(error);
            }
        };
        let negotiated = match perform_outbound_handshake(&mut conn, self.local_device_id).await {
            Ok(negotiated) => negotiated,
            Err(error) => {
                conn.reject_pending_peer_identity();
                let _ = self
                    .event_tx
                    .send(ManagerEvent::Error {
                        device_id,
                        error: error.to_string(),
                    })
                    .await;
                return Err(error);
            }
        };
        if negotiated.auth.peer_id != device_id {
            conn.reject_pending_peer_identity();
            anyhow::bail!(
                "QUIC peer identity mismatch: expected {}, got {}",
                device_id,
                negotiated.auth.peer_id
            );
        }

        conn.set_device_id(device_id);
        let auth = Arc::new(negotiated.auth.clone());
        let (qos_transport, releases) = conn.install_qos(auth.clone());
        let messages = conn.message_channel();
        let connected_permit = self.event_tx.reserve().await.ok();
        let generation;
        {
            let _lifecycle = self.lifecycle_lock.lock().await;
            if self.pool.diagnostics_for(&device_id).await.is_some() {
                conn.close().await;
                anyhow::bail!("Already connected to device {}", device_id);
            }
            generation = allocate_generation(&self.next_generation);
            self.qos_registry.insert(
                device_id,
                RegisteredPeer {
                    auth,
                    transport: qos_transport,
                },
            );
            forward_terminal_releases(releases, self.terminal_release_tx.clone());
            self.pool
                .insert_with_generation(device_id, generation, conn)
                .await;
            let mut info = ConnectionInfo::new(device_id, address.to_string());
            info.state = ConnectionState::Connected;
            info.cert_trust_state = Some("trusted".to_string());
            info.control_connection_id = Some(negotiated.auth.control_connection_id);
            self.connections
                .write()
                .expect("canonical connection registry poisoned")
                .insert(device_id, CanonicalConnection { generation, info });
            if let Some(permit) = connected_permit {
                permit.send(ManagerEvent::Connected(device_id));
            }
        }

        let _reader_task = spawn_message_reader(
            device_id,
            generation,
            messages,
            None,
            self.event_tx.clone(),
            self.connections.clone(),
            self.pool.clone(),
            self.lifecycle_lock.clone(),
            self.qos_registry.clone(),
        );
        Ok(())
    }

    pub async fn disconnect(&mut self, device_id: &DeviceId) -> Result<()> {
        let disconnected_permit = self.event_tx.reserve().await.ok();
        let (removed_connection_info, removed_pool_connection) = {
            let _lifecycle = self.lifecycle_lock.lock().await;
            let removed_connection_info = self
                .connections
                .write()
                .expect("canonical connection registry poisoned")
                .remove(device_id);
            if let Some(connection_id) = removed_connection_info
                .as_ref()
                .and_then(|connection| connection.info.control_connection_id)
            {
                self.qos_registry
                    .remove_if_generation(*device_id, connection_id);
            }
            let removed_pool_connection = self.pool.remove(device_id).await;
            (removed_connection_info, removed_pool_connection)
        };

        let removed_pool = removed_pool_connection.is_some();
        if let Some(connection) = removed_pool_connection {
            connection.close().await;
        }
        if removed_connection_info.is_some() || removed_pool {
            if let Some(permit) = disconnected_permit {
                permit.send(ManagerEvent::Disconnected(*device_id));
            }
        }
        Ok(())
    }

    pub async fn send_to(&mut self, device_id: &DeviceId, message: Message) -> Result<()> {
        let classified =
            ClassifiedMessage::try_from(message.clone()).map_err(|error| anyhow::anyhow!(error))?;
        match (self.qos_registry.peer(device_id), classified) {
            (Some(peer), ClassifiedMessage::Control(frame)) => {
                peer.transport.send_control(frame).await?;
            }
            (Some(peer), ClassifiedMessage::Bulk(frame)) => {
                peer.transport.send_bulk(frame).await?;
            }
            (Some(peer), ClassifiedMessage::Telemetry(frame)) => {
                peer.transport
                    .try_send_telemetry(frame)
                    .map_err(|error| anyhow::anyhow!(error))?;
            }
            (Some(peer), ClassifiedMessage::ReliableCompat(frame)) => {
                peer.transport.send_reliable_compat(frame).await?;
            }
            (_, ClassifiedMessage::Unsupported) | (None, _) => {
                self.pool.send_to(device_id, &message).await?;
            }
        }

        if let Some(connection) = self
            .connections
            .write()
            .expect("canonical connection registry poisoned")
            .get_mut(device_id)
        {
            connection.info.messages_sent += 1;
            connection.info.last_activity = Instant::now();
        }

        Ok(())
    }

    pub async fn broadcast(&mut self, message: Message) -> Result<()> {
        match ClassifiedMessage::try_from(message.clone())
            .map_err(|error| anyhow::anyhow!(error))?
        {
            ClassifiedMessage::Control(frame) => {
                ensure_broadcast_success(self.qos_registry.broadcast_control(frame).await)
            }
            ClassifiedMessage::Bulk(frame) => {
                ensure_broadcast_success(self.qos_registry.broadcast_bulk(frame).await)
            }
            ClassifiedMessage::Telemetry(frame) => {
                ensure_broadcast_success(self.qos_registry.broadcast_telemetry(frame))
            }
            ClassifiedMessage::ReliableCompat(frame) => {
                ensure_broadcast_success(self.qos_registry.broadcast_reliable_compat(frame).await)
            }
            ClassifiedMessage::Unsupported => self.pool.broadcast(&message).await,
        }
    }

    pub fn events(&mut self) -> Option<mpsc::Receiver<ManagerEvent>> {
        self.event_rx.take()
    }

    pub fn terminal_release_events(&mut self) -> Option<mpsc::Receiver<TerminalReleaseEvent>> {
        self.terminal_release_rx.take()
    }

    pub fn qos_registry(&self) -> Arc<ConnectionRegistry> {
        self.qos_registry.clone()
    }

    pub(crate) fn connection_view(&self) -> ConnectionView {
        ConnectionView {
            connections: self.connections.clone(),
            pool: self.pool.clone(),
        }
    }

    pub fn get_connection(&self, device_id: &DeviceId) -> Option<ConnectionInfo> {
        self.connections
            .read()
            .expect("canonical connection registry poisoned")
            .get(device_id)
            .map(|connection| connection.info.clone())
    }

    pub fn connections(&self) -> Vec<ConnectionInfo> {
        self.connections
            .read()
            .expect("canonical connection registry poisoned")
            .values()
            .map(|connection| connection.info.clone())
            .collect()
    }

    pub async fn connection_infos(&self) -> Vec<ConnectionInfo> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        collect_connection_infos(&self.connections, &self.pool).await
    }

    pub async fn is_connected(&self, device_id: &DeviceId) -> bool {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.pool.diagnostics_for(device_id).await.is_some()
    }

    pub async fn connected_count(&self) -> usize {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.pool.diagnostics_all().await.len()
    }

    pub async fn cleanup_stale(&mut self, timeout: Duration) -> Vec<DeviceId> {
        let stale: Vec<DeviceId> = self
            .connections
            .read()
            .expect("canonical connection registry poisoned")
            .iter()
            .filter(|(_, connection)| connection.info.is_stale(timeout))
            .map(|(id, _)| *id)
            .collect();

        for id in &stale {
            let _ = self.disconnect(id).await;
        }

        self.pool.cleanup().await;
        stale
    }

    pub fn pool(&self) -> &Arc<ConnectionPool> {
        &self.pool
    }

    pub fn transport_local_addr(&self) -> Option<std::net::SocketAddr> {
        self.transport.local_addr()
    }
}

pub type SharedConnectionManager = Arc<RwLock<ConnectionManager>>;

pub fn create_shared_manager(local_device_id: DeviceId) -> SharedConnectionManager {
    Arc::new(RwLock::new(ConnectionManager::new(local_device_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::QuicTrustStore;
    use rshare_core::{hello_back_message, ScreenInfo};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_trust_store_path(name: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("rshare-{name}-{suffix}"))
            .join("trust.json")
    }

    fn insert_test_canonical(manager: &ConnectionManager, connection_info: ConnectionInfo) -> u64 {
        let device_id = connection_info.device_id;
        let generation = allocate_generation(&manager.next_generation);
        manager
            .connections
            .write()
            .expect("canonical connection registry poisoned")
            .insert(
                device_id,
                CanonicalConnection {
                    generation,
                    info: connection_info,
                },
            );
        generation
    }

    #[test]
    fn test_connection_info() {
        let info = ConnectionInfo::new(DeviceId::new_v4(), "192.168.1.100:27431".to_string());
        assert_eq!(info.state, ConnectionState::Connecting);
    }

    #[tokio::test]
    async fn test_manager_new() {
        let manager = ConnectionManager::new(DeviceId::new_v4());
        assert_eq!(manager.connected_count().await, 0);
    }

    #[tokio::test]
    async fn message_reader_emits_disconnected_when_channel_closes() {
        let device_id = DeviceId::new_v4();
        let mut manager = ConnectionManager::new(DeviceId::new_v4());
        let generation = insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id,
                address: "127.0.0.1:27431".to_string(),
                state: ConnectionState::Connected,
                last_activity: Instant::now(),
                messages_sent: 0,
                messages_received: 0,
                transport: "quic".to_string(),
                datagram_available: false,
                rtt_ms: None,
                last_datagram_rx_ms: None,
                datagram_tx_dropped: 0,
                reliable_stream_reset_count: 0,
                cert_trust_state: None,
                control_connection_id: None,
            },
        );
        let (_message_tx, message_rx) = mpsc::channel(1);
        let mut event_rx = manager.events().unwrap();

        let _reader_task = spawn_message_reader(
            device_id,
            generation,
            message_rx,
            None,
            manager.event_tx.clone(),
            manager.connections.clone(),
            manager.pool.clone(),
            manager.lifecycle_lock.clone(),
            manager.qos_registry.clone(),
        );
        drop(_message_tx);

        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(event, ManagerEvent::Disconnected(id) if id == device_id));
    }

    #[tokio::test]
    async fn first_message_delivery_failure_retires_generation() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let trust_path = temp_trust_store_path("first-message-receiver-closed");

        let mut server = QuicTransport::new(remote_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let accepted = tokio::spawn(async move { incoming.recv().await.unwrap().connection });

        let mut client = QuicTransport::new(local_id).with_trust_store_path(trust_path.clone());
        let mut connection = client
            .connect(&address.to_string(), remote_id)
            .await
            .unwrap();
        let _server_connection = accepted.await.unwrap();
        connection.confirm_peer_identity(remote_id).unwrap();
        connection.set_device_id(remote_id);
        let messages = connection.message_channel();

        let mut manager = ConnectionManager::new(local_id);
        drop(manager.events().unwrap());
        let generation = insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id: remote_id,
                address: address.to_string(),
                state: ConnectionState::Connected,
                last_activity: Instant::now(),
                messages_sent: 0,
                messages_received: 0,
                transport: "quic".to_string(),
                datagram_available: true,
                rtt_ms: Some(1),
                last_datagram_rx_ms: None,
                datagram_tx_dropped: 0,
                reliable_stream_reset_count: 0,
                cert_trust_state: Some("trusted".to_string()),
                control_connection_id: None,
            },
        );
        manager
            .pool
            .insert_with_generation(remote_id, generation, connection)
            .await;

        let _reader_task = spawn_message_reader(
            remote_id,
            generation,
            messages,
            Some(Message::MouseMove { x: 3, y: 5 }),
            manager.event_tx.clone(),
            manager.connections.clone(),
            manager.pool.clone(),
            manager.lifecycle_lock.clone(),
            manager.qos_registry.clone(),
        );

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !is_current_generation(&manager.connections, remote_id, generation)
                    && manager.pool.diagnostics_for(&remote_id).await.is_none()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a reader must retire when its first event cannot be delivered");

        server.close().await.unwrap();
        if let Some(parent) = trust_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn retirement_waiting_for_event_capacity_does_not_hold_lifecycle() {
        let device_id = DeviceId::new_v4();
        let mut manager = ConnectionManager::new(DeviceId::new_v4());
        let generation = insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id,
                address: "127.0.0.1:27431".to_string(),
                state: ConnectionState::Connected,
                last_activity: Instant::now(),
                messages_sent: 0,
                messages_received: 0,
                transport: "quic".to_string(),
                datagram_available: false,
                rtt_ms: None,
                last_datagram_rx_ms: None,
                datagram_tx_dropped: 0,
                reliable_stream_reset_count: 0,
                cert_trust_state: None,
                control_connection_id: None,
            },
        );
        let mut events = manager.events().unwrap();
        while manager
            .event_tx
            .try_send(ManagerEvent::Connected(DeviceId::new_v4()))
            .is_ok()
        {}
        assert_eq!(manager.event_tx.capacity(), 0);

        let lifecycle = manager.lifecycle_lock.lock().await;
        let (message_tx, message_rx) = mpsc::channel(1);
        let _reader_task = spawn_message_reader(
            device_id,
            generation,
            message_rx,
            None,
            manager.event_tx.clone(),
            manager.connections.clone(),
            manager.pool.clone(),
            manager.lifecycle_lock.clone(),
            manager.qos_registry.clone(),
        );
        drop(message_tx);
        tokio::task::yield_now().await;
        drop(lifecycle);
        tokio::task::yield_now().await;

        assert!(
            tokio::time::timeout(Duration::from_millis(50), manager.lifecycle_lock.lock())
                .await
                .is_ok(),
            "event-channel backpressure must not hold the global lifecycle lock"
        );
        tokio::time::timeout(Duration::from_millis(50), manager.connection_infos())
            .await
            .expect("status queries must remain available while retirement waits for capacity");

        events.recv().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(
                    events.recv().await,
                    Some(ManagerEvent::Disconnected(disconnected)) if disconnected == device_id
                ) {
                    break;
                }
            }
        })
        .await
        .expect("the authorized Disconnected event should be queued after capacity is released");
        assert!(!is_current_generation(
            &manager.connections,
            device_id,
            generation
        ));
    }

    #[tokio::test]
    async fn reconnect_old_disconnect_cannot_remove_new_generation() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let old_control_id = ControlConnectionId::new();
        let replacement_control_id = ControlConnectionId::new();
        let trust_path = temp_trust_store_path("reserved-retirement-race");

        let mut server = QuicTransport::new(remote_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut first_client =
            QuicTransport::new(local_id).with_trust_store_path(trust_path.clone());
        let mut old_connection = first_client
            .connect(&address.to_string(), remote_id)
            .await
            .unwrap();
        let _first_server_connection = incoming.recv().await.unwrap().connection;
        old_connection.confirm_peer_identity(remote_id).unwrap();
        old_connection.set_device_id(remote_id);

        let mut second_client =
            QuicTransport::new(local_id).with_trust_store_path(trust_path.clone());
        let mut new_connection = second_client
            .connect(&address.to_string(), remote_id)
            .await
            .unwrap();
        let _second_server_connection = incoming.recv().await.unwrap().connection;
        new_connection.confirm_peer_identity(remote_id).unwrap();
        new_connection.set_device_id(remote_id);

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();
        let old_generation = insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id: remote_id,
                address: address.to_string(),
                state: ConnectionState::Connected,
                last_activity: Instant::now(),
                messages_sent: 0,
                messages_received: 0,
                transport: "quic".to_string(),
                datagram_available: true,
                rtt_ms: Some(1),
                last_datagram_rx_ms: None,
                datagram_tx_dropped: 0,
                reliable_stream_reset_count: 0,
                cert_trust_state: Some("trusted".to_string()),
                control_connection_id: Some(old_control_id),
            },
        );
        manager
            .pool
            .insert_with_generation(remote_id, old_generation, old_connection)
            .await;

        let (old_message_tx, old_message_rx) = mpsc::channel(1);
        let old_reader_task = spawn_message_reader(
            remote_id,
            old_generation,
            old_message_rx,
            None,
            manager.event_tx.clone(),
            manager.connections.clone(),
            manager.pool.clone(),
            manager.lifecycle_lock.clone(),
            manager.qos_registry.clone(),
        );

        let new_generation = allocate_generation(&manager.next_generation);
        manager
            .pool
            .insert_with_generation(remote_id, new_generation, new_connection)
            .await;
        manager
            .connections
            .write()
            .expect("canonical connection registry poisoned")
            .insert(
                remote_id,
                CanonicalConnection {
                    generation: new_generation,
                    info: ConnectionInfo {
                        device_id: remote_id,
                        address: address.to_string(),
                        state: ConnectionState::Connected,
                        last_activity: Instant::now(),
                        messages_sent: 0,
                        messages_received: 0,
                        transport: "quic".to_string(),
                        datagram_available: true,
                        rtt_ms: Some(1),
                        last_datagram_rx_ms: None,
                        datagram_tx_dropped: 0,
                        reliable_stream_reset_count: 0,
                        cert_trust_state: Some("trusted".to_string()),
                        control_connection_id: Some(replacement_control_id),
                    },
                },
            );
        manager
            .event_tx
            .send(ManagerEvent::Connected(remote_id))
            .await
            .unwrap();
        assert!(matches!(
            events.recv().await,
            Some(ManagerEvent::Connected(device_id)) if device_id == remote_id
        ));

        drop(old_message_tx);
        tokio::time::timeout(Duration::from_secs(1), old_reader_task)
            .await
            .expect("the old reader must complete its late retirement attempt")
            .expect("the old reader task must not panic");

        assert!(
            matches!(
                events.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "the old reader must not emit Disconnected for the replacement generation"
        );
        assert!(is_current_generation(
            &manager.connections,
            remote_id,
            new_generation
        ));
        assert!(manager.pool.diagnostics_for(&remote_id).await.is_some());
        assert_eq!(
            manager.pool.generation_for(&remote_id).await,
            Some(new_generation)
        );
        assert_eq!(
            manager
                .connections
                .read()
                .expect("canonical connection registry poisoned")
                .get(&remote_id)
                .expect("replacement generation should remain canonical")
                .info
                .control_connection_id,
            Some(replacement_control_id)
        );
        assert_ne!(old_control_id, replacement_control_id);

        server.close().await.unwrap();
        if let Some(parent) = trust_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn outbound_connect_failure_emits_error_event() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();

        assert!(manager.connect(remote_id, "not-an-addr").await.is_err());

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            event,
            ManagerEvent::Error {
                device_id,
                error: _
            } if device_id == remote_id
        ));
    }

    #[tokio::test]
    async fn explicit_disconnect_emits_disconnected_event() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut manager = ConnectionManager::new(local_id);
        insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id: remote_id,
                address: "127.0.0.1:27431".to_string(),
                state: ConnectionState::Connected,
                last_activity: Instant::now(),
                messages_sent: 0,
                messages_received: 0,
                transport: "quic".to_string(),
                datagram_available: false,
                rtt_ms: None,
                last_datagram_rx_ms: None,
                datagram_tx_dropped: 0,
                reliable_stream_reset_count: 0,
                cert_trust_state: None,
                control_connection_id: None,
            },
        );
        let mut events = manager.events().unwrap();

        manager.disconnect(&remote_id).await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(event, ManagerEvent::Disconnected(id) if id == remote_id));
    }

    #[tokio::test]
    async fn explicit_disconnect_closes_quic_and_retires_remote_reader() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let trust_path = temp_trust_store_path("explicit-disconnect-close");
        let mut remote = ConnectionManager::with_transport(
            remote_id,
            QuicTransport::new(remote_id).with_trust_store_path(trust_path.clone()),
        );
        let mut remote_events = remote.events().unwrap();
        remote.start_server("127.0.0.1:0").await.unwrap();
        let address = remote.transport_local_addr().unwrap();

        let mut local = ConnectionManager::with_transport(
            local_id,
            QuicTransport::new(local_id).with_trust_store_path(trust_path.clone()),
        );
        local
            .connect(remote_id, &address.to_string())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(
                    remote_events.recv().await,
                    Some(ManagerEvent::Connected(id)) if id == local_id
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap();

        local.disconnect(&remote_id).await.unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(
                    remote_events.recv().await,
                    Some(ManagerEvent::Disconnected(id)) if id == local_id
                ) {
                    break;
                }
            }
        })
        .await
        .expect("explicit disconnect must close QUIC and retire the remote reader");
        assert!(!remote.is_connected(&local_id).await);

        if let Some(parent) = trust_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn connected_entry_without_active_transport_is_reported_disconnected() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let manager = ConnectionManager::new(local_id);
        insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id: remote_id,
                address: "127.0.0.1:27431".to_string(),
                state: ConnectionState::Connected,
                last_activity: Instant::now(),
                messages_sent: 0,
                messages_received: 0,
                transport: "quic".to_string(),
                datagram_available: true,
                rtt_ms: Some(1),
                last_datagram_rx_ms: Some(1),
                datagram_tx_dropped: 0,
                reliable_stream_reset_count: 0,
                cert_trust_state: None,
                control_connection_id: None,
            },
        );

        let infos = manager.connection_infos().await;
        let info = infos
            .iter()
            .find(|info| info.device_id == remote_id)
            .expect("connection info should remain visible");

        assert_eq!(info.state, ConnectionState::Disconnected);
        assert!(!info.datagram_available);
        assert_eq!(info.rtt_ms, None);
        assert!(!manager.is_connected(&remote_id).await);
    }

    #[tokio::test]
    async fn closed_pool_transport_is_not_counted_before_reader_retirement() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let trust_path = temp_trust_store_path("closed-count");

        let mut server = QuicTransport::new(remote_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let accepted = tokio::spawn(async move { incoming.recv().await.unwrap().connection });

        let mut client = QuicTransport::new(local_id).with_trust_store_path(trust_path.clone());
        let mut connection = client
            .connect(&address.to_string(), remote_id)
            .await
            .unwrap();
        let _server_connection = accepted.await.unwrap();
        connection.confirm_peer_identity(remote_id).unwrap();
        connection.set_device_id(remote_id);
        let _held_messages = connection.message_channel();

        let manager = ConnectionManager::new(local_id);
        let generation = allocate_generation(&manager.next_generation);
        manager
            .pool
            .insert_with_generation(remote_id, generation, connection)
            .await;
        manager
            .connections
            .write()
            .expect("canonical connection registry poisoned")
            .insert(
                remote_id,
                CanonicalConnection {
                    generation,
                    info: ConnectionInfo {
                        device_id: remote_id,
                        address: address.to_string(),
                        state: ConnectionState::Connected,
                        last_activity: Instant::now(),
                        messages_sent: 0,
                        messages_received: 0,
                        transport: "quic".to_string(),
                        datagram_available: true,
                        rtt_ms: Some(1),
                        last_datagram_rx_ms: None,
                        datagram_tx_dropped: 0,
                        reliable_stream_reset_count: 0,
                        cert_trust_state: Some("trusted".to_string()),
                        control_connection_id: None,
                    },
                },
            );

        server.close().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if manager.pool.diagnostics_for(&remote_id).await.is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the closed QUIC transport should become inactive");

        assert!(!manager.is_connected(&remote_id).await);
        assert_eq!(manager.connected_count().await, 0);
        assert!(manager
            .connection_infos()
            .await
            .iter()
            .all(|info| info.device_id != remote_id || info.state != ConnectionState::Connected));

        if let Some(parent) = trust_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn manager_emits_message_received_for_connected_device() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut remote_manager = ConnectionManager::new(remote_id);
        let mut remote_events = remote_manager.events().unwrap();
        remote_manager.start_server("127.0.0.1:0").await.unwrap();
        let address = remote_manager.transport_local_addr().unwrap();

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();
        manager
            .connect(remote_id, &address.to_string())
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::Connected(device_id) = remote_events.recv().await.unwrap() {
                    assert_eq!(device_id, local_id);
                    break;
                }
            }
        })
        .await
        .unwrap();

        remote_manager
            .send_to(&local_id, Message::MouseMove { x: 7, y: 9 })
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match events.recv().await.unwrap() {
                    ManagerEvent::MessageReceived { from, message } => {
                        break (from, message);
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(received.0, remote_id);
        assert!(matches!(received.1, Message::MouseMove { x: 7, y: 9 }));
    }

    #[tokio::test]
    async fn manager_emits_message_received_for_incoming_connection() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();
        manager.start_server("127.0.0.1:0").await.unwrap();
        let address = manager.transport_local_addr().unwrap();

        let mut remote_manager = ConnectionManager::new(remote_id);
        remote_manager
            .connect(local_id, &address.to_string())
            .await
            .unwrap();
        remote_manager
            .send_to(&local_id, Message::MouseMove { x: 11, y: 13 })
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match events.recv().await.unwrap() {
                    ManagerEvent::MessageReceived { from, message } => {
                        break (from, message);
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();

        assert!(matches!(received.1, Message::MouseMove { x: 11, y: 13 }));

        let connection_infos = manager.connection_infos().await;
        assert!(connection_infos.iter().any(|info| {
            info.device_id == remote_id
                && info.state == ConnectionState::Connected
                && info.transport == "quic"
                && info.datagram_available
        }));
        assert!(manager.is_connected(&remote_id).await);

        manager.disconnect(&remote_id).await.unwrap();
        assert!(!manager.is_connected(&remote_id).await);
        assert!(manager.pool.diagnostics_for(&remote_id).await.is_none());
    }

    #[tokio::test]
    async fn incoming_hello_binds_connection_to_remote_device_id() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();
        manager.start_server("127.0.0.1:0").await.unwrap();
        let address = manager.transport_local_addr().unwrap();

        let mut remote_manager = ConnectionManager::new(remote_id);
        remote_manager
            .connect(local_id, &address.to_string())
            .await
            .unwrap();

        let connected = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::Connected(device_id) = events.recv().await.unwrap() {
                    break device_id;
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(connected, remote_id);
    }

    #[tokio::test]
    async fn incoming_duplicate_keeps_live_canonical_connection() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let trust_path = temp_trust_store_path("incoming-duplicate");

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();
        manager.start_server("127.0.0.1:0").await.unwrap();
        let address = manager.transport_local_addr().unwrap();

        let mut first_peer = ConnectionManager::with_transport(
            remote_id,
            QuicTransport::new(remote_id).with_trust_store_path(trust_path.clone()),
        );
        first_peer
            .connect(local_id, &address.to_string())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(
                    events.recv().await,
                    Some(ManagerEvent::Connected(device_id)) if device_id == remote_id
                ) {
                    break;
                }
            }
        })
        .await
        .expect("the first inbound connection should become canonical");
        let first_generation = manager
            .connections
            .read()
            .expect("canonical connection registry poisoned")
            .get(&remote_id)
            .expect("the first inbound connection should be present")
            .generation;

        let mut duplicate_peer = ConnectionManager::with_transport(
            remote_id,
            QuicTransport::new(remote_id).with_trust_store_path(trust_path.clone()),
        );
        duplicate_peer
            .connect(local_id, &address.to_string())
            .await
            .unwrap();

        let duplicate_connected = tokio::time::timeout(Duration::from_millis(150), async {
            loop {
                if matches!(
                    events.recv().await,
                    Some(ManagerEvent::Connected(device_id)) if device_id == remote_id
                ) {
                    break;
                }
            }
        })
        .await;
        assert!(
            duplicate_connected.is_err(),
            "a live canonical connection must not be replaced by a duplicate inbound peer"
        );
        assert_eq!(
            manager
                .connections
                .read()
                .expect("canonical connection registry poisoned")
                .get(&remote_id)
                .expect("the first inbound connection should remain present")
                .generation,
            first_generation
        );

        first_peer
            .send_to(&local_id, Message::MouseMove { x: 31, y: 37 })
            .await
            .unwrap();
        let received = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(ManagerEvent::MessageReceived { from, message }) = events.recv().await {
                    break (from, message);
                }
            }
        })
        .await
        .expect("the original inbound connection should remain usable");
        assert_eq!(received.0, remote_id);
        assert!(matches!(received.1, Message::MouseMove { x: 31, y: 37 }));

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !duplicate_peer.is_connected(&local_id).await {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the duplicate inbound transport should be closed");

        if let Some(parent) = trust_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn outbound_connect_accepts_hello_back_identity() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut remote_manager = ConnectionManager::new(remote_id);
        remote_manager.start_server("127.0.0.1:0").await.unwrap();
        let address = remote_manager.transport_local_addr().unwrap();

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();
        manager
            .connect(remote_id, &address.to_string())
            .await
            .unwrap();

        let connected = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::Connected(device_id) = events.recv().await.unwrap() {
                    break device_id;
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(connected, remote_id);
        assert!(manager.is_connected(&remote_id).await);
    }

    #[tokio::test]
    async fn reconnects_after_live_transport_closes() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let trust_path = temp_trust_store_path("reconnect");

        let mut first_remote = ConnectionManager::new(remote_id);
        first_remote.start_server("127.0.0.1:0").await.unwrap();
        let first_address = first_remote.transport_local_addr().unwrap();

        let mut manager = ConnectionManager::with_transport(
            local_id,
            QuicTransport::new(local_id).with_trust_store_path(trust_path.clone()),
        );
        let mut events = manager.events().unwrap();
        manager
            .connect(remote_id, &first_address.to_string())
            .await
            .unwrap();
        assert!(manager.is_connected(&remote_id).await);

        // Keep a reader from the first canonical connection alive until after the
        // replacement is installed. Its eventual close models a late old-reader
        // notification that must not disconnect the replacement generation.
        let first_generation = manager
            .connections
            .read()
            .expect("canonical connection registry poisoned")
            .get(&remote_id)
            .expect("the first connection should be canonical")
            .generation;
        let (late_old_reader_tx, late_old_reader_rx) = mpsc::channel(1);
        let _reader_task = spawn_message_reader(
            remote_id,
            first_generation,
            late_old_reader_rx,
            None,
            manager.event_tx.clone(),
            manager.connections.clone(),
            manager.pool.clone(),
            manager.lifecycle_lock.clone(),
            manager.qos_registry.clone(),
        );

        first_remote.transport.close().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if matches!(
                    events.recv().await,
                    Some(ManagerEvent::Disconnected(device_id)) if device_id == remote_id
                ) {
                    break;
                }
            }
        })
        .await
        .expect("the first connection reader should report its closed transport");
        assert!(!manager.is_connected(&remote_id).await);

        let mut replacement_remote = ConnectionManager::new(remote_id);
        replacement_remote
            .start_server("127.0.0.1:0")
            .await
            .unwrap();
        let replacement_address = replacement_remote.transport_local_addr().unwrap();

        manager
            .connect(remote_id, &replacement_address.to_string())
            .await
            .expect("a closed canonical connection must not block reconnect");

        assert!(manager.is_connected(&remote_id).await);
        assert_eq!(manager.connected_count().await, 1);

        let replacement_connected = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(ManagerEvent::Connected(device_id)) = events.recv().await {
                    if device_id == remote_id {
                        break device_id;
                    }
                }
            }
        })
        .await
        .expect("the replacement connection should emit Connected");
        assert_eq!(replacement_connected, remote_id);

        drop(late_old_reader_tx);
        let stale_disconnect = tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                match events.recv().await {
                    Some(ManagerEvent::Disconnected(device_id)) if device_id == remote_id => {
                        break Some(device_id);
                    }
                    Some(_) => {}
                    None => break None,
                }
            }
        })
        .await;
        assert!(
            stale_disconnect.is_err(),
            "a stale reader must not emit Disconnected for the replacement generation"
        );
        assert!(manager.is_connected(&remote_id).await);

        let infos = manager.connection_infos().await;
        assert_eq!(
            infos
                .iter()
                .filter(|info| {
                    info.device_id == remote_id && info.state == ConnectionState::Connected
                })
                .count(),
            1
        );

        if let Some(parent) = trust_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn outbound_connect_rejects_mismatched_hello_back_without_pinning() {
        let local_id = DeviceId::new_v4();
        let expected_id = DeviceId::new_v4();
        let returned_id = DeviceId::new_v4();
        let trust_path = temp_trust_store_path("hello-back-mismatch");

        let mut server = QuicTransport::new(returned_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let responder = tokio::spawn(async move {
            let mut connection = incoming.recv().await.unwrap().connection;
            assert!(matches!(
                connection.receive_message().await.unwrap(),
                Message::Hello { .. }
            ));
            connection
                .send_message(&hello_back_message(
                    returned_id,
                    "unexpected".to_string(),
                    "unexpected-host".to_string(),
                    ScreenInfo::primary(),
                ))
                .await
                .unwrap();
        });

        let mut manager = ConnectionManager::with_transport(
            local_id,
            QuicTransport::new(local_id).with_trust_store_path(trust_path.clone()),
        );
        let error = manager
            .connect(expected_id, &address.to_string())
            .await
            .expect_err("mismatched HelloBack identity must fail");

        assert!(error.to_string().contains("identity mismatch"));
        assert!(!manager.is_connected(&expected_id).await);
        assert!(QuicTrustStore::load(&trust_path)
            .unwrap()
            .fingerprint_for(&expected_id)
            .is_none());
        assert!(QuicTrustStore::load(&trust_path)
            .unwrap()
            .fingerprint_for(&returned_id)
            .is_none());

        responder.await.unwrap();
        server.close().await.unwrap();
        if let Some(parent) = trust_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn outbound_connect_rejects_unavailable_identity_without_pinning() {
        let local_id = DeviceId::new_v4();
        let expected_id = DeviceId::new_v4();
        let trust_path = temp_trust_store_path("hello-back-unavailable");

        let mut server = QuicTransport::new(expected_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let responder = tokio::spawn(async move {
            let mut connection = incoming.recv().await.unwrap().connection;
            assert!(matches!(
                connection.receive_message().await.unwrap(),
                Message::Hello { .. }
            ));
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        let mut manager = ConnectionManager::with_transport(
            local_id,
            QuicTransport::new(local_id).with_trust_store_path(trust_path.clone()),
        );
        let error = manager
            .connect(expected_id, &address.to_string())
            .await
            .expect_err("missing HelloBack identity must fail");

        assert!(error.to_string().contains("identity handshake timed out"));
        assert!(!manager.is_connected(&expected_id).await);
        assert!(QuicTrustStore::load(&trust_path)
            .unwrap()
            .fingerprint_for(&expected_id)
            .is_none());

        responder.await.unwrap();
        server.close().await.unwrap();
        if let Some(parent) = trust_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}
