//! Quinn QUIC transport layer for low-latency encrypted communication.

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use quinn::{
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
    ClientConfig, Endpoint, IdleTimeout, ServerConfig as QuinnServerConfig,
    TransportConfig as QuinnTransportConfig, VarInt,
};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    DigitallySignedStruct, DistinguishedName, Error as RustlsError, SignatureScheme,
};
use std::any::Any;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Once;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, Mutex as TokioMutex, Notify};

use super::codec::{ControlMessageCodec, RealtimeInputCodec};
use super::encryption::{
    Encryption, PeerCertificateFingerprint, QuicIdentity, QuicTrustDecision, QuicTrustStore,
};
use rshare_core::{DeviceId, Message};
use tracing::info;

const BOOTSTRAP_RELIABLE_READER_ACK: u8 = 0b01;
const BOOTSTRAP_DATAGRAM_READER_ACK: u8 = 0b10;
const BOOTSTRAP_ALL_READERS_ACKED: u8 =
    BOOTSTRAP_RELIABLE_READER_ACK | BOOTSTRAP_DATAGRAM_READER_ACK;

#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub max_idle_timeout: Duration,
    pub keep_alive_interval: Duration,
    pub max_message_size: usize,
    pub datagram_receive_buffer_size: usize,
    pub datagram_send_buffer_size: usize,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            max_idle_timeout: Duration::from_secs(10),
            keep_alive_interval: Duration::from_secs(2),
            max_message_size: 10 * 1024 * 1024,
            datagram_receive_buffer_size: 64 * 1024,
            datagram_send_buffer_size: 8 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReliableFrame {
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportConnectionDiagnostics {
    pub address: String,
    pub transport: &'static str,
    pub datagram_available: bool,
    pub rtt_ms: Option<u64>,
    pub last_datagram_rx_ms: Option<u64>,
    pub datagram_tx_dropped: u64,
    pub reliable_stream_reset_count: u64,
    pub cert_trust_state: Option<String>,
}

pub type PeerTransportConnection = QuicConnection;

pub struct QuicTransport {
    server_endpoint: Option<Endpoint>,
    server_task: Option<tokio::task::JoinHandle<()>>,
    config: TransportConfig,
    identity: QuicIdentity,
    local_device_id: DeviceId,
    incoming_tx: mpsc::Sender<IncomingConnection>,
    incoming_rx: Option<mpsc::Receiver<IncomingConnection>>,
    trust_store_path: Option<PathBuf>,
    present_client_certificate: bool,
    peer_protocol_handshake_required: bool,
    datagram_reader_start_barrier: Option<Arc<Notify>>,
}

pub struct IncomingConnection {
    pub device_id: Option<DeviceId>,
    pub address: SocketAddr,
    pub connection: QuicConnection,
}

impl QuicTransport {
    pub fn new(local_device_id: DeviceId) -> Self {
        let (incoming_tx, incoming_rx) = mpsc::channel(10);
        let identity = Encryption::load_or_generate_default_identity().unwrap_or_else(|error| {
            tracing::warn!(
                "Failed to load persistent QUIC identity, using ephemeral certificate: {}",
                error
            );
            let (cert_der, key_der) =
                Encryption::generate_cert().expect("ephemeral QUIC certificate generation failed");
            QuicIdentity { cert_der, key_der }
        });

        Self {
            server_endpoint: None,
            server_task: None,
            config: TransportConfig::default(),
            identity,
            local_device_id,
            incoming_tx,
            incoming_rx: Some(incoming_rx),
            trust_store_path: None,
            present_client_certificate: true,
            peer_protocol_handshake_required: false,
            datagram_reader_start_barrier: None,
        }
    }

    pub fn with_identity(local_device_id: DeviceId, identity: QuicIdentity) -> Self {
        let mut transport = Self::new(local_device_id);
        transport.identity = identity;
        transport
    }

    pub fn with_config(mut self, config: TransportConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_trust_store_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.trust_store_path = Some(path.into());
        self
    }

    pub fn without_client_certificate(mut self) -> Self {
        self.present_client_certificate = false;
        self
    }

    pub(crate) fn require_peer_protocol_handshake(&mut self) {
        self.peer_protocol_handshake_required = true;
    }

    #[cfg(test)]
    fn with_test_datagram_reader_barrier(mut self, barrier: Arc<Notify>) -> Self {
        self.datagram_reader_start_barrier = Some(barrier);
        self
    }

    pub async fn start_server(&mut self, bind_addr: &str) -> Result<()> {
        let bind_addr: SocketAddr = bind_addr
            .parse()
            .map_err(|_| anyhow!("Invalid bind address: {}", bind_addr))?;

        ensure_rustls_crypto_provider();
        let server_config = match make_server_config(&self.identity, &self.config) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(
                    "Persisted QUIC identity is invalid, regenerating certificate: {}",
                    error
                );
                self.identity = Encryption::regenerate_default_identity()?;
                make_server_config(&self.identity, &self.config)?
            }
        };
        let endpoint = Endpoint::server(server_config, bind_addr)
            .with_context(|| format!("Failed to bind QUIC endpoint on {bind_addr}"))?;
        let local_addr = endpoint.local_addr()?;
        info!("QUIC transport server listening on {}", local_addr);

        let incoming_tx = self.incoming_tx.clone();
        let local_device_id = self.local_device_id;
        let config = self.config.clone();
        let trust_store_path = match &self.trust_store_path {
            Some(path) => path.clone(),
            None => super::encryption::trust_store_path()?,
        };
        let peer_protocol_handshake_required = self.peer_protocol_handshake_required;
        let datagram_reader_start_barrier = self.datagram_reader_start_barrier.clone();
        let accept_endpoint = endpoint.clone();

        let server_task = tokio::spawn(async move {
            while let Some(connecting) = accept_endpoint.accept().await {
                let incoming_tx = incoming_tx.clone();
                let endpoint = accept_endpoint.clone();
                let config = config.clone();
                let trust_store_path = trust_store_path.clone();
                let datagram_reader_start_barrier = datagram_reader_start_barrier.clone();

                tokio::spawn(async move {
                    match connecting.await {
                        Ok(connection) => {
                            let addr = connection.remote_address();
                            info!("Incoming QUIC connection from {}", addr);
                            let quic_conn = QuicConnection::from_quinn(
                                endpoint,
                                connection,
                                local_device_id,
                                addr,
                                config,
                                None,
                                trust_store_path,
                                peer_protocol_handshake_required,
                                datagram_reader_start_barrier,
                            );

                            let _ = incoming_tx
                                .send(IncomingConnection {
                                    device_id: None,
                                    address: addr,
                                    connection: quic_conn,
                                })
                                .await;
                        }
                        Err(error) => {
                            tracing::warn!("Incoming QUIC handshake failed: {}", error);
                        }
                    }
                });
            }
        });

        self.server_endpoint = Some(endpoint);
        self.server_task = Some(server_task);
        Ok(())
    }

    pub async fn connect(
        &mut self,
        remote_addr: &str,
        device_id: DeviceId,
    ) -> Result<QuicConnection> {
        let remote_addr: SocketAddr = remote_addr
            .parse()
            .map_err(|_| anyhow!("Invalid remote address: {}", remote_addr))?;

        info!("Connecting to QUIC peer {}", remote_addr);

        ensure_rustls_crypto_provider();
        let mut endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap())
            .context("Failed to create QUIC client endpoint")?;
        endpoint.set_default_client_config(make_client_config(
            &self.identity,
            &self.config,
            self.present_client_certificate,
        )?);

        let connection = endpoint
            .connect(remote_addr, "rshare.local")
            .context("Failed to start QUIC connect")?
            .await
            .with_context(|| format!("QUIC handshake failed for {remote_addr}"))?;

        let trust_store_path = match &self.trust_store_path {
            Some(path) => path.clone(),
            None => super::encryption::trust_store_path()?,
        };
        let pending_peer_trust =
            inspect_outbound_peer_trust(&connection, device_id, trust_store_path.clone()).await?;

        info!("Connected to QUIC peer {}", connection.remote_address());

        Ok(QuicConnection::from_quinn(
            endpoint,
            connection,
            self.local_device_id,
            remote_addr,
            self.config.clone(),
            Some(pending_peer_trust),
            trust_store_path,
            self.peer_protocol_handshake_required,
            self.datagram_reader_start_barrier.clone(),
        ))
    }

    pub fn incoming(&mut self) -> mpsc::Receiver<IncomingConnection> {
        self.incoming_rx.take().expect("Incoming already taken")
    }

    pub fn is_running(&self) -> bool {
        self.server_task
            .as_ref()
            .map(|task| !task.is_finished())
            .unwrap_or(false)
    }

    pub fn local_device_id(&self) -> DeviceId {
        self.local_device_id
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.server_endpoint
            .as_ref()
            .and_then(|endpoint| endpoint.local_addr().ok())
    }

    pub async fn close(&mut self) -> Result<()> {
        if let Some(endpoint) = self.server_endpoint.take() {
            endpoint.close(0u32.into(), b"shutdown");
        }
        if let Some(task) = self.server_task.take() {
            task.abort();
            let _ = task.await;
        }
        info!("Transport closed");
        Ok(())
    }
}

