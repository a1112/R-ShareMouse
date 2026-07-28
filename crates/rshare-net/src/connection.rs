//! Connection management

use anyhow::Result;
use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as TokioMutex, RwLock};
use tokio::time::Instant;

use rshare_core::{ControlConnectionId, DeviceId, Message};

use super::handshake::{perform_outbound_handshake, receive_incoming_handshake};
use super::qos::{
    ClassifiedMessage, ConnectionRegistry, ControlFrame, RegisteredPeer, TerminalReleaseEvent,
    TransportSendError,
};
use super::transport::{ConnectionPool, PeerInbound, QuicTransport, TransportProtocolError};

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
    Connected(super::handshake::PeerAuthContext),
    Disconnected {
        peer_id: DeviceId,
        control_connection_id: ControlConnectionId,
    },
    MessageReceived {
        from: DeviceId,
        message: Message,
    },
    ControlReceived {
        auth: Arc<super::handshake::PeerAuthContext>,
        frame: ControlFrame,
    },
    ProtocolError {
        auth: Arc<super::handshake::PeerAuthContext>,
        error: String,
    },
    Error {
        peer_id: Option<DeviceId>,
        control_connection_id: Option<ControlConnectionId>,
        error: String,
    },
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
        collect_connection_infos(&self.connections, &self.pool)
    }

    pub(crate) async fn is_connected(&self, device_id: &DeviceId) -> bool {
        self.pool.diagnostics_for_now(device_id).is_some()
    }

    pub(crate) async fn send_legacy(&self, device_id: &DeviceId, message: &Message) -> Result<()> {
        let selected_identity = self.pool.send_to_with_identity(device_id, message).await?;
        if let Some(connection) = self
            .connections
            .write()
            .expect("canonical connection registry poisoned")
            .get_mut(device_id)
            .filter(|connection| {
                connection.generation == selected_identity.lifecycle_generation
                    && connection.info.control_connection_id
                        == selected_identity.control_connection_id
            })
        {
            connection.info.messages_sent += 1;
            connection.info.last_activity = Instant::now();
        }
        Ok(())
    }

    pub(crate) async fn broadcast_legacy(&self, message: &Message) -> Result<()> {
        self.pool.broadcast(message).await
    }

    pub(crate) fn record_send_success(
        &self,
        device_id: &DeviceId,
        generation: ControlConnectionId,
    ) {
        if let Some(connection) = self
            .connections
            .write()
            .expect("canonical connection registry poisoned")
            .get_mut(device_id)
            .filter(|connection| connection.info.control_connection_id == Some(generation))
        {
            connection.info.messages_sent += 1;
            connection.info.last_activity = Instant::now();
        }
    }

    #[cfg(test)]
    pub(crate) async fn pool_generation_for_test(&self, device_id: &DeviceId) -> u64 {
        self.pool
            .generation_for(device_id)
            .await
            .expect("test peer must have a pooled lifecycle generation")
    }

    #[cfg(test)]
    pub(crate) async fn replace_pool_generation_and_outbound_for_test(
        &self,
        device_id: DeviceId,
        lifecycle_generation: u64,
        control_connection_id: Option<ControlConnectionId>,
        send_channel: mpsc::Sender<crate::transport::OutboundFrame>,
    ) {
        self.pool
            .replace_generation_and_outbound_for_test(
                device_id,
                lifecycle_generation,
                control_connection_id,
                send_channel,
            )
            .await;
    }

    #[cfg(test)]
    pub(crate) fn replace_canonical_identity_for_test(
        &self,
        device_id: DeviceId,
        lifecycle_generation: u64,
        control_connection_id: Option<ControlConnectionId>,
    ) {
        let mut connections = self
            .connections
            .write()
            .expect("canonical connection registry poisoned");
        let connection = connections
            .get_mut(&device_id)
            .expect("test peer must have canonical connection state");
        connection.generation = lifecycle_generation;
        connection.info.control_connection_id = control_connection_id;
        connection.info.messages_sent = 0;
    }
}

fn collect_connection_infos(
    connections: &CanonicalConnections,
    pool: &ConnectionPool,
) -> Vec<ConnectionInfo> {
    let mut infos_by_id: HashMap<_, _> = connections
        .read()
        .expect("canonical connection registry poisoned")
        .iter()
        .map(|(device_id, connection)| (*device_id, connection.info.clone()))
        .collect();
    let active_diagnostics = pool.diagnostics_all_now();
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
    authenticated_peer_tx: mpsc::Sender<PeerInbound>,
    authenticated_peer_rx: Option<mpsc::Receiver<PeerInbound>>,
    #[cfg(test)]
    authenticated_peer_overflow_barrier: Option<Arc<tokio::sync::Notify>>,
    #[cfg(test)]
    authenticated_peer_overflow_waiting: Arc<AtomicBool>,
    terminal_release_tx: mpsc::Sender<TerminalReleaseEvent>,
    terminal_release_rx: Option<mpsc::Receiver<TerminalReleaseEvent>>,
}

fn spawn_control_event_reader(
    auth: Arc<super::handshake::PeerAuthContext>,
    generation: u64,
    mut controls: mpsc::Receiver<ControlFrame>,
    event_tx: mpsc::Sender<ManagerEvent>,
    connections: CanonicalConnections,
    lifecycle_lock: Arc<TokioMutex<()>>,
) {
    tokio::spawn(async move {
        while let Some(frame) = controls.recv().await {
            let permit = event_tx.reserve().await.ok();
            let _lifecycle = lifecycle_lock.lock().await;
            if !is_current_generation(&connections, auth.peer_id, generation)
                || !connections
                    .read()
                    .expect("canonical connection registry poisoned")
                    .get(&auth.peer_id)
                    .is_some_and(|connection| {
                        connection.info.control_connection_id == Some(auth.control_connection_id)
                    })
            {
                return;
            }
            let Some(permit) = permit else {
                return;
            };
            permit.send(ManagerEvent::ControlReceived {
                auth: auth.clone(),
                frame,
            });
        }
    });
}