impl Default for QuicTransport {
    fn default() -> Self {
        Self::new(DeviceId::new_v4())
    }
}

struct QuicConnectionInner {
    _endpoint: Endpoint,
    connection: quinn::Connection,
    reliable_send_stream: TokioMutex<Option<quinn::SendStream>>,
    datagram_tx_dropped: AtomicU64,
    reliable_stream_reset_count: AtomicU64,
    last_datagram_rx_us: AtomicU64,
    bootstrap_complete: AtomicBool,
    bootstrap_notify: Notify,
    bootstrap_stream_verified: AtomicBool,
    bootstrap_stream_notify: Notify,
    bootstrap_commit_requested: AtomicBool,
    bootstrap_commit_notify: Notify,
    bootstrap_commit_acks: AtomicU8,
    bootstrap_commit_ack_notify: Notify,
    datagram_reader_start_barrier: Option<Arc<Notify>>,
}

impl Drop for QuicConnectionInner {
    fn drop(&mut self) {
        self.connection.close(0u32.into(), b"shutdown");
    }
}

pub struct QuicConnection {
    device_id: Option<DeviceId>,
    remote_addr: SocketAddr,
    send_channel: mpsc::Sender<OutboundFrame>,
    message_rx: Option<mpsc::Receiver<Message>>,
    _local_device_id: DeviceId,
    inner: Arc<QuicConnectionInner>,
    cert_trust_state: Option<String>,
    pending_peer_trust: Option<PendingPeerTrust>,
    peer_fingerprint: Option<PeerCertificateFingerprint>,
    trust_store_path: PathBuf,
}

#[derive(Debug)]
struct PendingPeerTrust {
    expected_device_id: DeviceId,
    fingerprint: PeerCertificateFingerprint,
    trust_store_path: PathBuf,
    decision: QuicTrustDecision,
}

struct OutboundFrame {
    message: Message,
    ack: oneshot::Sender<std::result::Result<(), String>>,
}

#[derive(Clone)]
struct OutboundSender {
    send_channel: mpsc::Sender<OutboundFrame>,
}

impl OutboundSender {
    async fn send_message(&self, message: &Message) -> Result<()> {
        let (ack, written) = oneshot::channel();
        self.send_channel
            .send(OutboundFrame {
                message: message.clone(),
                ack,
            })
            .await
            .map_err(|_| anyhow!("Send channel closed"))?;
        written
            .await
            .map_err(|_| anyhow!("Write confirmation channel closed"))?
            .map_err(|error| anyhow!("Write failed: {error}"))
    }

    fn try_send_message(
        &self,
        message: &Message,
    ) -> std::result::Result<(), FanoutEnqueueFailureKind> {
        let (ack, written) = oneshot::channel();
        let frame = OutboundFrame {
            message: message.clone(),
            ack,
        };
        let result = self
            .send_channel
            .try_send(frame)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => FanoutEnqueueFailureKind::QueueFull,
                mpsc::error::TrySendError::Closed(_) => FanoutEnqueueFailureKind::QueueClosed,
            });
        drop(written);
        result
    }
}

impl QuicConnection {
    fn from_quinn(
        endpoint: Endpoint,
        connection: quinn::Connection,
        local_device_id: DeviceId,
        remote_addr: SocketAddr,
        config: TransportConfig,
        pending_peer_trust: Option<PendingPeerTrust>,
        trust_store_path: PathBuf,
        peer_protocol_handshake_required: bool,
        datagram_reader_start_barrier: Option<Arc<Notify>>,
    ) -> Self {
        let (send_channel, mut send_rx): (mpsc::Sender<OutboundFrame>, _) = mpsc::channel(128);
        let (message_tx, message_rx): (mpsc::Sender<Message>, _) = mpsc::channel(256);
        let peer_fingerprint = peer_certificate_fingerprint(&connection);
        let inner = Arc::new(QuicConnectionInner {
            _endpoint: endpoint,
            connection,
            reliable_send_stream: TokioMutex::new(None),
            datagram_tx_dropped: AtomicU64::new(0),
            reliable_stream_reset_count: AtomicU64::new(0),
            last_datagram_rx_us: AtomicU64::new(0),
            bootstrap_complete: AtomicBool::new(!peer_protocol_handshake_required),
            bootstrap_notify: Notify::new(),
            bootstrap_stream_verified: AtomicBool::new(!peer_protocol_handshake_required),
            bootstrap_stream_notify: Notify::new(),
            bootstrap_commit_requested: AtomicBool::new(false),
            bootstrap_commit_notify: Notify::new(),
            bootstrap_commit_acks: AtomicU8::new(if peer_protocol_handshake_required {
                0
            } else {
                BOOTSTRAP_ALL_READERS_ACKED
            }),
            bootstrap_commit_ack_notify: Notify::new(),
            datagram_reader_start_barrier,
        });
        let outbound_seq = Arc::new(AtomicU32::new(1));

        {
            let inner = inner.clone();
            let outbound_seq = outbound_seq.clone();
            let writer_config = config.clone();
            tokio::spawn(async move {
                while let Some(frame) = send_rx.recv().await {
                    let result = send_outbound_message(
                        &inner,
                        &outbound_seq,
                        &writer_config,
                        &frame.message,
                    )
                    .await
                    .map_err(|error| error.to_string());
                    let _ = frame.ack.send(result);
                }
            });
        }

        {
            let inner = inner.clone();
            let message_tx = message_tx.clone();
            let max_message_size = config.max_message_size;
            tokio::spawn(async move {
                read_reliable_messages(inner, message_tx, max_message_size).await;
            });
        }

        {
            let inner = inner.clone();
            tokio::spawn(async move {
                read_realtime_datagrams(inner, message_tx).await;
            });
        }

        let cert_trust_state = pending_peer_trust
            .as_ref()
            .map(|pending| match pending.decision {
                QuicTrustDecision::FirstSeen => "first_seen_pending".to_string(),
                _ => trust_state_label(&pending.decision).to_string(),
            });

        Self {
            device_id: None,
            remote_addr,
            send_channel,
            message_rx: Some(message_rx),
            _local_device_id: local_device_id,
            inner,
            cert_trust_state,
            pending_peer_trust,
            peer_fingerprint,
            trust_store_path,
        }
    }

    pub fn device_id(&self) -> Option<DeviceId> {
        self.device_id
    }

    pub fn set_device_id(&mut self, device_id: DeviceId) {
        self.device_id = Some(device_id);
    }