fn spawn_protocol_error_reader(
    generation: u64,
    mut errors: mpsc::Receiver<TransportProtocolError>,
    event_tx: mpsc::Sender<ManagerEvent>,
    connections: CanonicalConnections,
    lifecycle_lock: Arc<TokioMutex<()>>,
) {
    tokio::spawn(async move {
        while let Some(error) = errors.recv().await {
            let permit = event_tx.reserve().await.ok();
            let _lifecycle = lifecycle_lock.lock().await;
            if !is_current_generation(&connections, error.auth.peer_id, generation)
                || !connections
                    .read()
                    .expect("canonical connection registry poisoned")
                    .get(&error.auth.peer_id)
                    .is_some_and(|connection| {
                        connection.info.control_connection_id
                            == Some(error.auth.control_connection_id)
                    })
            {
                return;
            }
            let Some(permit) = permit else {
                return;
            };
            permit.send(ManagerEvent::ProtocolError {
                auth: error.auth,
                error: error.error,
            });
        }
    });
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
        pool.remove_generation_now(&device_id, generation);
        if let Some(permit) = permit {
            if let Some(control_connection_id) = connection_id {
                permit.send(ManagerEvent::Disconnected {
                    peer_id: device_id,
                    control_connection_id,
                });
            }
        }
    }
    removed_current
}

async fn fail_authenticated_publication(
    connections: &CanonicalConnections,
    pool: &ConnectionPool,
    lifecycle_lock: &TokioMutex<()>,
    event_tx: &mpsc::Sender<ManagerEvent>,
    auth: &super::handshake::PeerAuthContext,
    lifecycle_generation: u64,
    qos_registry: &ConnectionRegistry,
    reason: &'static str,
) -> bool {
    let removed_connection = {
        let _lifecycle = lifecycle_lock.lock().await;
        let removed_current = connections
            .read()
            .expect("canonical connection registry poisoned")
            .get(&auth.peer_id)
            .is_some_and(|connection| {
                connection.generation == lifecycle_generation
                    && connection.info.control_connection_id == Some(auth.control_connection_id)
            });
        if !removed_current {
            None
        } else {
            connections
                .write()
                .expect("canonical connection registry poisoned")
                .remove(&auth.peer_id);
            qos_registry.remove_if_generation(auth.peer_id, auth.control_connection_id);
            Some(pool.remove_generation_now(&auth.peer_id, lifecycle_generation))
        }
    };

    let Some(connection) = removed_connection else {
        return false;
    };
    if let Some(connection) = connection {
        connection.close().await;
    }
    let _ = event_tx
        .send(ManagerEvent::Error {
            peer_id: Some(auth.peer_id),
            control_connection_id: Some(auth.control_connection_id),
            error: reason.to_string(),
        })
        .await;
    true
}

impl ConnectionManager {
    pub fn new(local_device_id: DeviceId) -> Self {
        Self::with_transport(local_device_id, QuicTransport::new(local_device_id))
    }

    #[cfg(test)]
    pub(crate) fn isolated_for_test(local_device_id: DeviceId) -> Self {
        Self::with_transport(
            local_device_id,
            QuicTransport::isolated_for_test(local_device_id),
        )
    }

    pub fn with_transport(local_device_id: DeviceId, mut transport: QuicTransport) -> Self {
        let (event_tx, event_rx) = mpsc::channel(100);
        let (authenticated_peer_tx, authenticated_peer_rx) = mpsc::channel(32);
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
            authenticated_peer_tx,
            authenticated_peer_rx: Some(authenticated_peer_rx),
            #[cfg(test)]
            authenticated_peer_overflow_barrier: None,
            #[cfg(test)]
            authenticated_peer_overflow_waiting: Arc::new(AtomicBool::new(false)),
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
        let authenticated_peer_tx = self.authenticated_peer_tx.clone();
        #[cfg(test)]
        let authenticated_peer_overflow_barrier = self.authenticated_peer_overflow_barrier.clone();
        #[cfg(test)]
        let authenticated_peer_overflow_waiting = self.authenticated_peer_overflow_waiting.clone();
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
                let auth = Arc::new(negotiated.auth.clone());
                let mut candidate_connection = Some(incoming.connection);
                let (qos_transport, releases) = candidate_connection
                    .as_mut()
                    .expect("incoming connection candidate must be present")
                    .install_qos(auth.clone());
                let peer_inbound = candidate_connection
                    .as_mut()
                    .expect("incoming connection candidate must be present")
                    .take_peer_inbound()
                    .expect("authenticated QoS install must expose peer inbound");
                let control_events = candidate_connection
                    .as_mut()
                    .expect("incoming connection candidate must be present")
                    .take_control_events()
                    .expect("authenticated QoS install must expose control events");
                let protocol_errors = candidate_connection
                    .as_mut()
                    .expect("incoming connection candidate must be present")
                    .take_protocol_errors()
                    .expect("authenticated QoS install must expose protocol errors");
                let messages = candidate_connection
                    .as_mut()
                    .expect("incoming connection candidate must be present")
                    .message_channel();

                let installed = {
                    let _lifecycle = lifecycle_lock.lock().await;
                    if pool.diagnostics_for_now(&device_id).is_some() {
                        None
                    } else {
                        let removed_canonical = connections
                            .write()
                            .expect("canonical connection registry poisoned")
                            .remove(&device_id);
                        let removed_pool = pool.remove_now(&device_id).is_some();
                        let stale_control_connection_id = removed_canonical
                            .as_ref()
                            .and_then(|connection| connection.info.control_connection_id);
                        if stale_control_connection_id.is_none() && removed_pool {
                            tracing::debug!(
                                "Removed stale pool entry without authenticated generation for {}",
                                device_id
                            );
                        }

                        let generation = allocate_generation(&next_generation);
                        qos_registry.insert(
                            device_id,
                            RegisteredPeer {
                                auth: auth.clone(),
                                transport: qos_transport,
                            },
                        );
                        forward_terminal_releases(releases, terminal_release_tx.clone());
                        pool.insert_with_generation_now(
                            device_id,
                            generation,
                            candidate_connection
                                .take()
                                .expect("incoming connection candidate must be present"),
                        );
                        let mut info = ConnectionInfo::new(device_id, address);
                        info.state = ConnectionState::Connected;
                        info.cert_trust_state = Some("trusted".to_string());
                        info.control_connection_id = Some(negotiated.auth.control_connection_id);
                        connections
                            .write()
                            .expect("canonical connection registry poisoned")
                            .insert(device_id, CanonicalConnection { generation, info });
                        Some((generation, stale_control_connection_id))
                    }
                };

                let Some((generation, stale_control_connection_id)) = installed else {
                    candidate_connection
                        .take()
                        .expect("duplicate incoming connection candidate must be present")
                        .close()
                        .await;
                    continue;
                };

                if let Err(error) = authenticated_peer_tx.try_send(peer_inbound) {
                    #[cfg(test)]
                    if matches!(&error, mpsc::error::TrySendError::Full(_)) {
                        if let Some(barrier) = authenticated_peer_overflow_barrier.as_ref() {
                            authenticated_peer_overflow_waiting.store(true, Ordering::Release);
                            barrier.notified().await;
                            authenticated_peer_overflow_waiting.store(false, Ordering::Release);
                        }
                    }
                    let reason = match error {
                        mpsc::error::TrySendError::Full(_) => {
                            "authenticated peer receiver queue is full"
                        }
                        mpsc::error::TrySendError::Closed(_) => {
                            "authenticated peer receiver queue is closed"
                        }
                    };
                    fail_authenticated_publication(
                        &connections,
                        &pool,
                        &lifecycle_lock,
                        &event_tx,
                        &auth,
                        generation,
                        &qos_registry,
                        reason,
                    )
                    .await;
                    continue;
                }

                spawn_control_event_reader(
                    auth.clone(),
                    generation,
                    control_events,
                    event_tx.clone(),
                    connections.clone(),
                    lifecycle_lock.clone(),
                );
                spawn_protocol_error_reader(
                    generation,
                    protocol_errors,
                    event_tx.clone(),
                    connections.clone(),
                    lifecycle_lock.clone(),
                );
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

                let mut event_permits = event_tx.reserve_many(2).await.ok();
                if let Some(control_connection_id) = stale_control_connection_id {
                    if let Some(permit) = event_permits.as_mut().and_then(|permits| permits.next())
                    {
                        permit.send(ManagerEvent::Disconnected {
                            peer_id: device_id,
                            control_connection_id,
                        });
                    }
                }
                let connected_event_sent = if let Some(permit) =
                    event_permits.as_mut().and_then(|permits| permits.next())
                {
                    permit.send(ManagerEvent::Connected(negotiated.auth.clone()));
                    true
                } else {
                    false
                };
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
            if self.pool.diagnostics_for_now(&device_id).is_some() {
                anyhow::bail!("Already connected to device {}", device_id);
            }
        }