    pub fn confirm_peer_identity(
        &mut self,
        actual_device_id: DeviceId,
    ) -> Result<PeerCertificateFingerprint> {
        let pending = self
            .pending_peer_trust
            .take()
            .ok_or_else(|| anyhow!("No pending outbound peer identity"))?;
        if actual_device_id != pending.expected_device_id {
            self.inner
                .connection
                .close(0u32.into(), b"peer identity mismatch");
            anyhow::bail!(
                "QUIC peer identity mismatch: expected {}, got {}",
                pending.expected_device_id,
                actual_device_id
            );
        }

        let fingerprint = pending.fingerprint.clone();
        let decision = match pending.decision {
            QuicTrustDecision::FirstSeen => QuicTrustStore::trust_first_seen_at(
                &pending.trust_store_path,
                pending.expected_device_id,
                pending.fingerprint,
            ),
            decision => Ok(decision),
        };
        match decision {
            Ok(QuicTrustDecision::FirstSeen) | Ok(QuicTrustDecision::Trusted) => {
                self.cert_trust_state = Some("trusted".to_string());
                Ok(fingerprint)
            }
            Ok(QuicTrustDecision::Rejected { expected, actual }) => {
                self.inner
                    .connection
                    .close(0u32.into(), b"certificate fingerprint mismatch");
                anyhow::bail!(
                    "QUIC certificate fingerprint changed for {} while confirming identity: expected {}, got {}",
                    actual_device_id,
                    expected,
                    actual
                );
            }
            Err(error) => {
                self.inner
                    .connection
                    .close(0u32.into(), b"failed to persist peer trust");
                Err(error)
            }
        }
    }

    pub fn confirm_inbound_peer_identity(
        &mut self,
        actual_device_id: DeviceId,
    ) -> Result<PeerCertificateFingerprint> {
        let fingerprint = self
            .peer_fingerprint
            .clone()
            .ok_or_else(|| anyhow!("peer certificate unavailable"))?;
        let decision = QuicTrustStore::trust_first_seen_at(
            &self.trust_store_path,
            actual_device_id,
            fingerprint.clone(),
        )?;
        match decision {
            QuicTrustDecision::FirstSeen | QuicTrustDecision::Trusted => {
                self.cert_trust_state = Some("trusted".to_string());
                Ok(fingerprint)
            }
            QuicTrustDecision::Rejected { expected, actual } => {
                anyhow::bail!(
                    "QUIC certificate fingerprint changed for {}: expected {}, got {}",
                    actual_device_id,
                    expected,
                    actual
                )
            }
        }
    }

    pub(crate) async fn complete_peer_protocol_handshake(&self) -> Result<()> {
        if self.inner.bootstrap_complete.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(mut bootstrap_stream) = self.inner.reliable_send_stream.lock().await.take() {
            bootstrap_stream
                .finish()
                .context("Failed to finish compatibility bootstrap stream")?;
        }

        wait_for_bootstrap_stream_verification(&self.inner).await?;
        self.inner
            .bootstrap_commit_requested
            .store(true, Ordering::Release);
        self.inner.bootstrap_commit_notify.notify_waiters();
        wait_for_bootstrap_reader_acks(&self.inner).await?;

        if let Some(reason) = self.inner.connection.close_reason() {
            anyhow::bail!("peer closed during compatibility bootstrap: {reason}");
        }
        self.inner.bootstrap_complete.store(true, Ordering::Release);
        self.inner.bootstrap_notify.notify_waiters();
        Ok(())
    }

    pub fn reject_pending_peer_identity(&mut self) {
        self.pending_peer_trust = None;
        self.inner
            .connection
            .close(0u32.into(), b"peer identity unavailable");
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    pub async fn send_message(&self, message: &Message) -> Result<()> {
        self.outbound_sender().send_message(message).await
    }

    fn outbound_sender(&self) -> OutboundSender {
        OutboundSender {
            send_channel: self.send_channel.clone(),
        }
    }

    pub async fn receive_message(&mut self) -> Result<Message> {
        let rx = self
            .message_rx
            .as_mut()
            .ok_or_else(|| anyhow!("Message channel already taken"))?;
        rx.recv()
            .await
            .ok_or_else(|| anyhow!("Message channel closed"))
    }

    pub fn message_channel(&mut self) -> mpsc::Receiver<Message> {
        self.message_rx
            .take()
            .expect("Message channel already taken")
    }

    pub fn is_connected(&self) -> bool {
        !self.send_channel.is_closed() && self.inner.connection.close_reason().is_none()
    }

    pub fn transport_name(&self) -> &'static str {
        "quic"
    }

    pub fn rtt_ms(&self) -> u64 {
        self.inner.connection.stats().path.rtt.as_millis() as u64
    }

    pub fn datagram_tx_dropped(&self) -> u64 {
        self.inner.datagram_tx_dropped.load(Ordering::Relaxed)
    }

    pub fn reliable_stream_reset_count(&self) -> u64 {
        self.inner
            .reliable_stream_reset_count
            .load(Ordering::Relaxed)
    }

    pub fn diagnostics(&self) -> TransportConnectionDiagnostics {
        TransportConnectionDiagnostics {
            address: self.remote_addr.to_string(),
            transport: self.transport_name(),
            datagram_available: self.inner.connection.max_datagram_size().is_some(),
            rtt_ms: Some(self.rtt_ms()),
            last_datagram_rx_ms: self.last_datagram_rx_ms(),
            datagram_tx_dropped: self.datagram_tx_dropped(),
            reliable_stream_reset_count: self.reliable_stream_reset_count(),
            cert_trust_state: self.cert_trust_state.clone(),
        }
    }

    pub fn last_datagram_rx_ms(&self) -> Option<u64> {
        let last_rx = self.inner.last_datagram_rx_us.load(Ordering::Relaxed);
        if last_rx == 0 {
            return None;
        }
        Some(current_timestamp_us().saturating_sub(last_rx) / 1_000)
    }

    pub async fn close(self) {
        self.inner.connection.close(0u32.into(), b"shutdown");
        drop(self);
        info!("Connection closed");
    }
}

pub struct ConnectionPool {
    _local_device_id: DeviceId,
    connections: Arc<TokioMutex<std::collections::HashMap<DeviceId, PooledConnection>>>,
}

struct PooledConnection {
    generation: u64,
    outbound: OutboundSender,
    connection: QuicConnection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanoutEnqueueFailureKind {
    QueueFull,
    QueueClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanoutEnqueueFailure {
    pub device_id: DeviceId,
    pub kind: FanoutEnqueueFailureKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanoutEnqueueReceipt {
    pub enqueued: Vec<DeviceId>,
    pub failures: Vec<FanoutEnqueueFailure>,
}

impl ConnectionPool {
    pub fn new(local_device_id: DeviceId) -> Self {
        Self {
            _local_device_id: local_device_id,
            connections: Arc::new(TokioMutex::new(std::collections::HashMap::new())),
        }
    }

    pub async fn insert(&self, device_id: DeviceId, conn: QuicConnection) {
        self.insert_with_generation(device_id, 0, conn).await;
    }

    pub(crate) async fn insert_with_generation(
        &self,
        device_id: DeviceId,
        generation: u64,
        conn: QuicConnection,
    ) {
        let mut conns = self.connections.lock().await;
        conns.insert(
            device_id,
            PooledConnection {
                generation,
                outbound: conn.outbound_sender(),
                connection: conn,
            },
        );
    }

    pub fn get(&self, _device_id: &DeviceId) -> Option<&'static QuicConnection> {
        None
    }

    pub async fn send_to(&self, device_id: &DeviceId, message: &Message) -> Result<()> {
        let outbound = self
            .connections
            .lock()
            .await
            .get(device_id)
            .map(|entry| entry.outbound.clone())
            .ok_or_else(|| anyhow!("No active connection for device {}", device_id))?;
        outbound.send_message(message).await
    }

    pub async fn diagnostics_for(
        &self,
        device_id: &DeviceId,
    ) -> Option<TransportConnectionDiagnostics> {
        let conns = self.connections.lock().await;
        conns
            .get(device_id)
            .filter(|entry| entry.connection.is_connected())
            .map(|entry| entry.connection.diagnostics())
    }

    pub async fn diagnostics_all(&self) -> Vec<(DeviceId, TransportConnectionDiagnostics)> {
        let conns = self.connections.lock().await;
        conns
            .iter()
            .filter(|(_, entry)| entry.connection.is_connected())
            .map(|(device_id, entry)| (*device_id, entry.connection.diagnostics()))
            .collect()
    }

    pub async fn remove(&self, device_id: &DeviceId) -> Option<QuicConnection> {
        let mut conns = self.connections.lock().await;
        conns.remove(device_id).map(|entry| entry.connection)
    }

    pub(crate) async fn remove_generation(
        &self,
        device_id: &DeviceId,
        generation: u64,
    ) -> Option<QuicConnection> {
        let mut conns = self.connections.lock().await;
        if conns
            .get(device_id)
            .is_some_and(|entry| entry.generation == generation)
        {
            conns.remove(device_id).map(|entry| entry.connection)
        } else {
            None
        }
    }

    pub fn count(&self) -> usize {
        let conns = self.connections.blocking_lock();
        conns.len()
    }

    pub async fn broadcast(&self, message: &Message) -> Result<()> {
        let peers: Vec<_> = self
            .connections
            .lock()
            .await
            .iter()
            .map(|(device_id, entry)| (*device_id, entry.outbound.clone()))
            .collect();
        let mut tasks = tokio::task::JoinSet::new();
        for (device_id, outbound) in peers {
            let message = message.clone();
            tasks.spawn(async move {
                (
                    device_id,
                    outbound
                        .send_message(&message)
                        .await
                        .map_err(|error| error.to_string()),
                )
            });
        }

        let mut failures = Vec::new();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((_device_id, Ok(()))) => {}
                Ok((device_id, Err(error))) => {
                    failures.push((device_id.to_string(), error));
                }
                Err(error) => failures.push(("join".into(), error.to_string())),
            }
        }
        if !failures.is_empty() {
            failures.sort();
            return Err(anyhow!(
                "broadcast failed: {}",
                failures
                    .into_iter()
                    .map(|(peer, error)| format!("{peer}: {error}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        Ok(())
    }

    pub async fn try_fanout(&self, message: &Message) -> FanoutEnqueueReceipt {
        let mut peers: Vec<_> = self
            .connections
            .lock()
            .await
            .iter()
            .map(|(device_id, entry)| (*device_id, entry.outbound.clone()))
            .collect();
        peers.sort_by_key(|(device_id, _)| device_id.to_string());

        let mut receipt = FanoutEnqueueReceipt {
            enqueued: Vec::with_capacity(peers.len()),
            failures: Vec::new(),
        };
        for (device_id, outbound) in peers {
            match outbound.try_send_message(message) {
                Ok(()) => receipt.enqueued.push(device_id),
                Err(kind) => receipt
                    .failures
                    .push(FanoutEnqueueFailure { device_id, kind }),
            }
        }
        receipt
    }

    pub async fn cleanup(&self) {
        let mut conns = self.connections.lock().await;
        conns.retain(|_id, entry| entry.connection.is_connected());
    }
}

fn trust_state_label(decision: &QuicTrustDecision) -> &'static str {
    match decision {
        QuicTrustDecision::FirstSeen => "first_seen",
        QuicTrustDecision::Trusted => "trusted",
        QuicTrustDecision::Rejected { .. } => "rejected",
    }
}

impl Clone for ConnectionPool {
    fn clone(&self) -> Self {
        Self {
            _local_device_id: self._local_device_id,
            connections: Arc::clone(&self.connections),
        }
    }
}

async fn send_outbound_message(
    inner: &Arc<QuicConnectionInner>,
    outbound_seq: &Arc<AtomicU32>,
    config: &TransportConfig,
    message: &Message,
) -> Result<()> {
    if !inner.bootstrap_complete.load(Ordering::Acquire) {
        if !super::handshake::is_bootstrap_message(message) {
            anyhow::bail!("non-bootstrap message sent before peer authentication");
        }
        let bootstrap_size = serde_json::to_vec(message)?.len();
        if bootstrap_size > super::handshake::BOOTSTRAP_MAX_MESSAGE_SIZE {
            anyhow::bail!(
                "compatibility bootstrap message exceeds {} bytes",
                super::handshake::BOOTSTRAP_MAX_MESSAGE_SIZE
            );
        }
    }
    let seq = outbound_seq.fetch_add(1, Ordering::Relaxed);

    if let Some(datagram) = RealtimeInputCodec::encode_message(seq, message)? {
        if let Some(max_datagram_size) = inner.connection.max_datagram_size() {
            if datagram.len() <= max_datagram_size {
                match inner.connection.send_datagram(Bytes::from(datagram)) {
                    Ok(()) => return Ok(()),
                    Err(error) => {
                        inner.datagram_tx_dropped.fetch_add(1, Ordering::Relaxed);
                        tracing::debug!("QUIC datagram send failed, using fallback: {}", error);
                    }
                }
            }
        }
    }

    let encoded = ControlMessageCodec::encode(message)?;
    if encoded.len() > config.max_message_size {
        anyhow::bail!("Reliable message too large: {} bytes", encoded.len());
    }
    write_reliable_frame(inner, &encoded).await
}

async fn write_reliable_frame(inner: &Arc<QuicConnectionInner>, payload: &[u8]) -> Result<()> {
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| anyhow!("Reliable payload too large"))?;
    let mut stream_guard = inner.reliable_send_stream.lock().await;
    if stream_guard.is_none() {
        let stream = inner
            .connection
            .open_uni()
            .await
            .context("Failed to open persistent QUIC send stream")?;
        *stream_guard = Some(stream);
    }

    let stream = stream_guard
        .as_mut()
        .ok_or_else(|| anyhow!("Persistent QUIC send stream missing"))?;
    if let Err(error) = stream.write_all(&payload_len.to_be_bytes()).await {
        *stream_guard = None;
        inner
            .reliable_stream_reset_count
            .fetch_add(1, Ordering::Relaxed);
        anyhow::bail!("Reliable length write failed: {}", error);
    }
    if let Err(error) = stream.write_all(payload).await {
        *stream_guard = None;
        inner
            .reliable_stream_reset_count
            .fetch_add(1, Ordering::Relaxed);
        anyhow::bail!("Reliable payload write failed: {}", error);
    }
    if let Err(error) = stream.flush().await {
        *stream_guard = None;
        inner
            .reliable_stream_reset_count
            .fetch_add(1, Ordering::Relaxed);
        anyhow::bail!("Reliable stream flush failed: {}", error);
    }
    Ok(())
}

async fn read_reliable_messages(
    inner: Arc<QuicConnectionInner>,
    message_tx: mpsc::Sender<Message>,
    max_message_size: usize,
) {
    let mut bootstrap_stream_seen = inner.bootstrap_complete.load(Ordering::Acquire);
    loop {
        if bootstrap_stream_seen
            && !inner.bootstrap_complete.load(Ordering::Acquire)
            && !await_reliable_bootstrap_commit(&inner).await
        {
            return;
        }

        let mut stream = match inner.connection.accept_uni().await {
            Ok(stream) => stream,
            Err(error) => {
                tracing::debug!("QUIC reliable accept stopped: {}", error);
                break;
            }
        };
        let bootstrap_stream =
            !bootstrap_stream_seen && !inner.bootstrap_complete.load(Ordering::Acquire);
        if !bootstrap_stream && !inner.bootstrap_complete.load(Ordering::Acquire) {
            tracing::warn!("Closing peer that opened a second stream before authentication");
            inner
                .connection
                .close(0u32.into(), b"stream opened before peer authentication");
            return;
        }

        loop {
            let mut len_buf = [0u8; 4];
            if let Err(error) = stream.read_exact(&mut len_buf).await {
                tracing::debug!("QUIC reliable frame length read stopped: {}", error);
                inner
                    .reliable_stream_reset_count
                    .fetch_add(1, Ordering::Relaxed);
                break;
            }

            let len = u32::from_be_bytes(len_buf) as usize;
            let frame_limit = if bootstrap_stream {
                max_message_size.min(super::handshake::BOOTSTRAP_MAX_MESSAGE_SIZE)
            } else {
                max_message_size
            };
            if len > frame_limit {
                tracing::warn!("Dropping oversized QUIC reliable frame: {} bytes", len);
                inner
                    .connection
                    .close(0u32.into(), b"reliable frame exceeds active protocol limit");
                break;
            }

            let mut data = vec![0u8; len];
            if let Err(error) = stream.read_exact(&mut data).await {
                tracing::debug!("QUIC reliable payload read stopped: {}", error);
                inner
                    .reliable_stream_reset_count
                    .fetch_add(1, Ordering::Relaxed);
                break;
            }

            match ControlMessageCodec::decode(&data) {
                Ok(msg) => {
                    if bootstrap_stream && !super::handshake::is_bootstrap_message(&msg) {
                        tracing::warn!(
                            "Closing peer that sent non-bootstrap message before authentication"
                        );
                        inner
                            .connection
                            .close(0u32.into(), b"illegal compatibility bootstrap message");
                        return;
                    }
                    if message_tx.send(msg).await.is_err() {
                        return;
                    }
                    if bootstrap_stream {
                        // The compatibility stream is permanently bootstrap-only.
                        // Normal traffic must arrive on a newly opened stream after
                        // both peers have completed authentication.
                        bootstrap_stream_seen = true;
                        if !enforce_bootstrap_stream_eof(&inner, &mut stream).await {
                            return;
                        }
                        break;
                    }
                }
                Err(error) => tracing::debug!("Failed to decode reliable message: {}", error),
            }
        }
    }
}

async fn enforce_bootstrap_stream_eof(
    inner: &Arc<QuicConnectionInner>,
    stream: &mut quinn::RecvStream,
) -> bool {
    let mut trailing = [0u8; 1];
    match tokio::time::timeout(
        super::handshake::BOOTSTRAP_TIMEOUT,
        stream.read(&mut trailing),
    )
    .await
    {
        Ok(Ok(None)) => {
            inner
                .bootstrap_stream_verified
                .store(true, Ordering::Release);
            inner.bootstrap_stream_notify.notify_waiters();
            true
        }
        Ok(Ok(Some(_))) => {
            tracing::warn!("Closing peer that appended data to the bootstrap-only stream");
            inner
                .connection
                .close(0u32.into(), b"bootstrap stream carried trailing data");
            false
        }
        Ok(Err(error)) => {
            tracing::debug!("Bootstrap stream close check failed: {}", error);
            inner
                .connection
                .close(0u32.into(), b"bootstrap stream close check failed");
            false
        }
        Err(_) => {
            tracing::warn!("Closing peer that did not finish the bootstrap-only stream");
            inner
                .connection
                .close(0u32.into(), b"bootstrap stream did not finish");
            false
        }
    }
}

async fn await_reliable_bootstrap_commit(inner: &Arc<QuicConnectionInner>) -> bool {
    let commit_notified = inner.bootstrap_commit_notify.notified();
    tokio::pin!(commit_notified);
    commit_notified.as_mut().enable();
    if inner.bootstrap_commit_requested.load(Ordering::Acquire) {
        return guard_reliable_until_authenticated(inner).await;
    }
    tokio::select! {
        biased;
        stream = inner.connection.accept_uni() => {
            if stream.is_ok() {
                tracing::warn!("Closing peer that prequeued a second stream before authentication");
                inner.connection.close(0u32.into(), b"pre-authentication stream");
            }
            false
        }
        _ = commit_notified => {
            guard_reliable_until_authenticated(inner).await
        }
    }
}

async fn guard_reliable_until_authenticated(inner: &Arc<QuicConnectionInner>) -> bool {
    let authenticated = inner.bootstrap_notify.notified();
    tokio::pin!(authenticated);
    authenticated.as_mut().enable();
    mark_bootstrap_reader_ack(inner, BOOTSTRAP_RELIABLE_READER_ACK);
    tokio::select! {
        biased;
        stream = inner.connection.accept_uni() => {
            if stream.is_ok() {
                tracing::warn!("Closing peer that opened a stream during bootstrap commit");
                inner.connection.close(0u32.into(), b"stream during bootstrap commit");
            }
            false
        }
        _ = authenticated => true,
        _ = inner.connection.closed() => false,
    }
}

async fn read_realtime_datagrams(
    inner: Arc<QuicConnectionInner>,
    message_tx: mpsc::Sender<Message>,
) {
    if let Some(barrier) = &inner.datagram_reader_start_barrier {
        barrier.notified().await;
    }
    if !inner.bootstrap_complete.load(Ordering::Acquire) {
        let commit_notified = inner.bootstrap_commit_notify.notified();
        tokio::pin!(commit_notified);
        commit_notified.as_mut().enable();
        if !inner.bootstrap_commit_requested.load(Ordering::Acquire) {
            tokio::select! {
                biased;
                datagram = inner.connection.read_datagram() => {
                    match datagram {
                        Ok(_) => {
                            tracing::warn!("Closing peer that prequeued a datagram before authentication");
                            inner.connection.close(0u32.into(), b"datagram before peer authentication");
                        }
                        Err(error) => {
                            tracing::debug!("QUIC datagram reader stopped during bootstrap: {}", error);
                        }
                    }
                    return;
                }
                _ = commit_notified => {}
            }
        }

        let authenticated = inner.bootstrap_notify.notified();
        tokio::pin!(authenticated);
        authenticated.as_mut().enable();
        mark_bootstrap_reader_ack(&inner, BOOTSTRAP_DATAGRAM_READER_ACK);
        tokio::select! {
            biased;
            datagram = inner.connection.read_datagram() => {
                if datagram.is_ok() {
                    tracing::warn!("Closing peer that queued a datagram during bootstrap commit");
                    inner.connection.close(0u32.into(), b"datagram during bootstrap commit");
                }
                return;
            }
            _ = authenticated => {}
            _ = inner.connection.closed() => return,
        }
    }

    loop {
        match inner.connection.read_datagram().await {
            Ok(datagram) => match RealtimeInputCodec::decode_message(&datagram) {
                Ok(message) => {
                    inner
                        .last_datagram_rx_us
                        .store(current_timestamp_us(), Ordering::Relaxed);
                    if message_tx.send(message).await.is_err() {
                        break;
                    }
                }
                Err(error) => tracing::debug!("Failed to decode realtime datagram: {}", error),
            },
            Err(error) => {
                tracing::debug!("QUIC datagram reader stopped: {}", error);
                break;
            }
        }
    }
}

fn mark_bootstrap_reader_ack(inner: &QuicConnectionInner, ack: u8) {
    inner.bootstrap_commit_acks.fetch_or(ack, Ordering::AcqRel);
    inner.bootstrap_commit_ack_notify.notify_waiters();
}

async fn wait_for_bootstrap_stream_verification(inner: &Arc<QuicConnectionInner>) -> Result<()> {
    tokio::time::timeout(super::handshake::BOOTSTRAP_TIMEOUT, async {
        loop {
            let verified = inner.bootstrap_stream_notify.notified();
            tokio::pin!(verified);
            verified.as_mut().enable();
            if inner.bootstrap_stream_verified.load(Ordering::Acquire) {
                return Ok(());
            }
            tokio::select! {
                _ = verified => {}
                reason = inner.connection.closed() => {
                    anyhow::bail!("peer closed before bootstrap stream verification: {reason}");
                }
            }
        }
    })
    .await
    .map_err(|_| anyhow!("bootstrap stream verification timed out"))?
}

async fn wait_for_bootstrap_reader_acks(inner: &Arc<QuicConnectionInner>) -> Result<()> {
    tokio::time::timeout(super::handshake::BOOTSTRAP_TIMEOUT, async {
        loop {
            let acknowledged = inner.bootstrap_commit_ack_notify.notified();
            tokio::pin!(acknowledged);
            acknowledged.as_mut().enable();
            if inner.bootstrap_commit_acks.load(Ordering::Acquire) == BOOTSTRAP_ALL_READERS_ACKED {
                return Ok(());
            }
            tokio::select! {
                _ = acknowledged => {}
                reason = inner.connection.closed() => {
                    anyhow::bail!("peer closed during bootstrap reader commit: {reason}");
                }
            }
        }
    })
    .await
    .map_err(|_| anyhow!("bootstrap reader commit timed out"))?
}

fn make_server_config(
    identity: &QuicIdentity,
    config: &TransportConfig,
) -> Result<QuinnServerConfig> {
    ensure_rustls_crypto_provider();
    let cert = CertificateDer::from(identity.cert_der.clone());
    let key = PrivatePkcs8KeyDer::from(identity.key_der.clone());
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let rustls_config = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("Failed to select TLS 1.3 for QUIC")?
        .with_client_cert_verifier(Arc::new(OptionalBootstrapClientVerifier(provider)))
        .with_single_cert(vec![cert], PrivateKeyDer::Pkcs8(key))
        .context("Failed to build QUIC server identity")?;
    let crypto = QuicServerConfig::try_from(rustls_config)
        .map_err(|error| anyhow!("Failed to build QUIC server crypto: {error}"))?;
    let mut server_config = QuinnServerConfig::with_crypto(Arc::new(crypto));
    server_config.transport_config(Arc::new(make_quinn_transport_config(config)?));
    Ok(server_config)
}

fn make_client_config(
    identity: &QuicIdentity,
    config: &TransportConfig,
    present_client_certificate: bool,
) -> Result<ClientConfig> {
    ensure_rustls_crypto_provider();
    let builder = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(TofuServerVerifier::new()));
    let rustls_config = if present_client_certificate {
        builder
            .with_client_auth_cert(
                vec![CertificateDer::from(identity.cert_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key_der.clone())),
            )
            .context("Failed to build QUIC client identity")?
    } else {
        builder.with_no_client_auth()
    };
    let crypto = QuicClientConfig::try_from(rustls_config)
        .map_err(|error| anyhow!("Failed to build QUIC client crypto: {error}"))?;
    let mut client_config = ClientConfig::new(Arc::new(crypto));
    client_config.transport_config(Arc::new(make_quinn_transport_config(config)?));
    Ok(client_config)
}

fn ensure_rustls_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

fn make_quinn_transport_config(config: &TransportConfig) -> Result<QuinnTransportConfig> {
    let mut transport = QuinnTransportConfig::default();
    transport.max_idle_timeout(Some(
        IdleTimeout::try_from(config.max_idle_timeout)
            .map_err(|_| anyhow!("Invalid QUIC idle timeout"))?,
    ));
    transport.keep_alive_interval(Some(config.keep_alive_interval));
    transport.datagram_receive_buffer_size(Some(config.datagram_receive_buffer_size));
    transport.datagram_send_buffer_size(config.datagram_send_buffer_size);
    transport.initial_rtt(Duration::from_millis(20));
    transport.send_window(512 * 1024);
    transport.receive_window(VarInt::from_u32(512 * 1024));
    transport.stream_receive_window(VarInt::from_u32(128 * 1024));
    Ok(transport)
}

async fn inspect_outbound_peer_trust(
    connection: &quinn::Connection,
    device_id: DeviceId,
    trust_store_path: PathBuf,
) -> Result<PendingPeerTrust> {
    let fingerprint = match peer_certificate_fingerprint(connection) {
        Some(fingerprint) => fingerprint,
        None => {
            connection.close(0u32.into(), b"peer certificate unavailable");
            anyhow::bail!("QUIC peer did not present a certificate");
        }
    };
    let store = match QuicTrustStore::load(&trust_store_path) {
        Ok(store) => store,
        Err(error) => {
            connection.close(0u32.into(), b"peer trust store unavailable");
            return Err(error);
        }
    };
    let decision = store.check(device_id, &fingerprint);
    match &decision {
        QuicTrustDecision::FirstSeen => {}
        QuicTrustDecision::Trusted => {}
        QuicTrustDecision::Rejected { expected, actual } => {
            connection.close(0u32.into(), b"certificate fingerprint mismatch");
            anyhow::bail!(
                "QUIC certificate fingerprint changed for {}: expected {}, got {}",
                device_id,
                expected,
                actual
            );
        }
    }
    Ok(PendingPeerTrust {
        expected_device_id: device_id,
        fingerprint,
        trust_store_path,
        decision,
    })
}

fn peer_certificate_fingerprint(
    connection: &quinn::Connection,
) -> Option<PeerCertificateFingerprint> {
    let identity: Box<dyn Any> = connection.peer_identity()?;
    let certs = identity.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    certs
        .first()
        .map(|cert| PeerCertificateFingerprint::from_der(cert.as_ref()))
}

fn current_timestamp_us() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

#[derive(Debug)]
struct TofuServerVerifier(Arc<rustls::crypto::CryptoProvider>);

impl TofuServerVerifier {
    fn new() -> Self {
        Self(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
    }
}

impl ServerCertVerifier for TofuServerVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[derive(Debug)]
struct OptionalBootstrapClientVerifier(Arc<rustls::crypto::CryptoProvider>);

impl ClientCertVerifier for OptionalBootstrapClientVerifier {
    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, RustlsError> {
        // Self-signed identities are authorized only after the claimed DeviceId
        // is checked against TOFU during the application bootstrap.
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::SubjectPublicKeyInfoDer;
    use rustls::sign::{Signer, SigningKey};
    use rustls::{SignatureAlgorithm, SignatureScheme};
    use tokio::time::timeout;

    #[derive(Debug)]
    struct CorruptSigningKey(Arc<dyn SigningKey>);

    impl SigningKey for CorruptSigningKey {
        fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
            self.0
                .choose_scheme(offered)
                .map(|signer| Box::new(CorruptSigner(signer)) as Box<dyn Signer>)
        }

        fn public_key(&self) -> Option<SubjectPublicKeyInfoDer<'_>> {
            self.0.public_key()
        }

        fn algorithm(&self) -> SignatureAlgorithm {
            self.0.algorithm()
        }
    }

    #[derive(Debug)]
    struct CorruptSigner(Box<dyn Signer>);

    impl Signer for CorruptSigner {
        fn sign(&self, message: &[u8]) -> std::result::Result<Vec<u8>, RustlsError> {
            let mut signature = self.0.sign(message)?;
            if let Some(byte) = signature.first_mut() {
                *byte ^= 0x80;
            }
            Ok(signature)
        }

        fn scheme(&self) -> SignatureScheme {
            self.0.scheme()
        }
    }

    #[tokio::test]
    async fn optional_client_verifier_rejects_invalid_certificate_verify_signature() {
        ensure_rustls_crypto_provider();
        let server_id = DeviceId::new_v4();
        let mut server = QuicTransport::new(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();

        let identity = generated_test_identity();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let signing_key = provider
            .key_provider
            .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                identity.key_der,
            )))
            .unwrap();
        let certified_key = rustls::sign::CertifiedKey::new(
            vec![CertificateDer::from(identity.cert_der)],
            Arc::new(CorruptSigningKey(signing_key)),
        );
        let rustls_config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(TofuServerVerifier::new()))
            .with_client_cert_resolver(Arc::new(rustls::sign::SingleCertAndKey::from(
                certified_key,
            )));
        let crypto = QuicClientConfig::try_from(rustls_config).unwrap();
        let mut endpoint = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(ClientConfig::new(Arc::new(crypto)));