        let mut conn = match self.transport.connect(address, device_id).await {
            Ok(conn) => conn,
            Err(error) => {
                let _ = self
                    .event_tx
                    .send(ManagerEvent::Error {
                        peer_id: Some(device_id),
                        control_connection_id: None,
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
                        peer_id: Some(device_id),
                        control_connection_id: None,
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
        let peer_inbound = conn
            .take_peer_inbound()
            .expect("authenticated QoS install must expose peer inbound");
        let control_events = conn
            .take_control_events()
            .expect("authenticated QoS install must expose control events");
        let protocol_errors = conn
            .take_protocol_errors()
            .expect("authenticated QoS install must expose protocol errors");
        let messages = conn.message_channel();
        let mut candidate_connection = Some(conn);
        let installation = {
            let _lifecycle = self.lifecycle_lock.lock().await;
            if self.pool.diagnostics_for_now(&device_id).is_some() {
                Err(candidate_connection
                    .take()
                    .expect("outbound connection candidate must be present"))
            } else {
                let generation = allocate_generation(&self.next_generation);
                self.qos_registry.insert(
                    device_id,
                    RegisteredPeer {
                        auth: auth.clone(),
                        transport: qos_transport,
                    },
                );
                forward_terminal_releases(releases, self.terminal_release_tx.clone());
                self.pool.insert_with_generation_now(
                    device_id,
                    generation,
                    candidate_connection
                        .take()
                        .expect("outbound connection candidate must be present"),
                );
                let mut info = ConnectionInfo::new(device_id, address.to_string());
                info.state = ConnectionState::Connected;
                info.cert_trust_state = Some("trusted".to_string());
                info.control_connection_id = Some(negotiated.auth.control_connection_id);
                self.connections
                    .write()
                    .expect("canonical connection registry poisoned")
                    .insert(device_id, CanonicalConnection { generation, info });
                Ok(generation)
            }
        };
        let generation = match installation {
            Ok(generation) => generation,
            Err(connection) => {
                connection.close().await;
                anyhow::bail!("Already connected to device {}", device_id);
            }
        };

        if let Err(error) = self.authenticated_peer_tx.try_send(peer_inbound) {
            let reason = match error {
                mpsc::error::TrySendError::Full(_) => "authenticated peer receiver queue is full",
                mpsc::error::TrySendError::Closed(_) => {
                    "authenticated peer receiver queue is closed"
                }
            };
            fail_authenticated_publication(
                &self.connections,
                &self.pool,
                &self.lifecycle_lock,
                &self.event_tx,
                &auth,
                generation,
                &self.qos_registry,
                reason,
            )
            .await;
            anyhow::bail!("{reason}");
        }

        spawn_control_event_reader(
            auth.clone(),
            generation,
            control_events,
            self.event_tx.clone(),
            self.connections.clone(),
            self.lifecycle_lock.clone(),
        );
        spawn_protocol_error_reader(
            generation,
            protocol_errors,
            self.event_tx.clone(),
            self.connections.clone(),
            self.lifecycle_lock.clone(),
        );
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
        if let Some(permit) = self.event_tx.reserve().await.ok() {
            permit.send(ManagerEvent::Connected(negotiated.auth.clone()));
        }
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
            let removed_pool_connection = self.pool.remove_now(device_id);
            (removed_connection_info, removed_pool_connection)
        };

        let removed_pool = removed_pool_connection.is_some();
        if let Some(connection) = removed_pool_connection {
            connection.close().await;
        }
        if removed_connection_info.is_some() || removed_pool {
            if let (Some(permit), Some(control_connection_id)) = (
                disconnected_permit,
                removed_connection_info
                    .as_ref()
                    .and_then(|connection| connection.info.control_connection_id),
            ) {
                permit.send(ManagerEvent::Disconnected {
                    peer_id: *device_id,
                    control_connection_id,
                });
            }
        }
        Ok(())
    }

    pub async fn send_to(&mut self, device_id: &DeviceId, message: Message) -> Result<()> {
        enum SelectedSendIdentity {
            Control(ControlConnectionId),
            Pool(crate::transport::PooledSendIdentity),
        }

        let classified =
            ClassifiedMessage::try_from(message.clone()).map_err(|error| anyhow::anyhow!(error))?;
        let selected_identity = match (self.qos_registry.peer(device_id), classified) {
            (Some(peer), ClassifiedMessage::Control(frame)) => {
                peer.transport.send_control(frame).await?;
                SelectedSendIdentity::Control(peer.auth.control_connection_id)
            }
            (Some(peer), ClassifiedMessage::Bulk(frame)) => {
                peer.transport.send_bulk(frame).await?;
                SelectedSendIdentity::Control(peer.auth.control_connection_id)
            }
            (Some(peer), ClassifiedMessage::Telemetry(frame)) => {
                peer.transport.try_send_telemetry(frame)?;
                SelectedSendIdentity::Control(peer.auth.control_connection_id)
            }
            (Some(peer), ClassifiedMessage::ReliableCompat(frame)) => {
                peer.transport.send_reliable_compat(frame).await?;
                SelectedSendIdentity::Control(peer.auth.control_connection_id)
            }
            (_, ClassifiedMessage::Unsupported) | (None, _) => SelectedSendIdentity::Pool(
                self.pool.send_to_with_identity(device_id, &message).await?,
            ),
        };

        if let Some(connection) = self
            .connections
            .write()
            .expect("canonical connection registry poisoned")
            .get_mut(device_id)
            .filter(|connection| match selected_identity {
                SelectedSendIdentity::Control(control_connection_id) => {
                    connection.info.control_connection_id == Some(control_connection_id)
                }
                SelectedSendIdentity::Pool(identity) => {
                    connection.generation == identity.lifecycle_generation
                        && connection.info.control_connection_id == identity.control_connection_id
                }
            })
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

    pub fn authenticated_peers(&mut self) -> Option<mpsc::Receiver<PeerInbound>> {
        self.authenticated_peer_rx.take()
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
        collect_connection_infos(&self.connections, &self.pool)
    }

    pub async fn is_connected(&self, device_id: &DeviceId) -> bool {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.pool.diagnostics_for_now(device_id).is_some()
    }

    pub async fn connected_count(&self) -> usize {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.pool.diagnostics_all_now().len()
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

    #[cfg(test)]
    pub(crate) fn set_authenticated_peer_overflow_barrier(
        &mut self,
        barrier: Arc<tokio::sync::Notify>,
    ) {
        self.authenticated_peer_overflow_barrier = Some(barrier);
    }

    #[cfg(test)]
    pub(crate) fn authenticated_peer_overflow_waiting(&self) -> bool {
        self.authenticated_peer_overflow_waiting
            .load(Ordering::Acquire)
    }
}

pub type SharedConnectionManager = Arc<RwLock<ConnectionManager>>;

pub fn create_shared_manager(local_device_id: DeviceId) -> SharedConnectionManager {
    Arc::new(RwLock::new(ConnectionManager::new(local_device_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::{Encryption, QuicIdentity, QuicTrustStore};
    use rshare_core::{hello_back_message, ScreenInfo};

    fn temp_trust_store_path(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("rshare-state")
            .join(format!("rshare-{name}-{}", uuid::Uuid::new_v4()))
            .join("trust.json")
    }

    fn generated_identity() -> QuicIdentity {
        let (cert_der, key_der) = Encryption::generate_cert().unwrap();
        QuicIdentity { cert_der, key_der }
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
        let manager = ConnectionManager::isolated_for_test(DeviceId::new_v4());
        assert_eq!(manager.connected_count().await, 0);
    }

    #[test]
    fn stale_send_completion_does_not_increment_replacement_generation_metrics() {
        let manager = ConnectionManager::isolated_for_test(DeviceId::new_v4());
        let peer_id = DeviceId::new_v4();
        let old_generation = ControlConnectionId::new();
        let replacement_generation = ControlConnectionId::new();
        insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id: peer_id,
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
                cert_trust_state: Some("trusted".to_string()),
                control_connection_id: Some(replacement_generation),
            },
        );

        manager
            .connection_view()
            .record_send_success(&peer_id, old_generation);
        let messages_sent = || {
            manager
                .connections
                .read()
                .expect("canonical connection registry poisoned")
                .get(&peer_id)
                .expect("replacement remains canonical")
                .info
                .messages_sent
        };
        assert_eq!(
            messages_sent(),
            0,
            "a late completion from the replaced generation must be ignored"
        );

        manager
            .connection_view()
            .record_send_success(&peer_id, replacement_generation);
        assert_eq!(messages_sent(), 1);
    }

    #[tokio::test]
    async fn send_to_completion_from_replaced_generation_does_not_mutate_replacement_metrics() {
        let local_id = DeviceId::new_v4();
        let peer_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(peer_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(local_id);
        let client_connection = client.connect(&address.to_string(), peer_id).await.unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let bulk_reader_barrier = Arc::new(tokio::sync::Notify::new());
        server_connection.set_qos_bulk_reader_barrier(bulk_reader_barrier.clone());
        let old_generation = ControlConnectionId::new();
        let server_auth = Arc::new(crate::handshake::PeerAuthContext {
            peer_id: local_id,
            certificate_fingerprint: crate::encryption::PeerCertificateFingerprint::from_der(
                b"client",
            ),
            control_connection_id: ControlConnectionId::new(),
        });
        let client_auth = Arc::new(crate::handshake::PeerAuthContext {
            peer_id,
            certificate_fingerprint: crate::encryption::PeerCertificateFingerprint::from_der(
                b"server",
            ),
            control_connection_id: old_generation,
        });
        let (_server_qos, _server_releases) = server_connection.install_qos(server_auth);
        let (old_transport, _client_releases) = client_connection.install_qos(client_auth.clone());

        let mut manager = ConnectionManager::isolated_for_test(local_id);
        manager.qos_registry.insert(
            peer_id,
            RegisteredPeer {
                auth: client_auth,
                transport: old_transport.clone(),
            },
        );
        insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id: peer_id,
                address: address.to_string(),
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
                cert_trust_state: Some("trusted".to_string()),
                control_connection_id: Some(old_generation),
            },
        );
        let connections = manager.connections.clone();
        let registry = manager.qos_registry.clone();
        let send = tokio::spawn(async move {
            manager
                .send_to(
                    &peer_id,
                    Message::ClipboardData {
                        mime_type: "application/octet-stream".into(),
                        data: vec![0xA5; 512 * 1024],
                    },
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let (started, completed, _, _) = old_transport.awaited_write_counts_for_test();
                if started > completed && server_connection.qos_bulk_reader_waiting_for_test() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("send_to must be awaiting a real blocked old-generation write");

        let replacement_generation = ControlConnectionId::new();
        connections
            .write()
            .expect("canonical connection registry poisoned")
            .get_mut(&peer_id)
            .expect("canonical peer remains present")
            .info
            .control_connection_id = Some(replacement_generation);
        registry.insert(
            peer_id,
            RegisteredPeer {
                auth: Arc::new(crate::handshake::PeerAuthContext {
                    peer_id,
                    certificate_fingerprint:
                        crate::encryption::PeerCertificateFingerprint::from_der(b"server-new"),
                    control_connection_id: replacement_generation,
                }),
                transport: old_transport,
            },
        );
        bulk_reader_barrier.notify_waiters();
        send.await.unwrap().unwrap();

        assert_eq!(
            connections
                .read()
                .expect("canonical connection registry poisoned")
                .get(&peer_id)
                .expect("replacement remains canonical")
                .info
                .messages_sent,
            0,
            "late send_to success must not increment replacement metrics"
        );
    }

    #[tokio::test]
    async fn fallback_send_to_completion_from_replaced_generation_does_not_mutate_metrics() {
        let local_id = DeviceId::new_v4();
        let peer_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(peer_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(local_id);
        let client_connection = client.connect(&address.to_string(), peer_id).await.unwrap();
        let _server_connection = incoming.recv().await.unwrap().connection;
        let old_control_id = ControlConnectionId::new();
        let replacement_control_id = ControlConnectionId::new();
        let mut manager = ConnectionManager::isolated_for_test(local_id);
        let lifecycle_generation = insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id: peer_id,
                address: address.to_string(),
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
                cert_trust_state: Some("trusted".to_string()),
                control_connection_id: Some(old_control_id),
            },
        );
        manager
            .pool
            .insert_with_generation(peer_id, lifecycle_generation, client_connection)
            .await;
        let (blocked_tx, mut blocked_rx) = mpsc::channel(1);
        manager
            .pool
            .replace_outbound_for_test(peer_id, Some(old_control_id), blocked_tx)
            .await;
        let connections = manager.connections.clone();
        let send = tokio::spawn(async move {
            manager
                .send_to(&peer_id, Message::MouseMove { x: 9, y: 11 })
                .await
        });
        let blocked_frame = tokio::time::timeout(Duration::from_secs(1), blocked_rx.recv())
            .await
            .expect("fallback send must select the delayed outbound sender")
            .expect("delayed outbound sender must remain connected");

        connections
            .write()
            .expect("canonical connection registry poisoned")
            .get_mut(&peer_id)
            .expect("canonical peer remains present")
            .info
            .control_connection_id = Some(replacement_control_id);
        blocked_frame.complete_for_test(Ok(()));
        send.await.unwrap().unwrap();

        assert_eq!(
            connections
                .read()
                .expect("canonical connection registry poisoned")
                .get(&peer_id)
                .expect("replacement remains canonical")
                .info
                .messages_sent,
            0,
            "late fallback success must not increment replacement metrics"
        );
    }

    #[tokio::test]
    async fn failed_fallback_send_to_does_not_increment_metrics() {
        let local_id = DeviceId::new_v4();
        let peer_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(peer_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(local_id);
        let client_connection = client.connect(&address.to_string(), peer_id).await.unwrap();
        let _server_connection = incoming.recv().await.unwrap().connection;
        let control_id = ControlConnectionId::new();
        let mut manager = ConnectionManager::isolated_for_test(local_id);
        let lifecycle_generation = insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id: peer_id,
                address: address.to_string(),
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
                cert_trust_state: Some("trusted".to_string()),
                control_connection_id: Some(control_id),
            },
        );
        manager
            .pool
            .insert_with_generation(peer_id, lifecycle_generation, client_connection)
            .await;
        let (blocked_tx, mut blocked_rx) = mpsc::channel(1);
        manager
            .pool
            .replace_outbound_for_test(peer_id, Some(control_id), blocked_tx)
            .await;
        let connections = manager.connections.clone();
        let send = tokio::spawn(async move {
            manager
                .send_to(&peer_id, Message::MouseMove { x: 13, y: 17 })
                .await
        });
        let blocked_frame = tokio::time::timeout(Duration::from_secs(1), blocked_rx.recv())
            .await
            .expect("fallback send must select the delayed outbound sender")
            .expect("delayed outbound sender must remain connected");
        blocked_frame.complete_for_test(Err("injected write failure".into()));

        assert!(send.await.unwrap().is_err());
        assert_eq!(
            connections
                .read()
                .expect("canonical connection registry poisoned")
                .get(&peer_id)
                .expect("canonical peer remains present")
                .info
                .messages_sent,
            0,
            "failed fallback sends must never increment metrics"
        );
    }

    #[tokio::test]
    async fn fallback_send_to_with_no_control_id_still_matches_lifecycle_generation() {
        let local_id = DeviceId::new_v4();
        let peer_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(peer_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(local_id);
        let client_connection = client.connect(&address.to_string(), peer_id).await.unwrap();
        let _server_connection = incoming.recv().await.unwrap().connection;
        let mut manager = ConnectionManager::isolated_for_test(local_id);
        let old_lifecycle_generation = insert_test_canonical(
            &manager,
            ConnectionInfo {
                device_id: peer_id,
                address: address.to_string(),
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
                cert_trust_state: Some("trusted".to_string()),
                control_connection_id: Some(ControlConnectionId::new()),
            },
        );
        manager
            .pool
            .insert_with_generation(peer_id, old_lifecycle_generation, client_connection)
            .await;
        let (blocked_tx, mut blocked_rx) = mpsc::channel(1);
        manager
            .pool
            .replace_outbound_for_test(peer_id, None, blocked_tx)
            .await;
        let connections = manager.connections.clone();
        let next_generation = manager.next_generation.clone();
        let send = tokio::spawn(async move {
            manager
                .send_to(&peer_id, Message::MouseMove { x: 19, y: 23 })
                .await
        });
        let blocked_frame = tokio::time::timeout(Duration::from_secs(1), blocked_rx.recv())
            .await
            .expect("fallback send must select the delayed outbound sender")
            .expect("delayed outbound sender must remain connected");

        let replacement_generation = allocate_generation(&next_generation);
        connections
            .write()
            .expect("canonical connection registry poisoned")
            .insert(
                peer_id,
                CanonicalConnection {
                    generation: replacement_generation,
                    info: ConnectionInfo {
                        device_id: peer_id,
                        address: address.to_string(),
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
                        cert_trust_state: Some("trusted".to_string()),
                        control_connection_id: None,
                    },
                },
            );
        blocked_frame.complete_for_test(Ok(()));
        send.await.unwrap().unwrap();

        assert_eq!(
            connections
                .read()
                .expect("canonical connection registry poisoned")
                .get(&peer_id)
                .expect("replacement remains canonical")
                .info
                .messages_sent,
            0,
            "late fallback success must not match a replacement solely because both control IDs are absent"
        );
    }

    #[tokio::test]
    async fn message_reader_emits_disconnected_when_channel_closes() {
        let device_id = DeviceId::new_v4();
        let control_connection_id = ControlConnectionId::new();
        let mut manager = ConnectionManager::isolated_for_test(DeviceId::new_v4());
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
                control_connection_id: Some(control_connection_id),
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

        assert!(matches!(
            event,
            ManagerEvent::Disconnected { peer_id, .. } if peer_id == device_id
        ));
    }

    #[tokio::test]
    async fn first_message_delivery_failure_retires_generation() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();

        let mut server = QuicTransport::isolated_for_test(remote_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let accepted = tokio::spawn(async move { incoming.recv().await.unwrap().connection });

        let mut client = QuicTransport::isolated_for_test(local_id);
        let mut connection = client
            .connect(&address.to_string(), remote_id)
            .await
            .unwrap();
        let _server_connection = accepted.await.unwrap();
        connection.confirm_peer_identity(remote_id).unwrap();
        connection.set_device_id(remote_id);
        let messages = connection.message_channel();

        let mut manager = ConnectionManager::isolated_for_test(local_id);
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
    }

    #[tokio::test]
    async fn retirement_waiting_for_event_capacity_does_not_hold_lifecycle() {
        let device_id = DeviceId::new_v4();
        let control_connection_id = ControlConnectionId::new();
        let mut manager = ConnectionManager::isolated_for_test(DeviceId::new_v4());
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
                control_connection_id: Some(control_connection_id),
            },
        );
        let mut events = manager.events().unwrap();
        while manager
            .event_tx
            .try_send(ManagerEvent::Error {
                peer_id: None,
                control_connection_id: None,
                error: "fill".into(),
            })
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
                    Some(ManagerEvent::Disconnected { peer_id: disconnected, .. })
                        if disconnected == device_id
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

        let mut server = QuicTransport::isolated_for_test(remote_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut first_client = QuicTransport::isolated_for_test(local_id);
        let mut old_connection = first_client
            .connect(&address.to_string(), remote_id)
            .await
            .unwrap();
        let _first_server_connection = incoming.recv().await.unwrap().connection;
        old_connection.confirm_peer_identity(remote_id).unwrap();
        old_connection.set_device_id(remote_id);

        let mut second_client = QuicTransport::isolated_for_test(local_id);
        let mut new_connection = second_client
            .connect(&address.to_string(), remote_id)
            .await
            .unwrap();
        let _second_server_connection = incoming.recv().await.unwrap().connection;
        new_connection.confirm_peer_identity(remote_id).unwrap();
        new_connection.set_device_id(remote_id);

        let mut manager = ConnectionManager::isolated_for_test(local_id);
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
            .send(ManagerEvent::Error {
                peer_id: Some(remote_id),
                control_connection_id: Some(replacement_control_id),
                error: "replacement-installed".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            events.recv().await,
            Some(ManagerEvent::Error {
                peer_id: Some(device_id),
                ..
            }) if device_id == remote_id
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
    }

    #[tokio::test]
    async fn outbound_connect_failure_emits_error_event() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();

        let mut manager = ConnectionManager::isolated_for_test(local_id);
        let mut events = manager.events().unwrap();

        assert!(manager.connect(remote_id, "not-an-addr").await.is_err());

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            event,
            ManagerEvent::Error {
                peer_id: Some(device_id),
                error: _,
                ..
            } if device_id == remote_id
        ));
    }

    #[tokio::test]
    async fn explicit_disconnect_emits_disconnected_event() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let control_connection_id = ControlConnectionId::new();
        let mut manager = ConnectionManager::isolated_for_test(local_id);
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
                control_connection_id: Some(control_connection_id),
            },
        );
        let mut events = manager.events().unwrap();

        manager.disconnect(&remote_id).await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            event,
            ManagerEvent::Disconnected { peer_id, .. } if peer_id == remote_id
        ));
    }

    #[tokio::test]
    async fn explicit_disconnect_closes_quic_and_retires_remote_reader() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut remote = ConnectionManager::isolated_for_test(remote_id);
        let mut remote_events = remote.events().unwrap();
        remote.start_server("127.0.0.1:0").await.unwrap();
        let address = remote.transport_local_addr().unwrap();

        let mut local = ConnectionManager::isolated_for_test(local_id);
        local
            .connect(remote_id, &address.to_string())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(
                    remote_events.recv().await,
                    Some(ManagerEvent::Connected(auth)) if auth.peer_id == local_id
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
                    Some(ManagerEvent::Disconnected { peer_id: id, .. }) if id == local_id
                ) {
                    break;
                }
            }
        })
        .await
        .expect("explicit disconnect must close QUIC and retire the remote reader");
        assert!(!remote.is_connected(&local_id).await);
    }

    #[tokio::test]
    async fn connected_entry_without_active_transport_is_reported_disconnected() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let manager = ConnectionManager::isolated_for_test(local_id);
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

        let mut server = QuicTransport::isolated_for_test(remote_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let accepted = tokio::spawn(async move { incoming.recv().await.unwrap().connection });

        let mut client = QuicTransport::isolated_for_test(local_id);
        let mut connection = client
            .connect(&address.to_string(), remote_id)
            .await
            .unwrap();
        let _server_connection = accepted.await.unwrap();
        connection.confirm_peer_identity(remote_id).unwrap();
        connection.set_device_id(remote_id);
        let _held_messages = connection.message_channel();

        let manager = ConnectionManager::isolated_for_test(local_id);
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
    }

    #[tokio::test]
    async fn manager_emits_control_received_for_connected_device() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut remote_manager = ConnectionManager::isolated_for_test(remote_id);
        let mut remote_events = remote_manager.events().unwrap();
        remote_manager.start_server("127.0.0.1:0").await.unwrap();
        let address = remote_manager.transport_local_addr().unwrap();

        let mut manager = ConnectionManager::isolated_for_test(local_id);
        let mut events = manager.events().unwrap();
        manager
            .connect(remote_id, &address.to_string())
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::Connected(auth) = remote_events.recv().await.unwrap() {
                    assert_eq!(auth.peer_id, local_id);
                    break;
                }
            }
        })
        .await
        .unwrap();

        remote_manager
            .send_to(
                &local_id,
                Message::Heartbeat {
                    sequence: 7,
                    timestamp: 9,
                },
            )
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match events.recv().await.unwrap() {
                    ManagerEvent::ControlReceived { auth, frame } => {
                        break (auth.peer_id, frame.into_message());
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(received.0, remote_id);
        assert!(matches!(
            received.1,
            Message::Heartbeat {
                sequence: 7,
                timestamp: 9
            }
        ));
    }

    #[tokio::test]
    async fn manager_publishes_one_typed_inbound_set_and_generation_aware_control_event() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = ConnectionManager::isolated_for_test(server_id);
        let mut events = server.events().unwrap();
        let mut authenticated = server.authenticated_peers().unwrap();
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.transport_local_addr().unwrap();

        let mut client = ConnectionManager::isolated_for_test(client_id);
        client
            .connect(server_id, &address.to_string())
            .await
            .unwrap();

        let peer = tokio::time::timeout(Duration::from_secs(1), authenticated.recv())
            .await
            .expect("authenticated connection must publish its typed inbound set")
            .unwrap();
        assert_eq!(peer.auth.peer_id, client_id);
        assert!(
            authenticated.try_recv().is_err(),
            "one connection generation must publish exactly one receiver set"
        );

        client
            .send_to(
                &server_id,
                Message::Heartbeat {
                    sequence: 41,
                    timestamp: 43,
                },
            )
            .await
            .unwrap();
        let (auth, _frame) = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::ControlReceived { auth, frame } = events.recv().await.unwrap()
                {
                    break (auth, frame);
                }
            }
        })
        .await
        .expect("control compatibility event must be forwarded");
        assert_eq!(auth.peer_id, client_id);
        assert_eq!(
            auth.control_connection_id, peer.auth.control_connection_id,
            "control event must retain the authenticated generation"
        );
    }

    #[tokio::test]
    async fn authenticated_qos_is_installed_before_general_event_capacity_is_available() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = ConnectionManager::isolated_for_test(server_id);
        let mut events = server.events().unwrap();
        let mut authenticated = server.authenticated_peers().unwrap();
        while server
            .event_tx
            .try_send(ManagerEvent::Error {
                peer_id: None,
                control_connection_id: None,
                error: "fill".into(),
            })
            .is_ok()
        {}
        assert_eq!(server.event_tx.capacity(), 0);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.transport_local_addr().unwrap();

        let mut client = ConnectionManager::isolated_for_test(client_id);
        client
            .connect(server_id, &address.to_string())
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_millis(100), async {
            while server.qos_registry.peer(&client_id).is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("authenticated QoS context must be installed before event capacity is awaited");
        let mut peer = tokio::time::timeout(Duration::from_millis(100), authenticated.recv())
            .await
            .expect("typed inbound must publish independently of the general event FIFO")
            .unwrap();

        client
            .send_to(&server_id, Message::MouseMove { x: 7, y: 9 })
            .await
            .unwrap();
        let client_peer = client.qos_registry.peer(&server_id).unwrap();
        client_peer
            .transport
            .try_send_reliable_input(rshare_core::ReliableInputFrame {
                protocol_version: rshare_core::INPUT_PROTOCOL_VERSION,
                session_epoch: rshare_core::SessionEpoch(1),
                sequence: 1,
                captured_at: rshare_core::MonotonicStamp::new(rshare_core::ClockDomainId(1), 1),
                event: rshare_core::ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), peer.reliable_input_rx.recv())
            .await
            .expect("typed reliable input must drain while the general event FIFO is full")
            .unwrap();
        client_peer
            .transport
            .try_send_realtime(rshare_core::RealtimeInputFrame {
                protocol_version: rshare_core::INPUT_PROTOCOL_VERSION,
                session_epoch: rshare_core::SessionEpoch(1),
                sequence: 1,
                captured_at: rshare_core::MonotonicStamp::new(rshare_core::ClockDomainId(1), 2),
                payload: rshare_core::RealtimeInputPayload::RelativeMouse { dx: 1, dy: 2 },
            })
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), peer.realtime_rx.recv())
                .await
                .expect("typed realtime input must drain while the general event FIFO is full")
                .unwrap()
                .sequence,
            1
        );

        while events.try_recv().is_ok() {}
        let connected = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match events.recv().await.unwrap() {
                    ManagerEvent::Connected(auth) => break auth,
                    ManagerEvent::MessageReceived { message, .. } => {
                        panic!("legacy realtime entered general FIFO: {message:?}")
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("freeing event capacity must publish the connected event");
        assert_eq!(connected.peer_id, client_id);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), async {
                loop {
                    if let ManagerEvent::MessageReceived { message, .. } =
                        events.recv().await.unwrap()
                    {
                        break message;
                    }
                }
            })
            .await
            .is_err(),
            "legacy realtime must never enter the general ManagerEvent FIFO"
        );
    }

    #[tokio::test]
    async fn stale_authenticated_publication_failure_cannot_close_replacement_generation() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut remote = ConnectionManager::isolated_for_test(remote_id);
        remote.start_server("127.0.0.1:0").await.unwrap();
        let address = remote.transport_local_addr().unwrap().to_string();

        let mut manager = ConnectionManager::isolated_for_test(local_id);
        let mut events = manager.events().unwrap();
        manager.connect(remote_id, &address).await.unwrap();
        let replacement = manager
            .qos_registry
            .peer(&remote_id)
            .expect("replacement generation must be registered");
        let replacement_lifecycle_generation = manager
            .pool
            .generation_for(&remote_id)
            .await
            .expect("replacement generation must be pooled");
        let stale_auth = crate::handshake::PeerAuthContext {
            peer_id: remote_id,
            certificate_fingerprint: replacement.auth.certificate_fingerprint.clone(),
            control_connection_id: ControlConnectionId::new(),
        };

        assert!(
            !fail_authenticated_publication(
                &manager.connections,
                &manager.pool,
                &manager.lifecycle_lock,
                &manager.event_tx,
                &stale_auth,
                replacement_lifecycle_generation
                    .checked_add(1)
                    .expect("test generation must advance"),
                &manager.qos_registry,
                "stale authenticated peer publication failed",
            )
            .await,
            "stale publication failure must not remove a replacement generation"
        );
        assert!(manager.is_connected(&remote_id).await);
        assert_eq!(
            manager
                .qos_registry
                .peer(&remote_id)
                .unwrap()
                .auth
                .control_connection_id,
            replacement.auth.control_connection_id
        );
        assert!(
            events.try_recv().is_ok(),
            "the replacement connected event remains observable"
        );
        assert!(
            events.try_recv().is_err(),
            "stale publication failure must not emit an error for the replacement"
        );
    }

    #[tokio::test]
    async fn closed_authenticated_peer_receiver_fails_closed_with_generation_error() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = ConnectionManager::isolated_for_test(server_id);
        let mut events = server.events().unwrap();
        drop(
            server
                .authenticated_peers()
                .expect("authenticated peer receiver must be available"),
        );
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.transport_local_addr().unwrap().to_string();

        let mut client = ConnectionManager::isolated_for_test(client_id);
        client.connect(server_id, &address).await.unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::Error {
                    peer_id: Some(peer_id),
                    control_connection_id: Some(control_connection_id),
                    error,
                } = events.recv().await.unwrap()
                {
                    if peer_id == client_id {
                        assert!(error.contains("queue is closed"));
                        break control_connection_id;
                    }
                }
            }
        })
        .await
        .expect("closed public receiver must produce a generation-aware error");
        assert!(!server.is_connected(&client_id).await);
        assert!(server.qos_registry.peer(&client_id).is_none());
    }

    #[tokio::test]
    async fn manager_emits_control_received_for_incoming_connection() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();

        let mut manager = ConnectionManager::isolated_for_test(local_id);
        let mut events = manager.events().unwrap();
        manager.start_server("127.0.0.1:0").await.unwrap();
        let address = manager.transport_local_addr().unwrap();

        let mut remote_manager = ConnectionManager::isolated_for_test(remote_id);
        remote_manager
            .connect(local_id, &address.to_string())
            .await
            .unwrap();
        remote_manager
            .send_to(
                &local_id,
                Message::Heartbeat {
                    sequence: 11,
                    timestamp: 13,
                },
            )
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match events.recv().await.unwrap() {
                    ManagerEvent::ControlReceived { auth, frame } => {
                        break (auth.peer_id, frame.into_message());
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();

        assert!(matches!(
            received.1,
            Message::Heartbeat {
                sequence: 11,
                timestamp: 13
            }
        ));

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

        let mut manager = ConnectionManager::isolated_for_test(local_id);
        let mut events = manager.events().unwrap();
        manager.start_server("127.0.0.1:0").await.unwrap();
        let address = manager.transport_local_addr().unwrap();

        let mut remote_manager = ConnectionManager::isolated_for_test(remote_id);
        remote_manager
            .connect(local_id, &address.to_string())
            .await
            .unwrap();

        let connected = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::Connected(auth) = events.recv().await.unwrap() {
                    break auth.peer_id;
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
        let remote_identity = generated_identity();

        let mut manager = ConnectionManager::isolated_for_test(local_id);
        let mut events = manager.events().unwrap();
        manager.start_server("127.0.0.1:0").await.unwrap();
        let address = manager.transport_local_addr().unwrap();

        let mut first_peer = ConnectionManager::with_transport(
            remote_id,
            QuicTransport::isolated_with_identity_for_test(remote_id, remote_identity.clone()),
        );
        first_peer
            .connect(local_id, &address.to_string())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(
                    events.recv().await,
                    Some(ManagerEvent::Connected(auth)) if auth.peer_id == remote_id
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
            QuicTransport::isolated_with_identity_for_test(remote_id, remote_identity),
        );
        duplicate_peer
            .connect(local_id, &address.to_string())
            .await
            .unwrap();

        let duplicate_connected = tokio::time::timeout(Duration::from_millis(150), async {
            loop {
                if matches!(
                    events.recv().await,
                    Some(ManagerEvent::Connected(auth)) if auth.peer_id == remote_id
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
            .send_to(
                &local_id,
                Message::Heartbeat {
                    sequence: 31,
                    timestamp: 37,
                },
            )
            .await
            .unwrap();
        let received = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(ManagerEvent::ControlReceived { auth, frame }) = events.recv().await {
                    break (auth.peer_id, frame.into_message());
                }
            }
        })
        .await
        .expect("the original inbound connection should remain usable");
        assert_eq!(received.0, remote_id);
        assert!(matches!(
            received.1,
            Message::Heartbeat {
                sequence: 31,
                timestamp: 37
            }
        ));

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
    }

    #[tokio::test]
    async fn outbound_connect_accepts_hello_back_identity() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut remote_manager = ConnectionManager::isolated_for_test(remote_id);
        remote_manager.start_server("127.0.0.1:0").await.unwrap();
        let address = remote_manager.transport_local_addr().unwrap();

        let mut manager = ConnectionManager::isolated_for_test(local_id);
        let mut events = manager.events().unwrap();
        manager
            .connect(remote_id, &address.to_string())
            .await
            .unwrap();

        let connected = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::Connected(auth) = events.recv().await.unwrap() {
                    break auth.peer_id;
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
        let remote_identity = generated_identity();

        let mut first_remote = ConnectionManager::with_transport(
            remote_id,
            QuicTransport::isolated_with_identity_for_test(remote_id, remote_identity.clone()),
        );
        first_remote.start_server("127.0.0.1:0").await.unwrap();
        let first_address = first_remote.transport_local_addr().unwrap();

        let mut manager = ConnectionManager::isolated_for_test(local_id);
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
                    Some(ManagerEvent::Disconnected { peer_id: device_id, .. })
                        if device_id == remote_id
                ) {
                    break;
                }
            }
        })
        .await
        .expect("the first connection reader should report its closed transport");
        assert!(!manager.is_connected(&remote_id).await);

        let mut replacement_remote = ConnectionManager::with_transport(
            remote_id,
            QuicTransport::isolated_with_identity_for_test(remote_id, remote_identity),
        );
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
                if let Some(ManagerEvent::Connected(auth)) = events.recv().await {
                    if auth.peer_id == remote_id {
                        break auth.peer_id;
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
                    Some(ManagerEvent::Disconnected {
                        peer_id: device_id, ..
                    }) if device_id == remote_id => {
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
    }

    #[tokio::test]
    async fn outbound_connect_rejects_mismatched_hello_back_without_pinning() {
        let local_id = DeviceId::new_v4();
        let expected_id = DeviceId::new_v4();
        let returned_id = DeviceId::new_v4();
        let trust_path = temp_trust_store_path("hello-back-mismatch");

        let mut server = QuicTransport::isolated_for_test(returned_id);
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
            QuicTransport::isolated_for_test(local_id).with_trust_store_path(trust_path.clone()),
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

        let mut server = QuicTransport::isolated_for_test(expected_id);
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
            QuicTransport::isolated_for_test(local_id).with_trust_store_path(trust_path.clone()),
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