        let connection = endpoint
            .connect(address, "rshare.local")
            .unwrap()
            .await
            .unwrap();
        let result = timeout(Duration::from_secs(1), connection.closed()).await;
        assert!(
            result.is_ok(),
            "invalid CertificateVerify signature must close the TLS connection"
        );
    }

    fn generated_test_identity() -> QuicIdentity {
        let (cert_der, key_der) = Encryption::generate_cert().unwrap();
        QuicIdentity { cert_der, key_der }
    }

    async fn write_raw_reliable_payloads(connection: &QuicConnection, payloads: &[Vec<u8>]) {
        let mut stream = connection.inner.connection.open_uni().await.unwrap();
        for payload in payloads {
            stream
                .write_all(&(payload.len() as u32).to_be_bytes())
                .await
                .unwrap();
            stream.write_all(payload).await.unwrap();
        }
        stream.finish().unwrap();
    }

    #[tokio::test]
    async fn raw_bootstrap_stream_with_prequeued_input_is_closed() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::new(server_id);
        server.require_peer_protocol_handshake();
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::new(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let mut server_connection = incoming.recv().await.unwrap().connection;
        let hello = ControlMessageCodec::encode(&rshare_core::hello_message(
            client_id,
            "client".into(),
            "host".into(),
        ))
        .unwrap();
        let input = ControlMessageCodec::encode(&Message::MouseMove { x: 99, y: 101 }).unwrap();

        write_raw_reliable_payloads(&client_connection, &[hello, input]).await;
        assert!(matches!(
            server_connection.receive_message().await.unwrap(),
            Message::Hello { .. }
        ));
        assert!(
            server_connection
                .complete_peer_protocol_handshake()
                .await
                .is_err(),
            "bootstrap completion must fail when the bootstrap stream carries trailing input"
        );

        timeout(
            Duration::from_secs(1),
            server_connection.inner.connection.closed(),
        )
        .await
        .expect("receiver must close a bootstrap stream with trailing input");
        assert!(
            timeout(
                Duration::from_millis(100),
                server_connection.receive_message()
            )
            .await
            .is_ok_and(|message| message.is_err()),
            "prequeued input must never enter the decoded message channel"
        );
    }

    #[tokio::test]
    async fn raw_second_stream_prequeued_before_auth_never_reaches_manager() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut manager = crate::connection::ConnectionManager::new(server_id);
        let mut events = manager.events().unwrap();
        manager.start_server("127.0.0.1:0").await.unwrap();
        let address = manager.transport_local_addr().unwrap();

        let mut client = QuicTransport::new(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let hello = ControlMessageCodec::encode(&rshare_core::hello_message(
            client_id,
            "client".into(),
            "host".into(),
        ))
        .unwrap();
        let input = ControlMessageCodec::encode(&Message::MouseMove { x: 7, y: 9 }).unwrap();

        write_raw_reliable_payloads(&client_connection, &[hello]).await;
        write_raw_reliable_payloads(&client_connection, &[input]).await;

        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("pre-authentication second stream must close the QUIC connection");
        while let Ok(Some(event)) = timeout(Duration::from_millis(50), events.recv()).await {
            assert!(
                !matches!(
                    event,
                    crate::connection::ManagerEvent::Connected(id) if id == client_id
                ),
                "pre-authentication second stream must prevent registration"
            );
            assert!(
                !matches!(
                    event,
                    crate::connection::ManagerEvent::MessageReceived {
                        from,
                        message: Message::MouseMove { .. }
                    } if from == client_id
                ),
                "pre-authentication second stream leaked into ManagerEvent"
            );
        }
        assert!(manager.connections().is_empty());
    }

    #[tokio::test]
    async fn raw_datagram_prequeued_before_auth_never_reaches_manager() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let reader_barrier = Arc::new(Notify::new());
        let server_transport =
            QuicTransport::new(server_id).with_test_datagram_reader_barrier(reader_barrier.clone());
        let mut manager =
            crate::connection::ConnectionManager::with_transport(server_id, server_transport);
        let mut events = manager.events().unwrap();
        manager.start_server("127.0.0.1:0").await.unwrap();
        let address = manager.transport_local_addr().unwrap();

        let mut client = QuicTransport::new(client_id);
        let mut client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let datagram = RealtimeInputCodec::encode_message(1, &Message::MouseMove { x: 13, y: 17 })
            .unwrap()
            .unwrap();
        client_connection
            .inner
            .connection
            .send_datagram(Bytes::from(datagram))
            .unwrap();
        let hello = ControlMessageCodec::encode(&rshare_core::hello_message(
            client_id,
            "client".into(),
            "host".into(),
        ))
        .unwrap();
        write_raw_reliable_payloads(&client_connection, &[hello]).await;

        assert!(matches!(
            timeout(Duration::from_secs(1), client_connection.receive_message())
                .await
                .unwrap()
                .unwrap(),
            Message::HelloBack { .. }
        ));
        reader_barrier.notify_one();

        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("pre-authentication datagram must close the QUIC connection");
        while let Ok(Some(event)) = timeout(Duration::from_millis(50), events.recv()).await {
            assert!(
                !matches!(
                    event,
                    crate::connection::ManagerEvent::Connected(id) if id == client_id
                ),
                "pre-authentication datagram must prevent registration"
            );
            assert!(
                !matches!(
                    event,
                    crate::connection::ManagerEvent::MessageReceived {
                        from,
                        message: Message::MouseMove { .. }
                    } if from == client_id
                ),
                "pre-authentication datagram leaked into ManagerEvent"
            );
        }
        assert!(manager.connections().is_empty());
    }

    #[tokio::test]
    async fn raw_oversized_bootstrap_length_is_closed_before_payload_read() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::new(server_id);
        server.require_peer_protocol_handshake();
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::new(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let mut server_connection = incoming.recv().await.unwrap().connection;
        let mut stream = client_connection.inner.connection.open_uni().await.unwrap();
        let oversized = (super::super::handshake::BOOTSTRAP_MAX_MESSAGE_SIZE as u32) + 1;
        stream.write_all(&oversized.to_be_bytes()).await.unwrap();
        stream.finish().unwrap();

        timeout(
            Duration::from_secs(1),
            server_connection.inner.connection.closed(),
        )
        .await
        .expect("receiver must close on an oversized bootstrap length prefix");
        assert!(
            timeout(
                Duration::from_millis(100),
                server_connection.receive_message()
            )
            .await
            .is_ok_and(|message| message.is_err()),
            "oversized bootstrap must never allocate and publish a decoded message"
        );
    }

    #[test]
    fn test_transport_new() {
        let transport = QuicTransport::new(DeviceId::new_v4());
        assert!(!transport.is_running());
    }

    #[tokio::test]
    async fn start_server_marks_transport_running() {
        let mut transport = QuicTransport::new(DeviceId::new_v4());

        transport.start_server("127.0.0.1:0").await.unwrap();

        assert!(transport.is_running());

        transport.close().await.unwrap();
        assert!(!transport.is_running());
    }

    #[test]
    fn test_connection_pool() {
        let pool = ConnectionPool::new(DeviceId::new_v4());
        assert_eq!(pool.count(), 0);
    }

    #[tokio::test]
    async fn connection_pool_send_to_missing_connection_returns_error() {
        let pool = ConnectionPool::new(DeviceId::new_v4());
        let missing_id = DeviceId::new_v4();

        let result = pool
            .send_to(&missing_id, &Message::MouseMove { x: 1, y: 2 })
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No active connection"));
    }

    #[tokio::test]
    async fn connection_pool_does_not_hold_global_lock_while_target_send_waits() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut server = QuicTransport::new(local_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::new(remote_id);
        let sender = client
            .connect(&server_addr.to_string(), local_id)
            .await
            .unwrap();
        let receiver = incoming.recv().await.unwrap().connection;
        let pool = ConnectionPool::new(remote_id);
        pool.insert(local_id, sender).await;

        let (blocked_tx, mut blocked_rx) = mpsc::channel(1);
        {
            let mut connections = pool.connections.lock().await;
            connections.get_mut(&local_id).unwrap().outbound = OutboundSender {
                send_channel: blocked_tx,
            };
        }

        let sending_pool = pool.clone();
        let send_task = tokio::spawn(async move {
            sending_pool
                .send_to(&local_id, &Message::MouseMove { x: 1, y: 2 })
                .await
        });
        let blocked_frame = timeout(Duration::from_secs(1), blocked_rx.recv())
            .await
            .unwrap()
            .unwrap();

        let lock = timeout(Duration::from_millis(50), pool.connections.lock()).await;
        assert!(
            lock.is_ok(),
            "a slow target send must not retain the pool-wide mutex"
        );

        drop(blocked_frame);
        assert!(send_task.await.unwrap().is_err());
        drop(receiver);
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn broadcast_slow_peer_does_not_delay_fast_peer_or_hold_pool_lock() {
        let server_id = DeviceId::new_v4();
        let fast_id = DeviceId::new_v4();
        let slow_id = DeviceId::new_v4();
        let mut server = QuicTransport::new(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut fast_client = QuicTransport::new(fast_id);
        let fast_sender = fast_client
            .connect(&server_addr.to_string(), server_id)
            .await
            .unwrap();
        let mut fast_receiver = incoming.recv().await.unwrap().connection;
        let mut fast_messages = fast_receiver.message_channel();

        let mut slow_client = QuicTransport::new(slow_id);
        let slow_sender = slow_client
            .connect(&server_addr.to_string(), server_id)
            .await
            .unwrap();
        let slow_receiver = incoming.recv().await.unwrap().connection;

        let pool = ConnectionPool::new(server_id);
        pool.insert(fast_id, fast_sender).await;
        pool.insert(slow_id, slow_sender).await;
        let (blocked_tx, mut blocked_rx) = mpsc::channel(1);
        {
            let mut connections = pool.connections.lock().await;
            connections.get_mut(&slow_id).unwrap().outbound = OutboundSender {
                send_channel: blocked_tx,
            };
        }

        let broadcast_pool = pool.clone();
        let broadcast_task = tokio::spawn(async move {
            broadcast_pool
                .broadcast(&Message::MouseMove { x: 7, y: 11 })
                .await
        });
        let blocked_frame = timeout(Duration::from_secs(1), blocked_rx.recv())
            .await
            .expect("slow peer must enter the production outbound path")
            .expect("slow peer outbound channel must remain open");

        assert!(
            matches!(
                timeout(Duration::from_millis(100), fast_messages.recv())
                    .await
                    .expect("fast peer must receive while the slow peer is blocked"),
                Some(Message::MouseMove { x: 7, y: 11 })
            ),
            "fast peer must receive the broadcast payload"
        );
        assert!(
            timeout(Duration::from_millis(50), pool.connections.lock())
                .await
                .is_ok(),
            "broadcast must release the pool-wide mutex before awaiting peer sends"
        );

        blocked_frame
            .ack
            .send(Err("simulated slow-peer backpressure failure".into()))
            .unwrap();
        let error = timeout(Duration::from_secs(1), broadcast_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(error.contains(&slow_id.to_string()));
        assert!(error.contains("simulated slow-peer backpressure failure"));

        drop(slow_receiver);
        fast_receiver.close().await;
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn nonblocking_fanout_keeps_fast_peer_moving_when_slow_queue_fills() {
        let server_id = DeviceId::new_v4();
        let fast_id = DeviceId::new_v4();
        let slow_id = DeviceId::new_v4();
        let mut server = QuicTransport::new(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut fast_client = QuicTransport::new(fast_id);
        let fast_sender = fast_client
            .connect(&server_addr.to_string(), server_id)
            .await
            .unwrap();
        let mut fast_receiver = incoming.recv().await.unwrap().connection;
        let mut fast_messages = fast_receiver.message_channel();

        let mut slow_client = QuicTransport::new(slow_id);
        let slow_sender = slow_client
            .connect(&server_addr.to_string(), server_id)
            .await
            .unwrap();
        let slow_receiver = incoming.recv().await.unwrap().connection;

        let pool = ConnectionPool::new(server_id);
        pool.insert(fast_id, fast_sender).await;
        pool.insert(slow_id, slow_sender).await;
        let (blocked_tx, mut blocked_rx) = mpsc::channel(1);
        {
            let mut connections = pool.connections.lock().await;
            connections.get_mut(&slow_id).unwrap().outbound = OutboundSender {
                send_channel: blocked_tx,
            };
        }

        let first = pool.try_fanout(&Message::MouseMove { x: 1, y: 0 }).await;
        assert!(first.failures.is_empty());
        let mut expected_order = vec![fast_id, slow_id];
        expected_order.sort_by_key(DeviceId::to_string);
        assert_eq!(first.enqueued, expected_order);
        let blocked_first = blocked_rx
            .recv()
            .await
            .expect("slow peer must retain the first frame without acknowledging it");

        let second = pool.try_fanout(&Message::MouseMove { x: 2, y: 0 }).await;
        assert!(second.failures.is_empty());
        assert_eq!(second.enqueued, expected_order);
        for expected_x in [1, 2] {
            assert!(matches!(
                timeout(Duration::from_millis(100), fast_messages.recv())
                    .await
                    .expect("fast peer must receive consecutive fanout frames"),
                Some(Message::MouseMove { x, y: 0 }) if x == expected_x
            ));
        }
        assert!(
            timeout(Duration::from_millis(50), pool.connections.lock())
                .await
                .is_ok(),
            "nonblocking fanout must release the pool lock before peer queue work"
        );

        let third = pool.try_fanout(&Message::MouseMove { x: 3, y: 0 }).await;
        assert_eq!(third.enqueued, vec![fast_id]);
        assert_eq!(third.failures.len(), 1);
        assert_eq!(third.failures[0].device_id, slow_id);
        assert_eq!(third.failures[0].kind, FanoutEnqueueFailureKind::QueueFull);
        assert!(matches!(
            timeout(Duration::from_millis(100), fast_messages.recv())
                .await
                .expect("fast peer must continue after only the slow queue overflows"),
            Some(Message::MouseMove { x: 3, y: 0 })
        ));

        drop(blocked_first);
        drop(slow_receiver);
        fast_receiver.close().await;
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn quinn_loopback_sends_mouse_move_datagram() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut server = QuicTransport::new(local_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server
            .server_endpoint
            .as_ref()
            .unwrap()
            .local_addr()
            .unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::new(remote_id);
        let sender = client
            .connect(&server_addr.to_string(), local_id)
            .await
            .unwrap();
        let mut receiver = incoming.recv().await.unwrap().connection;
        let mut messages = receiver.message_channel();

        sender
            .send_message(&Message::MouseMove { x: 42, y: 24 })
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_secs(1), messages.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(received, Message::MouseMove { x: 42, y: 24 }));
    }

    #[tokio::test]
    async fn quinn_loopback_sends_key_over_reliable_stream() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut server = QuicTransport::new(local_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server
            .server_endpoint
            .as_ref()
            .unwrap()
            .local_addr()
            .unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::new(remote_id);
        let sender = client
            .connect(&server_addr.to_string(), local_id)
            .await
            .unwrap();
        let mut receiver = incoming.recv().await.unwrap().connection;
        let mut messages = receiver.message_channel();

        sender
            .send_message(&Message::Key {
                keycode: 65,
                state: rshare_core::KeyState::Pressed,
            })
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_secs(1), messages.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            received,
            Message::Key {
                keycode: 65,
                state: rshare_core::KeyState::Pressed
            }
        ));
    }
}
