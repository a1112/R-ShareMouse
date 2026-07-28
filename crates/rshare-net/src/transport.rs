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
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Once;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, watch, Mutex as TokioMutex, Notify, Semaphore};

use super::codec::ControlMessageCodec;
use super::encryption::{
    Encryption, PeerCertificateFingerprint, QuicIdentity, QuicTrustDecision, QuicTrustStore,
};
use super::handshake::PeerAuthContext;
use super::qos::{
    BulkFrame, ControlFrame, LaneDiscriminator, PeerTransportHandle, TelemetryFrame,
    TerminalReleaseEmitter, TerminalReleaseEvent, AWAITED_CANCEL_RESET_CODE, QOS_LANE_MAGIC,
    TERMINAL_CANCEL_RESET_CODE,
};
use rshare_core::{
    ControlConnectionId, DeviceId, Message, RealtimeInputFrame, ReliableInputEvent,
    ReliableInputFrame, SessionEpoch,
};
use tracing::info;

const BOOTSTRAP_RELIABLE_READER_ACK: u8 = 0b01;
const BOOTSTRAP_DATAGRAM_READER_ACK: u8 = 0b10;
const BOOTSTRAP_ALL_READERS_ACKED: u8 =
    BOOTSTRAP_RELIABLE_READER_ACK | BOOTSTRAP_DATAGRAM_READER_ACK;
const AUTHENTICATED_UNI_STREAM_TASK_BUDGET: usize = 32;
const AUTHENTICATED_UNI_STREAM_BUDGET_EXHAUSTED_CODE: u32 = 0x525342;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuicServerStartError {
    #[error("QUIC transport server is already registered; call close before starting it again")]
    AlreadyRunning,
}

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

/// Isolated inbound lanes for one authenticated peer connection generation.
pub struct PeerInbound {
    pub auth: Arc<PeerAuthContext>,
    pub realtime_rx: mpsc::Receiver<RealtimeInputFrame>,
    pub reliable_input_rx: mpsc::Receiver<ReliableInputFrame>,
    pub control_rx: mpsc::Receiver<ControlFrame>,
    pub telemetry_rx: mpsc::Receiver<TelemetryFrame>,
    pub bulk_rx: mpsc::Receiver<BulkFrame>,
}

#[derive(Debug, Clone)]
pub(crate) struct TransportProtocolError {
    pub auth: Arc<PeerAuthContext>,
    pub error: String,
}

struct InstalledInboundReceivers {
    peer: Option<PeerInbound>,
    control_events: Option<mpsc::Receiver<ControlFrame>>,
    protocol_errors: Option<mpsc::Receiver<TransportProtocolError>>,
}

#[derive(Clone)]
struct LatestRealtimeEmitter {
    latest_tx: watch::Sender<Option<RealtimeInputFrame>>,
    #[cfg(test)]
    workers: Arc<AtomicUsize>,
}

#[cfg(test)]
struct LatestRealtimeWorkerGuard(Arc<AtomicUsize>);

#[cfg(test)]
impl Drop for LatestRealtimeWorkerGuard {
    fn drop(&mut self) {
        self.0.store(0, Ordering::Release);
    }
}

impl LatestRealtimeEmitter {
    fn new(target: mpsc::Sender<RealtimeInputFrame>) -> Self {
        let (latest_tx, mut latest_rx) = watch::channel(None::<RealtimeInputFrame>);
        #[cfg(test)]
        let workers = Arc::new(AtomicUsize::new(1));
        #[cfg(test)]
        let worker_guard = LatestRealtimeWorkerGuard(workers.clone());
        tokio::spawn(async move {
            #[cfg(test)]
            let _worker_guard = worker_guard;
            while latest_rx.changed().await.is_ok() {
                let mut latest = latest_rx.borrow_and_update().clone();
                loop {
                    let Some(frame) = latest.take() else {
                        break;
                    };
                    let send = target.send(frame);
                    tokio::pin!(send);
                    tokio::select! {
                        biased;
                        changed = latest_rx.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            latest = latest_rx.borrow_and_update().clone();
                        }
                        sent = &mut send => {
                            if sent.is_err() {
                                return;
                            }
                            if latest_rx.has_changed().unwrap_or(false) {
                                latest = latest_rx.borrow_and_update().clone();
                            }
                        }
                    }
                }
            }
        });
        Self {
            latest_tx,
            #[cfg(test)]
            workers,
        }
    }

    fn emit(&self, frame: RealtimeInputFrame) {
        self.latest_tx.send_replace(Some(frame));
    }

    #[cfg(test)]
    fn worker_probe_for_test(&self) -> Arc<AtomicUsize> {
        self.workers.clone()
    }
}

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
    state_lifetime: Option<Arc<StateLifetimeOwner>>,
    #[cfg(test)]
    accept_task_barrier: Option<Arc<Notify>>,
    #[cfg(test)]
    accept_task_waiting: Arc<AtomicBool>,
}

#[derive(Debug)]
struct StateLifetimeOwner {
    state_dir: PathBuf,
    scratch_root: PathBuf,
}

impl StateLifetimeOwner {
    #[cfg(test)]
    fn isolated(state_dir: PathBuf) -> Self {
        Self {
            state_dir,
            scratch_root: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("rshare-state"),
        }
    }

    fn is_safe_isolated_state_dir(&self) -> bool {
        self.state_dir.parent() == Some(self.scratch_root.as_path())
            && self
                .state_dir
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    uuid::Uuid::parse_str(name).is_ok_and(|id| id.to_string() == name)
                })
    }
}

impl Drop for StateLifetimeOwner {
    fn drop(&mut self) {
        if self.is_safe_isolated_state_dir() {
            let _ = fs::remove_dir_all(&self.state_dir);
        }
    }
}

pub struct IncomingConnection {
    pub device_id: Option<DeviceId>,
    pub address: SocketAddr,
    pub connection: QuicConnection,
}

impl QuicTransport {
    pub fn new(local_device_id: DeviceId) -> Self {
        let identity = Encryption::load_or_generate_default_identity().unwrap_or_else(|error| {
            tracing::warn!(
                "Failed to load persistent QUIC identity, using ephemeral certificate: {}",
                error
            );
            let (cert_der, key_der) =
                Encryption::generate_cert().expect("ephemeral QUIC certificate generation failed");
            QuicIdentity { cert_der, key_der }
        });

        Self::from_identity(local_device_id, identity)
    }

    fn from_identity(local_device_id: DeviceId, identity: QuicIdentity) -> Self {
        let (incoming_tx, incoming_rx) = mpsc::channel(10);
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
            state_lifetime: None,
            #[cfg(test)]
            accept_task_barrier: None,
            #[cfg(test)]
            accept_task_waiting: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_identity(local_device_id: DeviceId, identity: QuicIdentity) -> Self {
        Self::from_identity(local_device_id, identity)
    }

    #[cfg(test)]
    pub(crate) fn isolated_for_test(local_device_id: DeviceId) -> Self {
        let (cert_der, key_der) =
            Encryption::generate_cert().expect("test QUIC certificate generation failed");
        Self::isolated_with_identity_for_test(local_device_id, QuicIdentity { cert_der, key_der })
    }

    #[cfg(test)]
    pub(crate) fn isolated_with_identity_for_test(
        local_device_id: DeviceId,
        identity: QuicIdentity,
    ) -> Self {
        let state_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("rshare-state")
            .join(uuid::Uuid::new_v4().to_string());
        let lifetime = Arc::new(StateLifetimeOwner::isolated(state_dir.clone()));
        let mut transport = Self::from_identity(local_device_id, identity)
            .with_trust_store_path(state_dir.join("quic-trust.json"));
        transport.state_lifetime = Some(lifetime);
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

    #[cfg(test)]
    fn with_test_accept_task_barrier(mut self, barrier: Arc<Notify>) -> Self {
        self.accept_task_barrier = Some(barrier);
        self
    }

    #[cfg(test)]
    fn accept_task_waiting_for_test(&self) -> bool {
        self.accept_task_waiting.load(Ordering::Acquire)
    }

    pub async fn start_server(&mut self, bind_addr: &str) -> Result<()> {
        if self.server_endpoint.is_some() || self.server_task.is_some() {
            return Err(QuicServerStartError::AlreadyRunning.into());
        }

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
        let state_lifetime = self.state_lifetime.clone();
        #[cfg(test)]
        let accept_task_barrier = self.accept_task_barrier.clone();
        #[cfg(test)]
        let accept_task_waiting = self.accept_task_waiting.clone();
        let accept_endpoint = endpoint.clone();

        let server_task = tokio::spawn(async move {
            let _state_lifetime = state_lifetime.clone();
            while let Some(connecting) = accept_endpoint.accept().await {
                let incoming_tx = incoming_tx.clone();
                let endpoint = accept_endpoint.clone();
                let config = config.clone();
                let trust_store_path = trust_store_path.clone();
                let datagram_reader_start_barrier = datagram_reader_start_barrier.clone();
                let state_lifetime = state_lifetime.clone();
                #[cfg(test)]
                let accept_task_barrier = accept_task_barrier.clone();
                #[cfg(test)]
                let accept_task_waiting = accept_task_waiting.clone();

                tokio::spawn(async move {
                    let _task_state_lifetime = state_lifetime.clone();
                    #[cfg(test)]
                    if let Some(barrier) = accept_task_barrier {
                        accept_task_waiting.store(true, Ordering::Release);
                        barrier.notified().await;
                        accept_task_waiting.store(false, Ordering::Release);
                    }
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
                                state_lifetime,
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
        let pending_peer_trust = inspect_outbound_peer_trust(
            &connection,
            device_id,
            trust_store_path.clone(),
            self.state_lifetime.clone(),
        )
        .await?;

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
            self.state_lifetime.clone(),
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

impl Drop for QuicTransport {
    fn drop(&mut self) {
        if let Some(task) = self.server_task.take() {
            task.abort();
        }
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
    _state_lifetime: Option<Arc<StateLifetimeOwner>>,
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
    qos_required: bool,
    datagram_reader_start_barrier: Option<Arc<Notify>>,
    qos_receive: StdMutex<Option<QosReceiveContext>>,
    qos_receivers: StdMutex<Option<InstalledInboundReceivers>>,
    qos_receive_notify: Notify,
    qos_inbound: StdMutex<QosInboundState>,
    authenticated_uni_stream_tasks: Arc<Semaphore>,
    #[cfg(test)]
    qos_reliable_reader_start_barrier: StdMutex<Option<Arc<Notify>>>,
    #[cfg(test)]
    qos_bulk_reader_start_barrier: StdMutex<Option<Arc<Notify>>>,
    #[cfg(test)]
    qos_bulk_reader_barrier_waiting: AtomicBool,
    #[cfg(test)]
    qos_reliable_faults_handled: AtomicU64,
    #[cfg(test)]
    qos_awaited_cancel_resets_observed: AtomicU64,
    #[cfg(test)]
    qos_terminal_cancel_resets_observed: AtomicU64,
    #[cfg(test)]
    authenticated_uni_streams_accepted: AtomicUsize,
    #[cfg(test)]
    authenticated_uni_stream_tasks_active: AtomicUsize,
    #[cfg(test)]
    authenticated_uni_stream_tasks_peak: AtomicUsize,
    #[cfg(test)]
    authenticated_uni_streams_rejected: AtomicUsize,
    #[cfg(test)]
    authenticated_uni_stream_tasks_completed: AtomicUsize,
}

#[cfg(test)]
struct AuthenticatedUniStreamTaskProbe {
    inner: Arc<QuicConnectionInner>,
}

#[cfg(test)]
impl AuthenticatedUniStreamTaskProbe {
    fn start(inner: Arc<QuicConnectionInner>) -> Self {
        let active = inner
            .authenticated_uni_stream_tasks_active
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        inner
            .authenticated_uni_stream_tasks_peak
            .fetch_max(active, Ordering::AcqRel);
        Self { inner }
    }
}

#[cfg(test)]
impl Drop for AuthenticatedUniStreamTaskProbe {
    fn drop(&mut self) {
        self.inner
            .authenticated_uni_stream_tasks_active
            .fetch_sub(1, Ordering::AcqRel);
        self.inner
            .authenticated_uni_stream_tasks_completed
            .fetch_add(1, Ordering::AcqRel);
    }
}

#[derive(Clone)]
struct QosReceiveContext {
    auth: Arc<PeerAuthContext>,
    release_emitter: TerminalReleaseEmitter,
    realtime: LatestRealtimeEmitter,
    reliable_input_tx: mpsc::Sender<ReliableInputFrame>,
    control_tx: mpsc::Sender<ControlFrame>,
    control_event_tx: mpsc::Sender<ControlFrame>,
    telemetry_tx: mpsc::Sender<TelemetryFrame>,
    bulk_tx: mpsc::Sender<BulkFrame>,
    protocol_error_tx: mpsc::Sender<TransportProtocolError>,
}

#[derive(Default)]
struct QosInboundState {
    active: Option<(ControlConnectionId, SessionEpoch, u64)>,
    retired_through: Option<(ControlConnectionId, SessionEpoch)>,
    realtime_last: Option<((ControlConnectionId, SessionEpoch), u64)>,
}

enum ReliableAccept {
    Accepted,
    EpochAdvanced { retired: SessionEpoch },
    RetiredEpoch,
    CurrentViolation { epoch: SessionEpoch },
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
    _state_lifetime: Option<Arc<StateLifetimeOwner>>,
}

pub(crate) struct OutboundFrame {
    message: Message,
    ack: oneshot::Sender<std::result::Result<(), String>>,
}

#[cfg(test)]
impl OutboundFrame {
    pub(crate) fn complete_for_test(self, result: std::result::Result<(), String>) {
        let _ = self.ack.send(result);
    }
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
        state_lifetime: Option<Arc<StateLifetimeOwner>>,
    ) -> Self {
        let (send_channel, mut send_rx): (mpsc::Sender<OutboundFrame>, _) = mpsc::channel(128);
        let (message_tx, message_rx): (mpsc::Sender<Message>, _) = mpsc::channel(256);
        let peer_fingerprint = peer_certificate_fingerprint(&connection);
        let inner = Arc::new(QuicConnectionInner {
            _endpoint: endpoint,
            connection,
            _state_lifetime: state_lifetime.clone(),
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
            qos_required: peer_protocol_handshake_required,
            datagram_reader_start_barrier,
            qos_receive: StdMutex::new(None),
            qos_receivers: StdMutex::new(None),
            qos_receive_notify: Notify::new(),
            qos_inbound: StdMutex::new(QosInboundState::default()),
            authenticated_uni_stream_tasks: Arc::new(Semaphore::new(
                AUTHENTICATED_UNI_STREAM_TASK_BUDGET,
            )),
            #[cfg(test)]
            qos_reliable_reader_start_barrier: StdMutex::new(None),
            #[cfg(test)]
            qos_bulk_reader_start_barrier: StdMutex::new(None),
            #[cfg(test)]
            qos_bulk_reader_barrier_waiting: AtomicBool::new(false),
            #[cfg(test)]
            qos_reliable_faults_handled: AtomicU64::new(0),
            #[cfg(test)]
            qos_awaited_cancel_resets_observed: AtomicU64::new(0),
            #[cfg(test)]
            qos_terminal_cancel_resets_observed: AtomicU64::new(0),
            #[cfg(test)]
            authenticated_uni_streams_accepted: AtomicUsize::new(0),
            #[cfg(test)]
            authenticated_uni_stream_tasks_active: AtomicUsize::new(0),
            #[cfg(test)]
            authenticated_uni_stream_tasks_peak: AtomicUsize::new(0),
            #[cfg(test)]
            authenticated_uni_streams_rejected: AtomicUsize::new(0),
            #[cfg(test)]
            authenticated_uni_stream_tasks_completed: AtomicUsize::new(0),
        });
        {
            let inner = inner.clone();
            let writer_config = config.clone();
            tokio::spawn(async move {
                while let Some(frame) = send_rx.recv().await {
                    let result = send_outbound_message(&inner, &writer_config, &frame.message)
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
                read_realtime_datagrams(inner).await;
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

    fn control_connection_id(&self) -> Option<ControlConnectionId> {
        self.inner
            .qos_receive
            .lock()
            .expect("qos receive context poisoned")
            .as_ref()
            .map(|context| context.auth.control_connection_id)
    }

    /// Installs the authenticated QoS identity and returns the cloneable lane
    /// handle plus the typed terminal-release receiver for this connection.
    pub fn install_qos(
        &self,
        auth: Arc<PeerAuthContext>,
    ) -> (PeerTransportHandle, mpsc::Receiver<TerminalReleaseEvent>) {
        let (release_tx, release_rx) = mpsc::channel(8);
        let release_emitter = TerminalReleaseEmitter::new(release_tx);
        let (realtime_tx, realtime_rx) = mpsc::channel(1);
        let realtime = LatestRealtimeEmitter::new(realtime_tx);
        let (reliable_input_tx, reliable_input_rx) = mpsc::channel(256);
        let (control_tx, control_rx) = mpsc::channel(64);
        let (control_event_tx, control_events) = mpsc::channel(64);
        let (telemetry_tx, telemetry_rx) = mpsc::channel(32);
        let (bulk_tx, bulk_rx) = mpsc::channel(8);
        let (protocol_error_tx, protocol_errors) = mpsc::channel(32);
        *self
            .inner
            .qos_receivers
            .lock()
            .expect("qos receiver set poisoned") = Some(InstalledInboundReceivers {
            peer: Some(PeerInbound {
                auth: auth.clone(),
                realtime_rx,
                reliable_input_rx,
                control_rx,
                telemetry_rx,
                bulk_rx,
            }),
            control_events: Some(control_events),
            protocol_errors: Some(protocol_errors),
        });
        *self
            .inner
            .qos_receive
            .lock()
            .expect("qos receive context poisoned") = Some(QosReceiveContext {
            auth: auth.clone(),
            release_emitter: release_emitter.clone(),
            realtime,
            reliable_input_tx,
            control_tx,
            control_event_tx,
            telemetry_tx,
            bulk_tx,
            protocol_error_tx,
        });
        self.inner.qos_receive_notify.notify_waiters();
        let monitor_inner = self.inner.clone();
        let monitor_auth = auth.clone();
        let monitor_release_emitter = release_emitter.clone();
        tokio::spawn(async move {
            let _ = monitor_inner.connection.closed().await;
            let active_epoch = {
                let mut state = monitor_inner
                    .qos_inbound
                    .lock()
                    .expect("qos inbound state poisoned");
                match state.active {
                    Some((generation, epoch, _))
                        if generation == monitor_auth.control_connection_id =>
                    {
                        retire_inbound_epoch(&mut state, (generation, epoch));
                        state.active = None;
                        Some(epoch)
                    }
                    _ => None,
                }
            };
            if let Some(epoch) = active_epoch {
                monitor_release_emitter.emit(TerminalReleaseEvent {
                    auth: monitor_auth,
                    epoch,
                    reason: rshare_core::ReleaseAllReason::Timeout,
                });
            }
        });
        (
            PeerTransportHandle::from_quinn(auth, self.inner.connection.clone(), release_emitter),
            release_rx,
        )
    }

    /// Takes the receiver set for this authenticated connection generation.
    pub fn take_peer_inbound(&self) -> Option<PeerInbound> {
        self.inner
            .qos_receivers
            .lock()
            .expect("qos receiver set poisoned")
            .as_mut()
            .and_then(|receivers| receivers.peer.take())
    }

    /// Takes the single-consumer authenticated control-event compatibility
    /// mirror. Production normally leaves ownership to `ConnectionManager`;
    /// direct typed-lane integrations such as the perf harness must drain this
    /// mirror alongside `PeerInbound::control_rx` to avoid artificial
    /// backpressure.
    pub fn take_control_events(&self) -> Option<mpsc::Receiver<ControlFrame>> {
        self.inner
            .qos_receivers
            .lock()
            .expect("qos receiver set poisoned")
            .as_mut()
            .and_then(|receivers| receivers.control_events.take())
    }

    pub(crate) fn take_protocol_errors(&self) -> Option<mpsc::Receiver<TransportProtocolError>> {
        self.inner
            .qos_receivers
            .lock()
            .expect("qos receiver set poisoned")
            .as_mut()
            .and_then(|receivers| receivers.protocol_errors.take())
    }

    #[cfg(test)]
    fn set_qos_reliable_reader_barrier(&self, barrier: Arc<Notify>) {
        *self
            .inner
            .qos_reliable_reader_start_barrier
            .lock()
            .expect("qos reliable reader barrier poisoned") = Some(barrier);
    }

    #[cfg(test)]
    pub(crate) fn set_qos_bulk_reader_barrier(&self, barrier: Arc<Notify>) {
        *self
            .inner
            .qos_bulk_reader_start_barrier
            .lock()
            .expect("qos bulk reader barrier poisoned") = Some(barrier);
    }

    #[cfg(test)]
    pub(crate) fn qos_bulk_reader_waiting_for_test(&self) -> bool {
        self.inner
            .qos_bulk_reader_barrier_waiting
            .load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn authenticated_uni_stream_task_counts_for_test(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.inner
                .authenticated_uni_streams_accepted
                .load(Ordering::Acquire),
            self.inner
                .authenticated_uni_stream_tasks_active
                .load(Ordering::Acquire),
            self.inner
                .authenticated_uni_stream_tasks_peak
                .load(Ordering::Acquire),
            self.inner
                .authenticated_uni_streams_rejected
                .load(Ordering::Acquire),
            self.inner
                .authenticated_uni_stream_tasks_completed
                .load(Ordering::Acquire),
        )
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
    connections: Arc<StdMutex<std::collections::HashMap<DeviceId, PooledConnection>>>,
}

struct PooledConnection {
    generation: u64,
    control_connection_id: Option<ControlConnectionId>,
    outbound: OutboundSender,
    connection: QuicConnection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PooledSendIdentity {
    pub(crate) lifecycle_generation: u64,
    pub(crate) control_connection_id: Option<ControlConnectionId>,
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
            connections: Arc::new(StdMutex::new(std::collections::HashMap::new())),
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
        self.insert_with_generation_now(device_id, generation, conn);
    }

    pub(crate) fn insert_with_generation_now(
        &self,
        device_id: DeviceId,
        generation: u64,
        conn: QuicConnection,
    ) {
        let control_connection_id = conn.control_connection_id();
        let mut conns = self.connections.lock().expect("connection pool poisoned");
        conns.insert(
            device_id,
            PooledConnection {
                generation,
                control_connection_id,
                outbound: conn.outbound_sender(),
                connection: conn,
            },
        );
    }

    pub fn get(&self, _device_id: &DeviceId) -> Option<&'static QuicConnection> {
        None
    }

    pub async fn send_to(&self, device_id: &DeviceId, message: &Message) -> Result<()> {
        self.send_to_with_identity(device_id, message)
            .await
            .map(|_| ())
    }

    pub(crate) async fn send_to_with_identity(
        &self,
        device_id: &DeviceId,
        message: &Message,
    ) -> Result<PooledSendIdentity> {
        let (outbound, identity) = self
            .connections
            .lock()
            .expect("connection pool poisoned")
            .get(device_id)
            .map(|entry| {
                (
                    entry.outbound.clone(),
                    PooledSendIdentity {
                        lifecycle_generation: entry.generation,
                        control_connection_id: entry.control_connection_id,
                    },
                )
            })
            .ok_or_else(|| anyhow!("No active connection for device {}", device_id))?;
        outbound.send_message(message).await?;
        Ok(identity)
    }

    #[cfg(test)]
    pub(crate) async fn replace_outbound_for_test(
        &self,
        device_id: DeviceId,
        control_connection_id: Option<ControlConnectionId>,
        send_channel: mpsc::Sender<OutboundFrame>,
    ) {
        self.connections
            .lock()
            .expect("connection pool poisoned")
            .get_mut(&device_id)
            .map(|entry| {
                entry.control_connection_id = control_connection_id;
                entry.outbound = OutboundSender { send_channel };
            })
            .expect("test peer must be present");
    }

    #[cfg(test)]
    pub(crate) async fn replace_generation_and_outbound_for_test(
        &self,
        device_id: DeviceId,
        lifecycle_generation: u64,
        control_connection_id: Option<ControlConnectionId>,
        send_channel: mpsc::Sender<OutboundFrame>,
    ) {
        self.connections
            .lock()
            .expect("connection pool poisoned")
            .get_mut(&device_id)
            .map(|entry| {
                entry.generation = lifecycle_generation;
                entry.control_connection_id = control_connection_id;
                entry.outbound = OutboundSender { send_channel };
            })
            .expect("test peer must be present");
    }

    pub async fn diagnostics_for(
        &self,
        device_id: &DeviceId,
    ) -> Option<TransportConnectionDiagnostics> {
        self.diagnostics_for_now(device_id)
    }

    pub(crate) fn diagnostics_for_now(
        &self,
        device_id: &DeviceId,
    ) -> Option<TransportConnectionDiagnostics> {
        let conns = self.connections.lock().expect("connection pool poisoned");
        conns
            .get(device_id)
            .filter(|entry| entry.connection.is_connected())
            .map(|entry| entry.connection.diagnostics())
    }

    pub async fn diagnostics_all(&self) -> Vec<(DeviceId, TransportConnectionDiagnostics)> {
        self.diagnostics_all_now()
    }

    pub(crate) fn diagnostics_all_now(&self) -> Vec<(DeviceId, TransportConnectionDiagnostics)> {
        let conns = self.connections.lock().expect("connection pool poisoned");
        conns
            .iter()
            .filter(|(_, entry)| entry.connection.is_connected())
            .map(|(device_id, entry)| (*device_id, entry.connection.diagnostics()))
            .collect()
    }

    pub async fn remove(&self, device_id: &DeviceId) -> Option<QuicConnection> {
        self.remove_now(device_id)
    }

    pub(crate) fn remove_now(&self, device_id: &DeviceId) -> Option<QuicConnection> {
        let mut conns = self.connections.lock().expect("connection pool poisoned");
        conns.remove(device_id).map(|entry| entry.connection)
    }

    pub(crate) fn remove_generation_now(
        &self,
        device_id: &DeviceId,
        generation: u64,
    ) -> Option<QuicConnection> {
        let mut conns = self.connections.lock().expect("connection pool poisoned");
        if conns
            .get(device_id)
            .is_some_and(|entry| entry.generation == generation)
        {
            conns.remove(device_id).map(|entry| entry.connection)
        } else {
            None
        }
    }

    #[cfg(test)]
    pub(crate) async fn generation_for(&self, device_id: &DeviceId) -> Option<u64> {
        self.connections
            .lock()
            .expect("connection pool poisoned")
            .get(device_id)
            .map(|entry| entry.generation)
    }

    pub fn count(&self) -> usize {
        let conns = self.connections.lock().expect("connection pool poisoned");
        conns.len()
    }

    pub async fn broadcast(&self, message: &Message) -> Result<()> {
        let peers: Vec<_> = self
            .connections
            .lock()
            .expect("connection pool poisoned")
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
            .expect("connection pool poisoned")
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
        let mut conns = self.connections.lock().expect("connection pool poisoned");
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
    let legacy_realtime_datagram = match encode_legacy_realtime_message(message) {
        Ok(datagram) => datagram,
        Err(error) => {
            inner.datagram_tx_dropped.fetch_add(1, Ordering::Relaxed);
            tracing::debug!("QUIC realtime datagram dropped during encoding: {}", error);
            return Ok(());
        }
    };
    if let Some(datagram) = legacy_realtime_datagram {
        if let Some(max_datagram_size) = inner.connection.max_datagram_size() {
            if datagram.len() <= max_datagram_size {
                match inner.connection.send_datagram(datagram) {
                    Ok(()) => return Ok(()),
                    Err(error) => {
                        inner.datagram_tx_dropped.fetch_add(1, Ordering::Relaxed);
                        tracing::debug!("QUIC realtime datagram dropped: {}", error);
                        return Ok(());
                    }
                }
            }
        }
        inner.datagram_tx_dropped.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(
            "QUIC realtime datagram dropped because datagrams are unavailable or too small"
        );
        return Ok(());
    }

    let encoded = ControlMessageCodec::encode(message)?;
    if encoded.len() > config.max_message_size {
        anyhow::bail!("Reliable message too large: {} bytes", encoded.len());
    }
    write_reliable_frame(inner, &encoded).await
}

/// Temporary adapter for the legacy `Message` transport.
///
/// The epoch-scoped realtime codec remains lossless and independent. Task 9
/// replaces this adapter when the transport accepts `RealtimeInputFrame`
/// directly.
pub(crate) fn encode_legacy_realtime_message(message: &Message) -> Result<Option<Bytes>> {
    match message {
        Message::MouseMove { .. } | Message::GamepadState { .. } => {
            Ok(Some(Bytes::from(ControlMessageCodec::encode(message)?)))
        }
        _ => Ok(None),
    }
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

        if !bootstrap_stream {
            #[cfg(test)]
            inner
                .authenticated_uni_streams_accepted
                .fetch_add(1, Ordering::AcqRel);
            let task_permit = match inner
                .authenticated_uni_stream_tasks
                .clone()
                .try_acquire_owned()
            {
                Ok(permit) => permit,
                Err(_) => {
                    let _ = stream.stop(VarInt::from_u32(0x525350));
                    #[cfg(test)]
                    inner
                        .authenticated_uni_streams_rejected
                        .fetch_add(1, Ordering::AcqRel);
                    try_report_current_protocol_error(
                        &inner,
                        "authenticated uni stream task budget exhausted".into(),
                    );
                    if let Some(context) = inner
                        .qos_receive
                        .lock()
                        .expect("qos receive context poisoned")
                        .clone()
                    {
                        fail_close_current_qos_generation_with_code(
                            &inner,
                            &context,
                            VarInt::from_u32(AUTHENTICATED_UNI_STREAM_BUDGET_EXHAUSTED_CODE),
                            b"authenticated uni stream task budget exhausted",
                        );
                    } else {
                        inner.connection.close(
                            VarInt::from_u32(AUTHENTICATED_UNI_STREAM_BUDGET_EXHAUSTED_CODE),
                            b"authenticated uni stream task budget exhausted",
                        );
                    }
                    return;
                }
            };
            let inner = inner.clone();
            let message_tx = message_tx.clone();
            tokio::spawn(async move {
                let _task_permit = task_permit;
                #[cfg(test)]
                let _probe = AuthenticatedUniStreamTaskProbe::start(inner.clone());
                read_authenticated_uni_stream(inner, stream, message_tx, max_message_size).await;
            });
            continue;
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

async fn read_authenticated_uni_stream(
    inner: Arc<QuicConnectionInner>,
    mut stream: quinn::RecvStream,
    message_tx: mpsc::Sender<Message>,
    max_message_size: usize,
) {
    let mut prefix = [0u8; 4];
    if let Err(error) = stream.read_exact(&mut prefix).await {
        if is_terminal_cancel_reset(&error) {
            #[cfg(test)]
            inner
                .qos_terminal_cancel_resets_observed
                .fetch_add(1, Ordering::AcqRel);
            return;
        }
        if is_stream_reset(&error, AWAITED_CANCEL_RESET_CODE) {
            #[cfg(test)]
            inner
                .qos_awaited_cancel_resets_observed
                .fetch_add(1, Ordering::AcqRel);
            return;
        }
        tracing::debug!("QUIC authenticated stream prefix read stopped: {}", error);
        if matches!(error, quinn::ReadExactError::FinishedEarly(_)) {
            reject_unrecognized_qos_stream(
                &inner,
                &mut stream,
                "truncated qos lane preface".into(),
            )
            .await;
        } else if let Some(context) = await_qos_receive_context(&inner).await {
            fail_close_current_qos_generation(
                &inner,
                &context,
                b"authenticated qos preface read failure",
            );
        } else {
            inner
                .connection
                .close(0u32.into(), b"authenticated qos preface read failure");
        }
        return;
    }
    if &prefix == QOS_LANE_MAGIC {
        let mut lane = [0u8; 1];
        if let Err(error) = stream.read_exact(&mut lane).await {
            if is_terminal_cancel_reset(&error) {
                #[cfg(test)]
                inner
                    .qos_terminal_cancel_resets_observed
                    .fetch_add(1, Ordering::AcqRel);
                return;
            }
            if is_stream_reset(&error, AWAITED_CANCEL_RESET_CODE) {
                #[cfg(test)]
                inner
                    .qos_awaited_cancel_resets_observed
                    .fetch_add(1, Ordering::AcqRel);
                return;
            }
            if matches!(error, quinn::ReadExactError::FinishedEarly(_)) {
                reject_unrecognized_qos_stream(
                    &inner,
                    &mut stream,
                    "truncated qos lane discriminator".into(),
                )
                .await;
            } else if let Some(context) = await_qos_receive_context(&inner).await {
                fail_close_current_qos_generation(
                    &inner,
                    &context,
                    b"authenticated qos discriminator read failure",
                );
            } else {
                inner
                    .connection
                    .close(0u32.into(), b"authenticated qos discriminator read failure");
            }
            return;
        }
        match lane[0] {
            value if value == LaneDiscriminator::ReliableInput as u8 => {
                read_qos_reliable_stream(inner, stream, false).await;
            }
            value if value == LaneDiscriminator::Emergency as u8 => {
                read_qos_reliable_stream(inner, stream, true).await;
            }
            value
                if value == LaneDiscriminator::Control as u8
                    || value == LaneDiscriminator::Bulk as u8
                    || value == LaneDiscriminator::Telemetry as u8
                    || value == LaneDiscriminator::ReliableCompat as u8 =>
            {
                read_qos_message_stream(inner, stream, max_message_size, value).await;
            }
            _ => {
                reject_unrecognized_qos_stream(
                    &inner,
                    &mut stream,
                    format!("unknown qos lane discriminator {}", lane[0]),
                )
                .await;
            }
        }
        return;
    }
    let authenticated_qos_installed = inner
        .qos_receive
        .lock()
        .expect("qos receive context poisoned")
        .is_some();
    if inner.qos_required || authenticated_qos_installed {
        reject_unrecognized_qos_stream(
            &inner,
            &mut stream,
            "missing qos lane preface on authenticated stream".into(),
        )
        .await;
        return;
    }
    read_legacy_framed_stream(inner, stream, message_tx, max_message_size, Some(prefix)).await;
}

async fn reject_unrecognized_qos_stream(
    inner: &QuicConnectionInner,
    stream: &mut quinn::RecvStream,
    error: String,
) {
    let _ = stream.stop(VarInt::from_u32(0x525350));
    if let Some(context) = await_qos_receive_context(inner).await {
        try_report_protocol_error(&context, error);
    }
}

fn try_report_current_protocol_error(inner: &QuicConnectionInner, error: String) {
    let context = inner
        .qos_receive
        .lock()
        .expect("qos receive context poisoned")
        .clone();
    if let Some(context) = context {
        try_report_protocol_error(&context, error);
    }
}

fn try_report_protocol_error(context: &QosReceiveContext, error: String) {
    let _ = context.protocol_error_tx.try_send(TransportProtocolError {
        auth: context.auth.clone(),
        error,
    });
}

async fn read_qos_message_stream(
    inner: Arc<QuicConnectionInner>,
    mut stream: quinn::RecvStream,
    max_message_size: usize,
    lane: u8,
) {
    let Some(context) = await_qos_receive_context(&inner).await else {
        return;
    };
    #[cfg(test)]
    if lane == LaneDiscriminator::Bulk as u8 {
        let barrier = inner
            .qos_bulk_reader_start_barrier
            .lock()
            .expect("qos bulk reader barrier poisoned")
            .clone();
        if let Some(barrier) = barrier {
            inner
                .qos_bulk_reader_barrier_waiting
                .store(true, Ordering::Release);
            barrier.notified().await;
            inner
                .qos_bulk_reader_barrier_waiting
                .store(false, Ordering::Release);
        }
    }
    loop {
        let mut len_buf = [0u8; 4];
        if let Err(error) = stream.read_exact(&mut len_buf).await {
            if is_stream_reset(&error, AWAITED_CANCEL_RESET_CODE) {
                #[cfg(test)]
                inner
                    .qos_awaited_cancel_resets_observed
                    .fetch_add(1, Ordering::AcqRel);
                return;
            }
            inner
                .connection
                .close(0u32.into(), b"truncated qos message length");
            return;
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > max_message_size {
            inner
                .connection
                .close(0u32.into(), b"oversized qos message frame");
            return;
        }
        let mut data = vec![0u8; len];
        if let Err(error) = stream.read_exact(&mut data).await {
            if is_stream_reset(&error, AWAITED_CANCEL_RESET_CODE) {
                #[cfg(test)]
                inner
                    .qos_awaited_cancel_resets_observed
                    .fetch_add(1, Ordering::AcqRel);
                return;
            }
            inner
                .connection
                .close(0u32.into(), b"truncated qos message payload");
            return;
        }
        let Ok(message) = ControlMessageCodec::decode(&data) else {
            inner
                .connection
                .close(0u32.into(), b"invalid qos message frame");
            return;
        };
        let Ok(classified) = super::qos::ClassifiedMessage::try_from(message.clone()) else {
            inner
                .connection
                .close(0u32.into(), b"unclassifiable qos message");
            return;
        };
        let lane_matches = matches!(
            (&classified, lane),
            (
                super::qos::ClassifiedMessage::Control(_),
                value
            ) if value == LaneDiscriminator::Control as u8
        ) || matches!(
            (&classified, lane),
            (
                super::qos::ClassifiedMessage::Bulk(_),
                value
            ) if value == LaneDiscriminator::Bulk as u8
        ) || matches!(
            (&classified, lane),
            (
                super::qos::ClassifiedMessage::Telemetry(_),
                value
            ) if value == LaneDiscriminator::Telemetry as u8
        ) || matches!(
            (&classified, lane),
            (
                super::qos::ClassifiedMessage::ReliableCompat(_),
                value
            ) if value == LaneDiscriminator::ReliableCompat as u8
        );
        if !lane_matches {
            let _ = stream.stop(VarInt::from_u32(0x525350));
            try_report_protocol_error(&context, "message entered wrong qos lane".into());
            return;
        }
        match classified {
            super::qos::ClassifiedMessage::Control(frame) => {
                if context.control_tx.send(frame.clone()).await.is_err() {
                    return;
                }
                if context.control_event_tx.send(frame).await.is_err() {
                    return;
                }
            }
            super::qos::ClassifiedMessage::Bulk(frame) => {
                if context.bulk_tx.send(frame).await.is_err() {
                    return;
                }
            }
            super::qos::ClassifiedMessage::Telemetry(frame) => {
                let _ = context.telemetry_tx.try_send(frame);
            }
            super::qos::ClassifiedMessage::ReliableCompat(_) => {
                let _ = stream.stop(VarInt::from_u32(0x525350));
                try_report_protocol_error(
                    &context,
                    "reliable compatibility input lacks authenticated epoch metadata".into(),
                );
                inner
                    .connection
                    .close(0u32.into(), b"rejected reliable compatibility input");
                return;
            }
            super::qos::ClassifiedMessage::Unsupported => unreachable!(),
        }
    }
}

async fn read_legacy_framed_stream(
    inner: Arc<QuicConnectionInner>,
    mut stream: quinn::RecvStream,
    message_tx: mpsc::Sender<Message>,
    max_message_size: usize,
    mut first_len: Option<[u8; 4]>,
) {
    loop {
        let len_buf = if let Some(prefix) = first_len.take() {
            prefix
        } else {
            let mut bytes = [0u8; 4];
            if stream.read_exact(&mut bytes).await.is_err() {
                return;
            }
            bytes
        };
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > max_message_size {
            inner
                .connection
                .close(0u32.into(), b"reliable frame exceeds active protocol limit");
            return;
        }
        let mut data = vec![0u8; len];
        if stream.read_exact(&mut data).await.is_err() {
            return;
        }
        match ControlMessageCodec::decode(&data) {
            Ok(message) => {
                if message_tx.send(message).await.is_err() {
                    return;
                }
            }
            Err(error) => tracing::debug!("Failed to decode reliable message: {}", error),
        }
    }
}

async fn read_qos_reliable_stream(
    inner: Arc<QuicConnectionInner>,
    mut stream: quinn::RecvStream,
    emergency: bool,
) {
    let mut stream_epoch: Option<SessionEpoch> = None;
    loop {
        #[cfg(test)]
        if !emergency {
            let barrier = inner
                .qos_reliable_reader_start_barrier
                .lock()
                .expect("qos reliable reader barrier poisoned")
                .take();
            if let Some(barrier) = barrier {
                barrier.notified().await;
            }
        }
        let mut len_buf = [0u8; 4];
        if let Err(error) = stream.read_exact(&mut len_buf).await {
            if let Some(context) = await_qos_receive_context(&inner).await {
                if !(is_terminal_cancel_reset(&error)
                    && handle_terminal_cancel_reset(&inner, &context, stream_epoch))
                {
                    fail_close_active_qos(
                        &inner,
                        &context,
                        stream_epoch,
                        b"truncated qos reliable length",
                    );
                }
            } else {
                inner
                    .connection
                    .close(0u32.into(), b"truncated qos reliable length");
            }
            #[cfg(test)]
            inner
                .qos_reliable_faults_handled
                .fetch_add(1, Ordering::AcqRel);
            return;
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > crate::codec::MAX_RELIABLE_INPUT_FRAME + 2 {
            if let Some(context) = await_qos_receive_context(&inner).await {
                fail_close_active_qos(&inner, &context, stream_epoch, b"oversized qos input frame");
            } else {
                inner
                    .connection
                    .close(0u32.into(), b"oversized qos input frame");
            }
            return;
        }
        let mut data = vec![0u8; len];
        if let Err(error) = stream.read_exact(&mut data).await {
            if let Some(context) = await_qos_receive_context(&inner).await {
                if !(is_terminal_cancel_reset(&error)
                    && handle_terminal_cancel_reset(&inner, &context, stream_epoch))
                {
                    fail_close_active_qos(
                        &inner,
                        &context,
                        stream_epoch,
                        b"truncated qos reliable payload",
                    );
                }
            } else {
                inner
                    .connection
                    .close(0u32.into(), b"truncated qos reliable payload");
            }
            #[cfg(test)]
            inner
                .qos_reliable_faults_handled
                .fetch_add(1, Ordering::AcqRel);
            return;
        }
        let Ok(frame) = crate::codec::ReliableInputCodec::decode(&data) else {
            if let Some(context) = await_qos_receive_context(&inner).await {
                fail_close_active_qos(&inner, &context, stream_epoch, b"invalid qos input frame");
            } else {
                inner
                    .connection
                    .close(0u32.into(), b"invalid qos input frame");
            }
            return;
        };
        let Some(context) = await_qos_receive_context(&inner).await else {
            return;
        };
        let key = (context.auth.control_connection_id, frame.session_epoch);
        if emergency {
            let ReliableInputEvent::ReleaseAll { reason } = &frame.event else {
                inner
                    .connection
                    .close(0u32.into(), b"non-terminal emergency input frame");
                return;
            };
            let reason = *reason;
            enum EmergencyDisposition {
                Ignore,
                Release(SessionEpoch, rshare_core::ReleaseAllReason),
                Violation(SessionEpoch),
            }
            let disposition = {
                let mut state = inner
                    .qos_inbound
                    .lock()
                    .expect("qos inbound state poisoned");
                if inbound_epoch_retired(&state, key) {
                    if state.active.is_some_and(|(generation, active_epoch, _)| {
                        generation == key.0 && active_epoch.0 > key.1 .0
                    }) {
                        EmergencyDisposition::Ignore
                    } else {
                        EmergencyDisposition::Violation(frame.session_epoch)
                    }
                } else {
                    match state.active {
                        Some((generation, epoch, _)) if (generation, epoch) == key => {
                            retire_inbound_epoch(&mut state, key);
                            state.active = None;
                            EmergencyDisposition::Release(epoch, reason)
                        }
                        Some((generation, active_epoch, _)) => {
                            retire_inbound_epoch(&mut state, (generation, active_epoch));
                            state.active = None;
                            EmergencyDisposition::Violation(active_epoch)
                        }
                        None => {
                            retire_inbound_epoch(&mut state, key);
                            EmergencyDisposition::Release(frame.session_epoch, reason)
                        }
                    }
                }
            };
            match disposition {
                EmergencyDisposition::Ignore => {}
                EmergencyDisposition::Release(epoch, reason) => {
                    if context.reliable_input_tx.try_send(frame).is_err() {
                        fail_close_reliable_delivery(
                            &inner,
                            &context,
                            epoch,
                            b"qos reliable inbound receiver unavailable",
                        );
                        return;
                    }
                    context.release_emitter.emit(TerminalReleaseEvent {
                        auth: context.auth,
                        epoch,
                        reason,
                    })
                }
                EmergencyDisposition::Violation(epoch) => {
                    inner
                        .connection
                        .close(0u32.into(), b"future emergency epoch mismatch");
                    context.release_emitter.emit(TerminalReleaseEvent {
                        auth: context.auth,
                        epoch,
                        reason: rshare_core::ReleaseAllReason::BackendFailure,
                    });
                }
            }
            return;
        }

        if !matches!(frame.event, ReliableInputEvent::Enter { .. })
            && stream_epoch != Some(frame.session_epoch)
        {
            fail_close_current_qos_generation(
                &inner,
                &context,
                b"qos reliable stream epoch mismatch",
            );
            return;
        }
        match accept_reliable_input(&inner, &context, &frame) {
            ReliableAccept::RetiredEpoch => continue,
            ReliableAccept::EpochAdvanced { retired } => {
                context.release_emitter.emit(TerminalReleaseEvent {
                    auth: context.auth.clone(),
                    epoch: retired,
                    reason: rshare_core::ReleaseAllReason::BackendFailure,
                });
            }
            ReliableAccept::CurrentViolation { epoch } => {
                inner
                    .connection
                    .close(0u32.into(), b"qos reliable sequence violation");
                context.release_emitter.emit(TerminalReleaseEvent {
                    auth: context.auth.clone(),
                    epoch,
                    reason: rshare_core::ReleaseAllReason::BackendFailure,
                });
                return;
            }
            ReliableAccept::Accepted => {}
        }
        if context.reliable_input_tx.try_send(frame.clone()).is_err() {
            fail_close_reliable_delivery(
                &inner,
                &context,
                frame.session_epoch,
                b"qos reliable inbound receiver unavailable",
            );
            return;
        }
        if matches!(frame.event, ReliableInputEvent::Enter { .. }) {
            stream_epoch = Some(frame.session_epoch);
        }
        if let ReliableInputEvent::ReleaseAll { reason } = frame.event {
            context.release_emitter.emit(TerminalReleaseEvent {
                auth: context.auth.clone(),
                epoch: frame.session_epoch,
                reason,
            });
            return;
        }
    }
}

fn fail_close_reliable_delivery(
    inner: &QuicConnectionInner,
    context: &QosReceiveContext,
    epoch: SessionEpoch,
    close_reason: &'static [u8],
) {
    {
        let mut state = inner
            .qos_inbound
            .lock()
            .expect("qos inbound state poisoned");
        retire_inbound_epoch(&mut state, (context.auth.control_connection_id, epoch));
        if state.active.is_some_and(|(generation, active_epoch, _)| {
            generation == context.auth.control_connection_id && active_epoch == epoch
        }) {
            state.active = None;
        }
    }
    inner.connection.close(0u32.into(), close_reason);
    context.release_emitter.emit(TerminalReleaseEvent {
        auth: context.auth.clone(),
        epoch,
        reason: rshare_core::ReleaseAllReason::BackendFailure,
    });
}

fn is_terminal_cancel_reset(error: &quinn::ReadExactError) -> bool {
    is_stream_reset(error, TERMINAL_CANCEL_RESET_CODE)
}

fn is_stream_reset(error: &quinn::ReadExactError, expected_code: u32) -> bool {
    matches!(
        error,
        quinn::ReadExactError::ReadError(quinn::ReadError::Reset(code))
            if *code == quinn::VarInt::from_u32(expected_code)
    )
}

fn handle_terminal_cancel_reset(
    inner: &QuicConnectionInner,
    context: &QosReceiveContext,
    stream_epoch: Option<SessionEpoch>,
) -> bool {
    let Some(stream_epoch) = stream_epoch else {
        return true;
    };
    let key = (context.auth.control_connection_id, stream_epoch);
    let release = {
        let mut state = inner
            .qos_inbound
            .lock()
            .expect("qos inbound state poisoned");
        if inbound_epoch_retired(&state, key) {
            return true;
        }
        match state.active {
            Some((generation, epoch, _)) if (generation, epoch) == key => {
                retire_inbound_epoch(&mut state, key);
                state.active = None;
                true
            }
            Some((generation, epoch, _)) if generation == key.0 && epoch.0 > stream_epoch.0 => {
                return true;
            }
            _ => return false,
        }
    };
    if release {
        context.release_emitter.emit(TerminalReleaseEvent {
            auth: context.auth.clone(),
            epoch: stream_epoch,
            reason: rshare_core::ReleaseAllReason::BackendFailure,
        });
    }
    true
}

fn fail_close_active_qos(
    inner: &QuicConnectionInner,
    context: &QosReceiveContext,
    stream_epoch: Option<SessionEpoch>,
    close_reason: &'static [u8],
) {
    let active_epoch = {
        let mut state = inner
            .qos_inbound
            .lock()
            .expect("qos inbound state poisoned");
        if let Some(stream_epoch) = stream_epoch {
            let stream_key = (context.auth.control_connection_id, stream_epoch);
            let matches_active = state
                .active
                .is_some_and(|(generation, epoch, _)| (generation, epoch) == stream_key);
            if !matches_active && inbound_epoch_retired(&state, stream_key) {
                return;
            }
        }
        match state.active {
            Some((generation, epoch, _))
                if generation == context.auth.control_connection_id
                    && stream_epoch == Some(epoch) =>
            {
                retire_inbound_epoch(&mut state, (generation, epoch));
                state.active = None;
                Some(epoch)
            }
            _ => None,
        }
    };
    inner.connection.close(0u32.into(), close_reason);
    if let Some(epoch) = active_epoch {
        context.release_emitter.emit(TerminalReleaseEvent {
            auth: context.auth.clone(),
            epoch,
            reason: rshare_core::ReleaseAllReason::BackendFailure,
        });
    }
}

fn fail_close_current_qos_generation(
    inner: &QuicConnectionInner,
    context: &QosReceiveContext,
    close_reason: &'static [u8],
) {
    fail_close_current_qos_generation_with_code(inner, context, VarInt::from_u32(0), close_reason);
}

fn fail_close_current_qos_generation_with_code(
    inner: &QuicConnectionInner,
    context: &QosReceiveContext,
    close_code: VarInt,
    close_reason: &'static [u8],
) {
    let active_epoch = {
        let mut state = inner
            .qos_inbound
            .lock()
            .expect("qos inbound state poisoned");
        match state.active {
            Some((generation, epoch, _)) if generation == context.auth.control_connection_id => {
                retire_inbound_epoch(&mut state, (generation, epoch));
                state.active = None;
                Some(epoch)
            }
            _ => None,
        }
    };
    inner.connection.close(close_code, close_reason);
    if let Some(epoch) = active_epoch {
        context.release_emitter.emit(TerminalReleaseEvent {
            auth: context.auth.clone(),
            epoch,
            reason: rshare_core::ReleaseAllReason::BackendFailure,
        });
    }
}

async fn await_qos_receive_context(inner: &QuicConnectionInner) -> Option<QosReceiveContext> {
    if let Some(context) = inner
        .qos_receive
        .lock()
        .expect("qos receive context poisoned")
        .clone()
    {
        return Some(context);
    }
    let notified = inner.qos_receive_notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    if let Some(context) = inner
        .qos_receive
        .lock()
        .expect("qos receive context poisoned")
        .clone()
    {
        return Some(context);
    }
    if tokio::time::timeout(super::handshake::BOOTSTRAP_TIMEOUT, notified)
        .await
        .is_err()
    {
        inner
            .connection
            .close(0u32.into(), b"qos stream before authenticated install");
        return None;
    }
    inner
        .qos_receive
        .lock()
        .expect("qos receive context poisoned")
        .clone()
}

fn accept_reliable_input(
    inner: &QuicConnectionInner,
    context: &QosReceiveContext,
    frame: &rshare_core::ReliableInputFrame,
) -> ReliableAccept {
    let connection_id = context.auth.control_connection_id;
    let mut state = inner
        .qos_inbound
        .lock()
        .expect("qos inbound state poisoned");
    accept_reliable_state(&mut state, connection_id, frame)
}

fn accept_reliable_state(
    state: &mut QosInboundState,
    connection_id: ControlConnectionId,
    frame: &rshare_core::ReliableInputFrame,
) -> ReliableAccept {
    let key = (connection_id, frame.session_epoch);
    if inbound_epoch_retired(state, key) {
        if state.active.is_some_and(|(generation, active_epoch, _)| {
            generation == connection_id && active_epoch.0 > frame.session_epoch.0
        }) {
            return ReliableAccept::RetiredEpoch;
        }
        return ReliableAccept::CurrentViolation {
            epoch: frame.session_epoch,
        };
    }
    match &frame.event {
        ReliableInputEvent::Enter { .. } => {
            if let Some((generation, active_epoch, _)) = state.active {
                if generation == connection_id && frame.session_epoch.0 > active_epoch.0 {
                    retire_inbound_epoch(state, (generation, active_epoch));
                    state.active = Some((connection_id, frame.session_epoch, frame.sequence));
                    return ReliableAccept::EpochAdvanced {
                        retired: active_epoch,
                    };
                }
            }
            if state.active.is_some()
                || state.retired_through.is_some_and(|(generation, epoch)| {
                    generation == connection_id && frame.session_epoch.0 <= epoch.0
                })
            {
                let epoch = state
                    .active
                    .map(|(_, epoch, _)| epoch)
                    .unwrap_or(frame.session_epoch);
                retire_inbound_epoch(state, (connection_id, epoch));
                state.active = None;
                return ReliableAccept::CurrentViolation { epoch };
            }
            state.active = Some((connection_id, frame.session_epoch, frame.sequence));
            ReliableAccept::Accepted
        }
        ReliableInputEvent::ReleaseAll { .. } => {
            let valid = state.active.is_some_and(|(generation, epoch, last)| {
                generation == connection_id
                    && epoch == frame.session_epoch
                    && last
                        .checked_add(1)
                        .is_some_and(|expected| frame.sequence == expected)
            });
            if !valid {
                let epoch = state
                    .active
                    .map(|(_, epoch, _)| epoch)
                    .unwrap_or(frame.session_epoch);
                retire_inbound_epoch(state, (connection_id, epoch));
                state.active = None;
                return ReliableAccept::CurrentViolation { epoch };
            }
            retire_inbound_epoch(state, key);
            state.active = None;
            ReliableAccept::Accepted
        }
        _ => {
            let Some((generation, epoch, last)) = state.active.as_mut() else {
                retire_inbound_epoch(state, key);
                return ReliableAccept::CurrentViolation {
                    epoch: frame.session_epoch,
                };
            };
            if *generation != connection_id
                || *epoch != frame.session_epoch
                || !last
                    .checked_add(1)
                    .is_some_and(|expected| frame.sequence == expected)
            {
                let active_epoch = *epoch;
                retire_inbound_epoch(state, (connection_id, active_epoch));
                state.active = None;
                return ReliableAccept::CurrentViolation {
                    epoch: active_epoch,
                };
            }
            *last = frame.sequence;
            ReliableAccept::Accepted
        }
    }
}

fn inbound_epoch_retired(
    state: &QosInboundState,
    key: (ControlConnectionId, SessionEpoch),
) -> bool {
    state
        .retired_through
        .is_some_and(|(generation, epoch)| generation == key.0 && key.1 .0 <= epoch.0)
}

fn retire_inbound_epoch(state: &mut QosInboundState, key: (ControlConnectionId, SessionEpoch)) {
    if state
        .retired_through
        .is_none_or(|(generation, epoch)| generation != key.0 || key.1 .0 > epoch.0)
    {
        state.retired_through = Some(key);
    }
    if state
        .realtime_last
        .is_some_and(|(realtime_key, _)| realtime_key.0 == key.0 && realtime_key.1 .0 <= key.1 .0)
    {
        state.realtime_last = None;
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

async fn read_realtime_datagrams(inner: Arc<QuicConnectionInner>) {
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
            Ok(datagram) => {
                if let Ok(frame) = crate::codec::RealtimeInputCodec::decode(&datagram) {
                    let Some(context) = await_qos_receive_context(&inner).await else {
                        return;
                    };
                    if accept_qos_realtime(&inner, &context, &frame) {
                        inner
                            .last_datagram_rx_us
                            .store(current_timestamp_us(), Ordering::Relaxed);
                        context.realtime.emit(frame);
                    }
                    continue;
                }
                tracing::debug!("Rejected non-v3 realtime datagram");
            }
            Err(error) => {
                tracing::debug!("QUIC datagram reader stopped: {}", error);
                break;
            }
        }
    }
}

fn accept_qos_realtime(
    inner: &QuicConnectionInner,
    context: &QosReceiveContext,
    frame: &rshare_core::RealtimeInputFrame,
) -> bool {
    let key = (context.auth.control_connection_id, frame.session_epoch);
    let mut state = inner
        .qos_inbound
        .lock()
        .expect("qos inbound state poisoned");
    if inbound_epoch_retired(&state, key)
        || !state
            .active
            .is_some_and(|(generation, epoch, _)| (generation, epoch) == key)
    {
        return false;
    }
    match state.realtime_last.as_mut() {
        Some((last_key, last)) if *last_key == key && frame.sequence <= *last => false,
        Some((last_key, last)) if *last_key == key => {
            *last = frame.sequence;
            true
        }
        _ => {
            state.realtime_last = Some((key, frame.sequence));
            true
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
    state_lifetime: Option<Arc<StateLifetimeOwner>>,
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
        _state_lifetime: state_lifetime,
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
    use rshare_core::{
        ClockDomainId, MonotonicStamp, RealtimeInputFrame, RealtimeInputPayload, ReleaseAllReason,
        ReliableInputFrame, INPUT_PROTOCOL_VERSION,
    };
    use rustls::pki_types::SubjectPublicKeyInfoDer;
    use rustls::sign::{Signer, SigningKey};
    use rustls::{SignatureAlgorithm, SignatureScheme};
    use tokio::time::timeout;

    fn reliable_frame(epoch: u64, sequence: u64, event: ReliableInputEvent) -> ReliableInputFrame {
        ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(epoch),
            sequence,
            captured_at: MonotonicStamp::new(ClockDomainId(1), sequence),
            event,
        }
    }

    fn realtime_frame(epoch: u64, sequence: u64) -> RealtimeInputFrame {
        RealtimeInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(epoch),
            sequence,
            captured_at: MonotonicStamp::new(ClockDomainId(1), sequence),
            payload: RealtimeInputPayload::RelativeMouse {
                dx: sequence as i32,
                dy: 0,
            },
        }
    }

    async fn write_raw_qos_input(
        connection: &QuicConnection,
        lane: LaneDiscriminator,
        frame: &ReliableInputFrame,
    ) {
        let mut stream = open_raw_qos_input_stream(connection, lane, frame).await;
        stream.finish().unwrap();
    }

    async fn open_raw_qos_input_stream(
        connection: &QuicConnection,
        lane: LaneDiscriminator,
        frame: &ReliableInputFrame,
    ) -> quinn::SendStream {
        let payload = crate::codec::ReliableInputCodec::encode(frame).unwrap();
        let mut stream = connection.inner.connection.open_uni().await.unwrap();
        stream
            .write_all(&[
                QOS_LANE_MAGIC[0],
                QOS_LANE_MAGIC[1],
                QOS_LANE_MAGIC[2],
                QOS_LANE_MAGIC[3],
                lane as u8,
            ])
            .await
            .unwrap();
        stream
            .write_all(&(payload.len() as u32).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&payload).await.unwrap();
        stream
    }

    async fn write_reliable_frame_to_stream(
        stream: &mut quinn::SendStream,
        frame: &ReliableInputFrame,
    ) {
        let payload = crate::codec::ReliableInputCodec::encode(frame).unwrap();
        stream
            .write_all(&(payload.len() as u32).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&payload).await.unwrap();
    }

    struct TwoEpochReliableFixture {
        client_connection: QuicConnection,
        server_connection: QuicConnection,
        releases: mpsc::Receiver<TerminalReleaseEvent>,
        old_stream: quinn::SendStream,
        new_stream: quinn::SendStream,
        generation: ControlConnectionId,
        _server_qos: PeerTransportHandle,
    }

    async fn two_epoch_reliable_fixture() -> TwoEpochReliableFixture {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let (server_qos, mut releases) = server_connection.install_qos(server_auth);
        let old_stream = open_raw_qos_input_stream(
            &client_connection,
            LaneDiscriminator::ReliableInput,
            &reliable_frame(
                1,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "old".into(),
                    x: 0,
                    y: 0,
                },
            ),
        )
        .await;
        timeout(Duration::from_secs(1), async {
            while server_connection.inner.qos_inbound.lock().unwrap().active
                != Some((generation, SessionEpoch(1), 1))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first epoch must become active");
        let new_stream = open_raw_qos_input_stream(
            &client_connection,
            LaneDiscriminator::ReliableInput,
            &reliable_frame(
                2,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "new".into(),
                    x: 0,
                    y: 0,
                },
            ),
        )
        .await;
        let displaced = timeout(Duration::from_secs(1), releases.recv())
            .await
            .expect("the larger Enter must release the old epoch")
            .unwrap();
        assert_eq!(displaced.epoch, SessionEpoch(1));
        assert_eq!(
            server_connection.inner.qos_inbound.lock().unwrap().active,
            Some((generation, SessionEpoch(2), 1))
        );
        TwoEpochReliableFixture {
            client_connection,
            server_connection,
            releases,
            old_stream,
            new_stream,
            generation,
            _server_qos: server_qos,
        }
    }

    #[test]
    fn legacy_realtime_encoder_is_private_and_rejects_reliable_messages() {
        let realtime = Message::MouseMove { x: 7, y: -4 };
        assert!(encode_legacy_realtime_message(&realtime).unwrap().is_some());

        let reliable = Message::Key {
            keycode: 0x41,
            state: rshare_core::KeyState::Pressed,
        };
        assert!(encode_legacy_realtime_message(&reliable).unwrap().is_none());
    }

    #[tokio::test]
    async fn authenticated_legacy_realtime_datagram_never_enters_message_fifo() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let mut server_connection = incoming.recv().await.unwrap().connection;
        let auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_qos, _releases) = server_connection.install_qos(auth);

        client_connection
            .send_message(&Message::MouseMove { x: 7, y: 9 })
            .await
            .unwrap();

        assert!(
            timeout(
                Duration::from_millis(100),
                server_connection.receive_message()
            )
            .await
            .is_err(),
            "authenticated realtime input must never return as legacy Message"
        );
    }

    #[tokio::test]
    async fn authenticated_reliable_compat_input_fails_closed_without_entering_message_fifo() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let mut server_connection = incoming.recv().await.unwrap().connection;
        let auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_qos, _releases) = server_connection.install_qos(auth);
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);
        let mut protocol_errors = server_connection.take_protocol_errors().unwrap();

        let super::super::qos::ClassifiedMessage::ReliableCompat(frame) =
            super::super::qos::ClassifiedMessage::try_from(Message::Key {
                keycode: 0x41,
                state: rshare_core::KeyState::Pressed,
            })
            .unwrap()
        else {
            panic!("legacy key must classify as reliable compatibility input");
        };
        client_qos.send_reliable_compat(frame).await.unwrap();

        let error = timeout(Duration::from_secs(1), protocol_errors.recv())
            .await
            .expect("legacy input must report a protocol error")
            .unwrap();
        assert!(error.error.contains("reliable compatibility input"));
        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("legacy input without authenticated epoch metadata must fail closed");
        let message = timeout(
            Duration::from_millis(100),
            server_connection.receive_message(),
        )
        .await;
        assert!(
            !matches!(message, Ok(Ok(_))),
            "legacy input must never enter the general Message FIFO"
        );
    }

    #[tokio::test]
    async fn blocked_quic_reliable_stream_still_delivers_emergency_release() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let reliable_reader = Arc::new(Notify::new());

        let connection_id = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: connection_id,
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, mut server_releases) = server_connection.install_qos(server_auth);
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);
        let epoch = SessionEpoch(41);

        client_qos
            .try_send_reliable_input(ReliableInputFrame {
                protocol_version: INPUT_PROTOCOL_VERSION,
                session_epoch: epoch,
                sequence: 1,
                captured_at: MonotonicStamp::new(ClockDomainId(1), 1),
                event: ReliableInputEvent::Enter {
                    target_display_id: "primary".to_string(),
                    x: 0,
                    y: 0,
                },
            })
            .unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                if server_connection
                    .inner
                    .qos_inbound
                    .lock()
                    .expect("qos inbound state poisoned")
                    .active
                    .is_some()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Enter must activate before reliable flow-control is blocked");
        server_connection.set_qos_reliable_reader_barrier(reliable_reader.clone());

        assert_eq!(
            client_qos
                .try_send_realtime(RealtimeInputFrame {
                    protocol_version: INPUT_PROTOCOL_VERSION,
                    session_epoch: epoch,
                    sequence: 1,
                    captured_at: MonotonicStamp::new(ClockDomainId(1), 1),
                    payload: RealtimeInputPayload::RelativeMouse { dx: 1, dy: -1 },
                })
                .unwrap(),
            super::super::qos::RealtimeSendOutcome::Sent
        );
        timeout(Duration::from_secs(1), async {
            loop {
                if server_connection
                    .inner
                    .qos_inbound
                    .lock()
                    .expect("qos inbound state poisoned")
                    .realtime_last
                    .is_some_and(|((_, received_epoch), sequence)| {
                        received_epoch == epoch && sequence == 1
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("remote realtime receiver must update while reliable is blocked");

        let mut next_sequence = 2_u64;
        timeout(Duration::from_secs(2), async {
            loop {
                let sequence = next_sequence;
                next_sequence += 1;
                client_qos
                    .try_send_reliable_input(ReliableInputFrame {
                        protocol_version: INPUT_PROTOCOL_VERSION,
                        session_epoch: epoch,
                        sequence,
                        captured_at: MonotonicStamp::new(ClockDomainId(1), sequence),
                        event: ReliableInputEvent::TextCommit {
                            text: "x".repeat(3_072),
                        },
                    })
                    .unwrap();
                tokio::task::yield_now().await;
                let (started, completed) = client_qos.reliable_write_counts_for_test();
                if started > completed {
                    break;
                }
            }
        })
        .await
        .expect("a real QUIC reliable write must be pending on flow control");
        let overflow_sequence = loop {
            static TEXT: &str = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
            let mut overflow = None;
            for _ in 0..1024_u64 {
                let sequence = next_sequence;
                next_sequence += 1;
                let result = client_qos.try_send_reliable_input(ReliableInputFrame {
                    protocol_version: INPUT_PROTOCOL_VERSION,
                    session_epoch: epoch,
                    sequence,
                    captured_at: MonotonicStamp::new(ClockDomainId(1), sequence),
                    event: ReliableInputEvent::TextCommit {
                        text: TEXT.repeat(48),
                    },
                });
                if result == Err(super::super::qos::TransportSendError::ReliableLaneFull) {
                    overflow = Some(sequence);
                    break;
                }
            }
            if let Some(sequence) = overflow {
                break sequence;
            }
            tokio::task::yield_now().await;
        };
        let release = timeout(Duration::from_secs(3), server_releases.recv())
            .await
            .expect("emergency release must bypass blocked reliable stream")
            .expect("terminal release callback must remain connected");
        assert_eq!(release.epoch, epoch);
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert_eq!(release.auth.control_connection_id, connection_id);
        assert!(overflow_sequence > 1);
        assert!(
            client_qos
                .try_send_reliable_input(ReliableInputFrame {
                    protocol_version: INPUT_PROTOCOL_VERSION,
                    session_epoch: epoch,
                    sequence: overflow_sequence + 1,
                    captured_at: MonotonicStamp::new(ClockDomainId(1), overflow_sequence + 1,),
                    event: ReliableInputEvent::Leave,
                })
                .is_err(),
            "the failed epoch must remain tombstoned"
        );
        timeout(Duration::from_secs(1), async {
            while client_qos.reliable_available_for_test() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retired reliable backlog must be purged before re-entry");
        let next_epoch = SessionEpoch(epoch.0 + 1);
        client_qos
            .try_send_reliable_input(ReliableInputFrame {
                protocol_version: INPUT_PROTOCOL_VERSION,
                session_epoch: next_epoch,
                sequence: 1,
                captured_at: MonotonicStamp::new(ClockDomainId(1), 1),
                event: ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            })
            .unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                if server_connection
                    .inner
                    .qos_inbound
                    .lock()
                    .expect("qos inbound state poisoned")
                    .active
                    .is_some_and(|(_, active_epoch, _)| active_epoch == next_epoch)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("stale cancellation must not consume the next epoch Enter");
        reliable_reader.notify_waiters();
    }

    #[tokio::test]
    async fn connection_close_while_reliable_payload_is_flow_control_blocked_fails_frame() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let reliable_reader = Arc::new(Notify::new());
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, _server_releases) = server_connection.install_qos(server_auth);
        let (client_qos, mut client_releases) = client_connection.install_qos(client_auth);
        let worker_probe = client_qos.worker_probe_for_test();
        let epoch = SessionEpoch(42);
        client_qos
            .try_send_reliable_input(reliable_frame(
                epoch.0,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                if server_connection
                    .inner
                    .qos_inbound
                    .lock()
                    .expect("qos inbound state poisoned")
                    .active
                    .is_some()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Enter must activate before blocking the payload reader");
        server_connection.set_qos_reliable_reader_barrier(reliable_reader);

        let mut sequence = 2_u64;
        timeout(Duration::from_secs(5), async {
            loop {
                let (started_before, _) = client_qos.reliable_write_counts_for_test();
                client_qos
                    .try_send_reliable_input(reliable_frame(
                        epoch.0,
                        sequence,
                        ReliableInputEvent::TextCommit {
                            text: "x".repeat(3_072),
                        },
                    ))
                    .unwrap();
                sequence += 1;
                while client_qos.reliable_write_counts_for_test().0 == started_before {
                    tokio::task::yield_now().await;
                }
                let started = client_qos.reliable_write_counts_for_test().0;
                if timeout(Duration::from_millis(10), async {
                    while client_qos.reliable_write_counts_for_test().1 < started {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .is_err()
                {
                    break;
                }
            }
        })
        .await
        .expect("a real QUIC payload write must become flow-control blocked");
        let (started, completed) = client_qos.reliable_write_counts_for_test();
        assert!(
            started > completed,
            "the payload write must still be pending"
        );

        client_qos.pause_reliable_payload_poll_for_test();
        client_connection
            .inner
            .connection
            .close(0u32.into(), b"close blocked reliable payload");
        let release = timeout(Duration::from_secs(1), client_releases.recv())
            .await
            .expect("connection close must fail the blocked payload frame")
            .expect("typed terminal release stream must remain connected");
        assert_eq!(release.epoch, epoch);
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert!(client_qos.is_tombstoned(epoch));
        assert_eq!(
            client_qos.try_send_reliable_input(reliable_frame(
                epoch.0,
                sequence,
                ReliableInputEvent::Key {
                    keycode: 0x41,
                    state: rshare_core::KeyState::Pressed,
                },
            )),
            Err(super::super::qos::TransportSendError::UnsupportedMessage)
        );

        drop(client_qos);
        timeout(Duration::from_secs(1), async {
            while worker_probe.running() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all QoS workers must exit after the closed transport is dropped");
    }

    #[tokio::test]
    async fn overwritten_cancel_still_resets_blocked_old_epoch_and_allows_new_enter() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let reliable_reader = Arc::new(Notify::new());
        let generation = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, _server_releases) = server_connection.install_qos(server_auth);
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);
        let blocked_epoch = SessionEpoch(50);
        client_qos
            .try_send_reliable_input(reliable_frame(
                blocked_epoch.0,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "blocked".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        timeout(Duration::from_secs(1), async {
            while server_connection.inner.qos_inbound.lock().unwrap().active
                != Some((generation, blocked_epoch, 1))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the blocked epoch must first become active");
        server_connection.set_qos_reliable_reader_barrier(reliable_reader.clone());

        let mut sequence = 2;
        timeout(Duration::from_secs(2), async {
            loop {
                client_qos
                    .try_send_reliable_input(reliable_frame(
                        blocked_epoch.0,
                        sequence,
                        ReliableInputEvent::TextCommit {
                            text: "x".repeat(3_072),
                        },
                    ))
                    .unwrap();
                sequence += 1;
                tokio::task::yield_now().await;
                let (started, completed) = client_qos.reliable_write_counts_for_test();
                if started > completed {
                    break;
                }
            }
        })
        .await
        .expect("a real reliable write must be flow-control blocked");

        client_qos.publish_cancel_for_test(blocked_epoch);
        client_qos.publish_cancel_for_test(SessionEpoch(blocked_epoch.0 + 1));
        let replacement_epoch = SessionEpoch(blocked_epoch.0 + 2);
        client_qos
            .try_send_reliable_input(reliable_frame(
                replacement_epoch.0,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "replacement".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();

        timeout(Duration::from_secs(1), async {
            while server_connection.inner.qos_inbound.lock().unwrap().active
                != Some((generation, replacement_epoch, 1))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled-through must not lose the blocked epoch cancellation");
        reliable_reader.notify_waiters();
    }

    #[tokio::test]
    async fn reliable_preface_cancel_resets_unbound_stream_and_allows_next_epoch_enter() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, mut server_releases) = server_connection.install_qos(server_auth);
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);
        client_qos.set_reliable_preface_barrier_for_test(Arc::new(Notify::new()));
        let cancelled_epoch = SessionEpoch(70);
        client_qos
            .try_send_reliable_input(reliable_frame(
                cancelled_epoch.0,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "cancelled".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        timeout(Duration::from_secs(1), async {
            while !client_qos.reliable_preface_waiting_for_test() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the reliable writer must open a real QUIC stream and block before its preface");

        client_qos.publish_cancel_for_test(cancelled_epoch);
        let next_epoch = SessionEpoch(cancelled_epoch.0 + 1);
        client_qos
            .try_send_reliable_input(reliable_frame(
                next_epoch.0,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "next".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                let reset_succeeded = client_qos.reliable_cancel_reset_succeeded_for_test() != 0;
                let reset_observed = server_connection
                    .inner
                    .qos_terminal_cancel_resets_observed
                    .load(Ordering::Acquire)
                    != 0;
                let next_active = server_connection
                    .inner
                    .qos_inbound
                    .lock()
                    .expect("qos inbound state poisoned")
                    .active
                    == Some((generation, next_epoch, 1));
                if reset_succeeded && reset_observed && next_active {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("preface cancellation must reset the unbound stream and unblock the next Enter");
        assert!(
            server_connection.inner.connection.close_reason().is_none(),
            "a dedicated unbound cancel reset must keep the connection open"
        );
        assert!(
            server_releases.try_recv().is_err(),
            "an unbound cancel reset must not release any unrelated active epoch"
        );
    }

    #[tokio::test]
    async fn authenticated_qos_control_and_bulk_use_separate_framed_streams() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, _server_releases) = server_connection.install_qos(server_auth);
        let mut inbound = server_connection.take_peer_inbound().unwrap();
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);

        client_qos
            .send_control(super::super::qos::ControlFrame::heartbeat(7, 11))
            .await
            .unwrap();
        let stream_id = uuid::Uuid::new_v4();
        client_qos
            .send_bulk(super::super::qos::BulkFrame::audio_stream_stop(
                stream_id,
                "done".into(),
            ))
            .await
            .unwrap();
        client_qos
            .try_send_telemetry(super::super::qos::TelemetryFrame::latency_probe(
                13, 17, false, None,
            ))
            .unwrap();

        timeout(Duration::from_secs(1), inbound.control_rx.recv())
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(1), inbound.bulk_rx.recv())
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(1), inbound.telemetry_rx.recv())
            .await
            .unwrap()
            .unwrap();

        for message in [
            Message::ClipboardData {
                mime_type: "text/plain".into(),
                data: vec![1, 2, 3],
            },
            Message::AudioStreamStop {
                stream_id: uuid::Uuid::new_v4(),
                reason: "compat-audio".into(),
            },
            Message::UsbDeviceDetached {
                bus_id: "usb:1-2".into(),
                reason: "gone".into(),
            },
        ] {
            let super::super::qos::ClassifiedMessage::Bulk(frame) =
                super::super::qos::ClassifiedMessage::try_from(message).unwrap()
            else {
                panic!("expected bulk lane");
            };
            client_qos.send_bulk(frame).await.unwrap();
            timeout(Duration::from_secs(1), inbound.bulk_rx.recv())
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn saturated_control_receiver_does_not_stop_realtime_latest_drain() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, _server_releases) = server_connection.install_qos(server_auth);
        let mut inbound = server_connection
            .take_peer_inbound()
            .expect("authenticated connection must expose typed inbound lanes");
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);

        client_qos
            .try_send_reliable_input(reliable_frame(
                1,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        assert_eq!(
            timeout(Duration::from_secs(1), inbound.reliable_input_rx.recv())
                .await
                .unwrap()
                .unwrap()
                .sequence,
            1
        );

        for sequence in 0..128 {
            client_qos
                .send_control(super::super::qos::ControlFrame::heartbeat(
                    sequence, sequence,
                ))
                .await
                .unwrap();
        }
        for sequence in 1..=100 {
            client_qos
                .try_send_realtime(realtime_frame(1, sequence))
                .unwrap();
        }

        let latest = timeout(Duration::from_millis(100), async {
            loop {
                let frame = inbound.realtime_rx.recv().await.unwrap();
                if frame.sequence == 100 {
                    break frame;
                }
            }
        })
        .await
        .expect("blocked control receiver must not stop realtime datagram drain");
        assert_eq!(latest.sequence, 100);
        assert_eq!(inbound.auth.control_connection_id, generation);
    }

    #[tokio::test]
    async fn full_realtime_downstream_keeps_only_latest_and_worker_exits_on_drop() {
        let (target_tx, mut target_rx) = mpsc::channel(1);
        target_tx.try_send(realtime_frame(7, 0)).unwrap();
        assert_eq!(target_rx.len(), 1, "the downstream must start full");

        let emitter = LatestRealtimeEmitter::new(target_tx);
        let workers = emitter.worker_probe_for_test();
        for sequence in 1..=100 {
            emitter.emit(realtime_frame(7, sequence));
        }

        assert_eq!(target_rx.recv().await.unwrap().sequence, 0);
        let latest = timeout(Duration::from_secs(1), async {
            loop {
                let frame = target_rx.recv().await.unwrap();
                if frame.sequence == 100 {
                    break frame;
                }
            }
        })
        .await
        .expect("unblocking a full downstream must deliver the latest replacement");
        assert_eq!(latest.sequence, 100);

        drop(emitter);
        timeout(Duration::from_secs(1), async {
            while workers.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the latest bridge worker must exit when its sender lifetime ends");
    }

    #[tokio::test]
    async fn reliable_input_has_an_independent_bounded_ordered_receiver() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, _server_releases) = server_connection.install_qos(server_auth);
        let mut inbound = server_connection.take_peer_inbound().unwrap();
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);

        for frame in [
            reliable_frame(
                9,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ),
            reliable_frame(9, 2, ReliableInputEvent::Leave),
            reliable_frame(
                9,
                3,
                ReliableInputEvent::Wheel {
                    delta_x: 0,
                    delta_y: 1,
                },
            ),
        ] {
            client_qos.try_send_reliable_input(frame).unwrap();
        }

        let mut sequences = Vec::new();
        for _ in 0..3 {
            sequences.push(
                timeout(Duration::from_secs(1), inbound.reliable_input_rx.recv())
                    .await
                    .unwrap()
                    .unwrap()
                    .sequence,
            );
        }
        assert_eq!(sequences, vec![1, 2, 3]);
        assert!(inbound.reliable_input_rx.capacity() < usize::MAX);
    }

    #[tokio::test]
    async fn full_reliable_input_receiver_fails_closed_and_requests_release() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let (_qos, mut releases) = server_connection.install_qos(auth);
        let inbound = server_connection.take_peer_inbound().unwrap();
        let epoch = SessionEpoch(17);
        let mut stream = open_raw_qos_input_stream(
            &client_connection,
            LaneDiscriminator::ReliableInput,
            &reliable_frame(
                epoch.0,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ),
        )
        .await;
        for sequence in 2..=257 {
            write_reliable_frame_to_stream(
                &mut stream,
                &reliable_frame(
                    epoch.0,
                    sequence,
                    ReliableInputEvent::Wheel {
                        delta_x: 0,
                        delta_y: 1,
                    },
                ),
            )
            .await;
        }
        stream.finish().unwrap();

        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("full reliable receiver must close the authenticated connection");
        let release = timeout(Duration::from_secs(1), releases.recv())
            .await
            .expect("full reliable receiver must request local release")
            .unwrap();
        assert_eq!(release.auth.control_connection_id, generation);
        assert_eq!(release.epoch, epoch);
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert_eq!(inbound.reliable_input_rx.len(), 256);
    }

    #[tokio::test]
    async fn unknown_lane_discriminator_is_stream_local_and_reports_protocol_error() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_qos, _releases) = server_connection.install_qos(auth.clone());
        let mut protocol_errors = server_connection
            .take_protocol_errors()
            .expect("authenticated connection must expose protocol errors");

        let mut malformed = client_connection.inner.connection.open_uni().await.unwrap();
        malformed
            .write_all(&[
                QOS_LANE_MAGIC[0],
                QOS_LANE_MAGIC[1],
                QOS_LANE_MAGIC[2],
                QOS_LANE_MAGIC[3],
                0xff,
            ])
            .await
            .unwrap();
        malformed.finish().unwrap();

        let error = timeout(Duration::from_secs(1), protocol_errors.recv())
            .await
            .expect("unknown lane must report a protocol error")
            .unwrap();
        assert_eq!(error.auth.control_connection_id, auth.control_connection_id);
        assert!(error.error.contains("unknown qos lane"));
        assert!(
            server_connection.inner.connection.close_reason().is_none(),
            "malformed lane must only close its own stream"
        );
    }

    #[tokio::test]
    async fn completed_malformed_stream_flood_cannot_block_realtime() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, _releases) = server_connection.install_qos(server_auth);
        let mut inbound = server_connection.take_peer_inbound().unwrap();
        let protocol_errors = server_connection.take_protocol_errors().unwrap();
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);

        client_qos
            .try_send_reliable_input(reliable_frame(
                1,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        timeout(Duration::from_secs(1), inbound.reliable_input_rx.recv())
            .await
            .expect("reliable enter must activate the realtime epoch")
            .unwrap();

        for _ in 0..32 {
            let completed_before = server_connection
                .authenticated_uni_stream_task_counts_for_test()
                .4;
            let mut malformed = client_connection.inner.connection.open_uni().await.unwrap();
            malformed
                .write_all(&[
                    QOS_LANE_MAGIC[0],
                    QOS_LANE_MAGIC[1],
                    QOS_LANE_MAGIC[2],
                    QOS_LANE_MAGIC[3],
                    0xff,
                ])
                .await
                .unwrap();
            malformed.finish().unwrap();
            timeout(Duration::from_secs(1), async {
                while server_connection
                    .authenticated_uni_stream_task_counts_for_test()
                    .4
                    == completed_before
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("each completed malformed stream must release its reader permit");
        }
        timeout(Duration::from_secs(1), async {
            while protocol_errors.len() != 32 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("malformed diagnostics FIFO must become full");
        timeout(Duration::from_secs(1), async {
            while server_connection
                .authenticated_uni_stream_task_counts_for_test()
                .1
                != 1
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completed malformed streams must release all reader permits");

        let counts_before_blocked_diagnostic =
            server_connection.authenticated_uni_stream_task_counts_for_test();
        let mut blocked_diagnostic = client_connection.inner.connection.open_uni().await.unwrap();
        blocked_diagnostic
            .write_all(&[
                QOS_LANE_MAGIC[0],
                QOS_LANE_MAGIC[1],
                QOS_LANE_MAGIC[2],
                QOS_LANE_MAGIC[3],
                0xff,
            ])
            .await
            .unwrap();
        blocked_diagnostic.finish().unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                let counts = server_connection.authenticated_uni_stream_task_counts_for_test();
                if counts.0 > counts_before_blocked_diagnostic.0
                    && counts.4 > counts_before_blocked_diagnostic.4
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("full diagnostics FIFO must not park an unknown-lane handler");

        let (_accepted, active, peak, rejected, _completed) =
            server_connection.authenticated_uni_stream_task_counts_for_test();
        assert!(active <= AUTHENTICATED_UNI_STREAM_TASK_BUDGET);
        assert!(peak <= AUTHENTICATED_UNI_STREAM_TASK_BUDGET);
        assert_eq!(rejected, 0);

        client_qos.try_send_realtime(realtime_frame(1, 1)).unwrap();
        assert_eq!(
            timeout(Duration::from_secs(1), inbound.realtime_rx.recv())
                .await
                .expect("realtime datagram must bypass blocked malformed diagnostics")
                .unwrap()
                .sequence,
            1
        );
        assert!(server_connection.inner.connection.close_reason().is_none());
        assert!(client_connection.inner.connection.close_reason().is_none());

        drop(protocol_errors);
    }

    #[tokio::test]
    async fn partial_stream_budget_exhaustion_fails_closed_and_releases_active_epoch() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, mut releases) = server_connection.install_qos(server_auth);
        let mut inbound = server_connection.take_peer_inbound().unwrap();
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);
        let epoch = SessionEpoch(7);

        client_qos
            .try_send_reliable_input(reliable_frame(
                epoch.0,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        let enter = timeout(Duration::from_secs(1), inbound.reliable_input_rx.recv())
            .await
            .expect("reliable enter must activate the epoch")
            .unwrap();
        assert_eq!(enter.session_epoch, epoch);

        let active_reliable_streams = server_connection
            .authenticated_uni_stream_task_counts_for_test()
            .1;
        assert_eq!(active_reliable_streams, 1);
        let mut partial_streams = Vec::new();
        for _ in active_reliable_streams..AUTHENTICATED_UNI_STREAM_TASK_BUDGET {
            let mut stream = client_connection.inner.connection.open_uni().await.unwrap();
            stream.write_all(&QOS_LANE_MAGIC[..1]).await.unwrap();
            partial_streams.push(stream);
        }
        timeout(Duration::from_secs(1), async {
            while server_connection
                .authenticated_uni_stream_task_counts_for_test()
                .1
                != AUTHENTICATED_UNI_STREAM_TASK_BUDGET
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the active reliable stream plus partial streams must occupy the reader budget");

        let mut emergency = open_raw_qos_input_stream(
            &client_connection,
            LaneDiscriminator::Emergency,
            &reliable_frame(
                epoch.0,
                2,
                ReliableInputEvent::ReleaseAll {
                    reason: ReleaseAllReason::SessionEnded,
                },
            ),
        )
        .await;
        let _ = emergency.finish();

        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("reader budget exhaustion must fail-close the authenticated connection");
        let release = timeout(Duration::from_secs(1), releases.recv())
            .await
            .expect("connection close must release the active input epoch")
            .unwrap();
        assert_eq!(release.auth.control_connection_id, generation);
        assert_eq!(release.epoch, epoch);
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert_eq!(
            server_connection
                .inner
                .qos_inbound
                .lock()
                .unwrap()
                .retired_through,
            Some((generation, epoch))
        );

        let (_accepted, active, peak, rejected, _completed) =
            server_connection.authenticated_uni_stream_task_counts_for_test();
        assert!(active <= AUTHENTICATED_UNI_STREAM_TASK_BUDGET);
        assert!(peak <= AUTHENTICATED_UNI_STREAM_TASK_BUDGET);
        assert_eq!(rejected, 1);
        drop(partial_streams);
    }

    #[tokio::test]
    async fn missing_qos_lane_magic_is_stream_local_and_reports_authenticated_error() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_qos, _releases) = server_connection.install_qos(auth.clone());
        let mut protocol_errors = server_connection.take_protocol_errors().unwrap();

        let mut malformed = client_connection.inner.connection.open_uni().await.unwrap();
        malformed.write_all(b"NOPE").await.unwrap();
        malformed.finish().unwrap();

        let error = timeout(Duration::from_secs(1), protocol_errors.recv())
            .await
            .expect("missing lane magic must report a protocol error")
            .unwrap();
        assert_eq!(error.auth.control_connection_id, auth.control_connection_id);
        assert!(error.error.contains("missing qos lane preface"));
        assert!(
            server_connection.inner.connection.close_reason().is_none(),
            "missing lane magic must only stop its stream"
        );
    }

    #[tokio::test]
    async fn truncated_qos_lane_discriminator_is_stream_local_and_reports_authenticated_error() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_qos, _releases) = server_connection.install_qos(auth.clone());
        let mut protocol_errors = server_connection.take_protocol_errors().unwrap();

        let mut malformed = client_connection.inner.connection.open_uni().await.unwrap();
        malformed.write_all(QOS_LANE_MAGIC).await.unwrap();
        malformed.finish().unwrap();

        let error = timeout(Duration::from_secs(1), protocol_errors.recv())
            .await
            .expect("truncated lane discriminator must report a protocol error")
            .unwrap();
        assert_eq!(error.auth.control_connection_id, auth.control_connection_id);
        assert!(error.error.contains("truncated qos lane discriminator"));
        assert!(
            server_connection.inner.connection.close_reason().is_none(),
            "truncated lane discriminator must only stop its stream"
        );
    }

    #[tokio::test]
    async fn occupied_emergency_slot_closes_quic_and_releases_remote_active_epoch() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, mut server_releases) = server_connection.install_qos(server_auth);
        let (client_qos, mut client_releases) = client_connection.install_qos(client_auth);
        let epoch = SessionEpoch(73);
        client_qos
            .try_send_reliable_input(ReliableInputFrame {
                protocol_version: INPUT_PROTOCOL_VERSION,
                session_epoch: epoch,
                sequence: 1,
                captured_at: MonotonicStamp::new(ClockDomainId(1), 1),
                event: ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            })
            .unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                if server_connection
                    .inner
                    .qos_inbound
                    .lock()
                    .expect("qos inbound state poisoned")
                    .active
                    .is_some()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("receiver must activate the epoch");

        client_qos.occupy_emergency_slot_for_test();
        assert_eq!(
            client_qos.try_send_emergency(ReliableInputFrame {
                protocol_version: INPUT_PROTOCOL_VERSION,
                session_epoch: epoch,
                sequence: 2,
                captured_at: MonotonicStamp::new(ClockDomainId(1), 2),
                event: ReliableInputEvent::ReleaseAll {
                    reason: ReleaseAllReason::BackendFailure,
                },
            }),
            Err(super::super::qos::TransportSendError::EmergencySlotFull)
        );
        let local = timeout(Duration::from_secs(1), client_releases.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(local.epoch, epoch);
        let remote = timeout(Duration::from_secs(1), server_releases.recv())
            .await
            .expect("connection loss must release the remote active epoch")
            .unwrap();
        assert_eq!(remote.epoch, epoch);
        assert_eq!(remote.reason, ReleaseAllReason::BackendFailure);
    }

    #[tokio::test]
    async fn authenticated_wrong_lane_message_is_rejected() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_qos, _releases) = server_connection.install_qos(auth);
        let mut protocol_errors = server_connection.take_protocol_errors().unwrap();
        let message = Message::AudioStreamStop {
            stream_id: uuid::Uuid::new_v4(),
            reason: "wrong-lane".into(),
        };
        let payload = ControlMessageCodec::encode(&message).unwrap();
        let mut stream = client_connection.inner.connection.open_uni().await.unwrap();
        stream
            .write_all(&[
                QOS_LANE_MAGIC[0],
                QOS_LANE_MAGIC[1],
                QOS_LANE_MAGIC[2],
                QOS_LANE_MAGIC[3],
                LaneDiscriminator::Control as u8,
            ])
            .await
            .unwrap();
        stream
            .write_all(&(payload.len() as u32).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&payload).await.unwrap();
        stream.finish().unwrap();
        let error = timeout(Duration::from_secs(1), protocol_errors.recv())
            .await
            .expect("wrong-lane payload must report a protocol error")
            .unwrap();
        assert!(error.error.contains("wrong qos lane"));
        assert!(client_connection.inner.connection.close_reason().is_none());
    }

    #[tokio::test]
    async fn truncated_authenticated_message_lane_closes_connection() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_qos, _releases) = server_connection.install_qos(auth);
        let mut stream = client_connection.inner.connection.open_uni().await.unwrap();
        stream
            .write_all(&[
                QOS_LANE_MAGIC[0],
                QOS_LANE_MAGIC[1],
                QOS_LANE_MAGIC[2],
                QOS_LANE_MAGIC[3],
                LaneDiscriminator::Control as u8,
            ])
            .await
            .unwrap();
        stream.write_all(&16u32.to_be_bytes()).await.unwrap();
        stream.write_all(&[1, 2]).await.unwrap();
        stream.finish().unwrap();
        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("truncated authenticated message lane must close the connection");
    }

    #[tokio::test]
    async fn future_emergency_closes_and_releases_only_receiver_active_epoch() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, mut releases) = server_connection.install_qos(server_auth);
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);
        client_qos
            .try_send_reliable_input(reliable_frame(
                1,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                if server_connection
                    .inner
                    .qos_inbound
                    .lock()
                    .unwrap()
                    .active
                    .is_some()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        write_raw_qos_input(
            &client_connection,
            LaneDiscriminator::Emergency,
            &reliable_frame(
                2,
                1,
                ReliableInputEvent::ReleaseAll {
                    reason: ReleaseAllReason::SessionEnded,
                },
            ),
        )
        .await;
        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("future emergency must close the authenticated connection");
        let release = timeout(Duration::from_secs(1), releases.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(release.epoch, SessionEpoch(1));
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert_eq!(
            server_connection
                .inner
                .qos_inbound
                .lock()
                .unwrap()
                .retired_through,
            Some((generation, SessionEpoch(1)))
        );
    }

    #[tokio::test]
    async fn sender_future_emergency_closes_and_releases_sender_active_epoch() {
        let server_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(DeviceId::new_v4());
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let _server_connection = incoming.recv().await.unwrap().connection;
        let auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (handle, mut releases) = client_connection.install_qos(auth);
        handle
            .try_send_reliable_input(reliable_frame(
                1,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        assert_eq!(
            handle.try_send_emergency(reliable_frame(
                2,
                1,
                ReliableInputEvent::ReleaseAll {
                    reason: ReleaseAllReason::SessionEnded,
                },
            )),
            Err(super::super::qos::TransportSendError::UnsupportedMessage)
        );
        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("sender future emergency must close the connection");
        let release = timeout(Duration::from_secs(1), releases.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(release.epoch, SessionEpoch(1));
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert!(!handle.is_tombstoned(SessionEpoch(2)));
    }

    #[tokio::test]
    async fn late_duplicate_enter_after_terminal_epoch_fails_closed_without_duplicate_release() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let (_server_qos, mut releases) = server_connection.install_qos(auth);

        let mut stream = open_raw_qos_input_stream(
            &client_connection,
            LaneDiscriminator::ReliableInput,
            &reliable_frame(
                1,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ),
        )
        .await;
        write_reliable_frame_to_stream(
            &mut stream,
            &reliable_frame(
                1,
                2,
                ReliableInputEvent::ReleaseAll {
                    reason: ReleaseAllReason::SessionEnded,
                },
            ),
        )
        .await;
        let terminal = timeout(Duration::from_secs(1), releases.recv())
            .await
            .expect("the terminal frame must request the normal release")
            .unwrap();
        assert_eq!(terminal.epoch, SessionEpoch(1));
        assert_eq!(terminal.reason, ReleaseAllReason::SessionEnded);

        let _late = open_raw_qos_input_stream(
            &client_connection,
            LaneDiscriminator::ReliableInput,
            &reliable_frame(
                1,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "late".into(),
                    x: 0,
                    y: 0,
                },
            ),
        )
        .await;
        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("a late frame for the terminal epoch must fail closed");
        assert_eq!(
            server_connection
                .inner
                .qos_inbound
                .lock()
                .unwrap()
                .retired_through,
            Some((generation, SessionEpoch(1)))
        );
        assert!(
            timeout(Duration::from_millis(50), releases.recv())
                .await
                .is_err(),
            "the generation-wide high-water must coalesce a duplicate terminal epoch"
        );
    }

    #[tokio::test]
    async fn larger_enter_retires_old_epoch_and_ignores_delayed_old_terminal_signals() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let (_server_qos, mut releases) = server_connection.install_qos(server_auth);

        let mut old_stream = open_raw_qos_input_stream(
            &client_connection,
            LaneDiscriminator::ReliableInput,
            &reliable_frame(
                1,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "old".into(),
                    x: 0,
                    y: 0,
                },
            ),
        )
        .await;
        timeout(Duration::from_secs(1), async {
            while server_connection.inner.qos_inbound.lock().unwrap().active
                != Some((generation, SessionEpoch(1), 1))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first epoch must become active");

        let _new_stream = open_raw_qos_input_stream(
            &client_connection,
            LaneDiscriminator::ReliableInput,
            &reliable_frame(
                2,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "new".into(),
                    x: 0,
                    y: 0,
                },
            ),
        )
        .await;
        let retired = timeout(Duration::from_secs(1), releases.recv())
            .await
            .expect("the larger Enter must release the displaced epoch")
            .unwrap();
        assert_eq!(retired.epoch, SessionEpoch(1));
        assert_eq!(retired.reason, ReleaseAllReason::BackendFailure);
        assert_eq!(
            server_connection.inner.qos_inbound.lock().unwrap().active,
            Some((generation, SessionEpoch(2), 1))
        );

        old_stream
            .reset(quinn::VarInt::from_u32(TERMINAL_CANCEL_RESET_CODE))
            .unwrap();
        write_raw_qos_input(
            &client_connection,
            LaneDiscriminator::Emergency,
            &reliable_frame(
                1,
                2,
                ReliableInputEvent::ReleaseAll {
                    reason: ReleaseAllReason::SessionEnded,
                },
            ),
        )
        .await;

        assert!(
            timeout(Duration::from_millis(50), releases.recv())
                .await
                .is_err(),
            "old reset and delayed emergency must not release the replacement epoch"
        );
        assert_eq!(
            server_connection.inner.qos_inbound.lock().unwrap().active,
            Some((generation, SessionEpoch(2), 1))
        );
        assert!(
            timeout(
                Duration::from_millis(50),
                client_connection.inner.connection.closed()
            )
            .await
            .is_err(),
            "recognized old terminal signals must not close the live connection"
        );
    }

    #[tokio::test]
    async fn retired_old_stream_fin_does_not_close_or_release_active_replacement() {
        let mut fixture = two_epoch_reliable_fixture().await;
        fixture.old_stream.finish().unwrap();
        timeout(Duration::from_secs(1), async {
            while fixture
                .server_connection
                .inner
                .qos_reliable_faults_handled
                .load(Ordering::Acquire)
                == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the old stream FIN must be observed by its reader");

        let close_reason = fixture.server_connection.inner.connection.close_reason();
        assert!(
            close_reason.is_none(),
            "an old retired stream FIN must not close the connection: {close_reason:?}"
        );
        assert_eq!(
            fixture
                .server_connection
                .inner
                .qos_inbound
                .lock()
                .unwrap()
                .active,
            Some((fixture.generation, SessionEpoch(2), 1))
        );
        assert!(fixture.releases.try_recv().is_err());
        let _ = fixture.new_stream;
        let _ = fixture.client_connection;
    }

    #[tokio::test]
    async fn retired_old_stream_nonterminal_reset_does_not_close_active_replacement() {
        let mut fixture = two_epoch_reliable_fixture().await;
        fixture
            .old_stream
            .reset(quinn::VarInt::from_u32(77))
            .unwrap();
        timeout(Duration::from_secs(1), async {
            while fixture
                .server_connection
                .inner
                .qos_reliable_faults_handled
                .load(Ordering::Acquire)
                == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the old stream reset must be observed by its reader");

        let close_reason = fixture.server_connection.inner.connection.close_reason();
        assert!(
            close_reason.is_none(),
            "an old retired stream reset must not close the connection: {close_reason:?}"
        );
        assert_eq!(
            fixture
                .server_connection
                .inner
                .qos_inbound
                .lock()
                .unwrap()
                .active,
            Some((fixture.generation, SessionEpoch(2), 1))
        );
        assert!(fixture.releases.try_recv().is_err());
        let _ = fixture.new_stream;
        let _ = fixture.client_connection;
    }

    #[tokio::test]
    async fn active_stream_nonterminal_reset_still_fails_closed() {
        let mut fixture = two_epoch_reliable_fixture().await;
        fixture
            .new_stream
            .reset(quinn::VarInt::from_u32(78))
            .unwrap();
        timeout(
            Duration::from_secs(1),
            fixture.client_connection.inner.connection.closed(),
        )
        .await
        .expect("a nonterminal reset of the active stream must close the connection");
        let release = timeout(Duration::from_secs(1), fixture.releases.recv())
            .await
            .expect("the active reset must release active input")
            .unwrap();
        assert_eq!(release.epoch, SessionEpoch(2));
        let _ = fixture.old_stream;
    }

    #[tokio::test]
    async fn unbound_terminal_cancel_reset_does_not_disturb_active_epoch() {
        let mut fixture = two_epoch_reliable_fixture().await;
        let mut cancelled = fixture
            .client_connection
            .inner
            .connection
            .open_uni()
            .await
            .unwrap();
        cancelled
            .reset(quinn::VarInt::from_u32(TERMINAL_CANCEL_RESET_CODE))
            .unwrap();
        timeout(Duration::from_secs(1), async {
            while fixture
                .server_connection
                .inner
                .qos_terminal_cancel_resets_observed
                .load(Ordering::Acquire)
                == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the peer must observe the explicit unbound terminal cancel reset");

        assert!(fixture
            .server_connection
            .inner
            .connection
            .close_reason()
            .is_none());
        assert_eq!(
            fixture
                .server_connection
                .inner
                .qos_inbound
                .lock()
                .expect("qos inbound state poisoned")
                .active,
            Some((fixture.generation, SessionEpoch(2), 1))
        );
        assert!(fixture.releases.try_recv().is_err());
        let _ = fixture.old_stream;
        let _ = fixture.new_stream;
    }

    #[tokio::test]
    async fn unbound_unknown_reset_still_fails_closed_and_releases_active_epoch() {
        let mut fixture = two_epoch_reliable_fixture().await;
        let mut unknown = fixture
            .client_connection
            .inner
            .connection
            .open_uni()
            .await
            .unwrap();
        unknown.reset(quinn::VarInt::from_u32(79)).unwrap();

        timeout(
            Duration::from_secs(1),
            fixture.client_connection.inner.connection.closed(),
        )
        .await
        .expect("an unknown unbound reset must fail closed");
        let release = timeout(Duration::from_secs(1), fixture.releases.recv())
            .await
            .expect("connection close must release the active epoch")
            .unwrap();
        assert_eq!(release.epoch, SessionEpoch(2));
        let _ = fixture.old_stream;
        let _ = fixture.new_stream;
    }

    #[tokio::test]
    async fn retired_old_stream_cannot_inject_non_enter_for_active_replacement_epoch() {
        let mut fixture = two_epoch_reliable_fixture().await;
        write_reliable_frame_to_stream(
            &mut fixture.old_stream,
            &reliable_frame(
                2,
                2,
                ReliableInputEvent::Key {
                    keycode: 0x41,
                    state: rshare_core::KeyState::Pressed,
                },
            ),
        )
        .await;

        timeout(
            Duration::from_secs(1),
            fixture.client_connection.inner.connection.closed(),
        )
        .await
        .expect("a retired old stream must not inject a non-Enter frame for the active epoch");
        let release = timeout(Duration::from_secs(1), fixture.releases.recv())
            .await
            .expect("cross-stream epoch injection must release the current active epoch")
            .unwrap();
        assert_eq!(release.epoch, SessionEpoch(2));
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert_eq!(
            fixture
                .server_connection
                .inner
                .qos_inbound
                .lock()
                .expect("qos inbound state poisoned")
                .active,
            None
        );
        let _ = fixture.new_stream;
        let _ = fixture.generation;
    }

    #[tokio::test]
    async fn truncated_reliable_frame_closes_and_releases_current_epoch() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, mut releases) = server_connection.install_qos(server_auth);
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);
        client_qos
            .try_send_reliable_input(reliable_frame(
                4,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        client_qos
            .try_send_reliable_input(reliable_frame(
                4,
                2,
                ReliableInputEvent::Key {
                    keycode: 0x41,
                    state: rshare_core::KeyState::Pressed,
                },
            ))
            .unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                if server_connection.inner.qos_inbound.lock().unwrap().active
                    == Some((generation, SessionEpoch(4), 2))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let mut stream = client_connection.inner.connection.open_uni().await.unwrap();
        stream
            .write_all(&[
                QOS_LANE_MAGIC[0],
                QOS_LANE_MAGIC[1],
                QOS_LANE_MAGIC[2],
                QOS_LANE_MAGIC[3],
                LaneDiscriminator::ReliableInput as u8,
            ])
            .await
            .unwrap();
        stream.write_all(&16u32.to_be_bytes()).await.unwrap();
        stream.write_all(&[1, 2]).await.unwrap();
        stream.finish().unwrap();

        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("truncated reliable frame must close the connection");
        let release = timeout(Duration::from_secs(1), releases.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(release.epoch, SessionEpoch(4));
    }

    #[tokio::test]
    async fn reliable_fin_at_frame_boundary_still_closes_when_epoch_is_active() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, mut releases) = server_connection.install_qos(server_auth);
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);
        client_qos
            .try_send_reliable_input(reliable_frame(
                5,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ))
            .unwrap();
        client_qos
            .try_send_reliable_input(reliable_frame(
                5,
                2,
                ReliableInputEvent::Key {
                    keycode: 0x41,
                    state: rshare_core::KeyState::Pressed,
                },
            ))
            .unwrap();
        timeout(Duration::from_secs(1), async {
            while server_connection.inner.qos_inbound.lock().unwrap().active
                != Some((generation, SessionEpoch(5), 2))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let mut stream = client_connection.inner.connection.open_uni().await.unwrap();
        stream
            .write_all(&[
                QOS_LANE_MAGIC[0],
                QOS_LANE_MAGIC[1],
                QOS_LANE_MAGIC[2],
                QOS_LANE_MAGIC[3],
                LaneDiscriminator::ReliableInput as u8,
            ])
            .await
            .unwrap();
        stream.finish().unwrap();

        timeout(
            Duration::from_secs(1),
            client_connection.inner.connection.closed(),
        )
        .await
        .expect("frame-boundary FIN with active input must close the connection");
        assert_eq!(
            timeout(Duration::from_secs(1), releases.recv())
                .await
                .unwrap()
                .unwrap()
                .epoch,
            SessionEpoch(5)
        );
    }

    #[tokio::test]
    async fn first_qos_stream_waits_for_authenticated_context_install() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);
        client_qos
            .send_control(super::super::qos::ControlFrame::heartbeat(99, 101))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, _server_releases) = server_connection.install_qos(server_auth);
        let mut inbound = server_connection.take_peer_inbound().unwrap();
        timeout(Duration::from_secs(1), inbound.control_rx.recv())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn reliable_writer_fault_tombstones_and_emits_terminal_release() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let _server_connection = incoming.recv().await.unwrap().connection;
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (client_qos, mut releases) = client_connection.install_qos(client_auth);
        client_connection
            .inner
            .connection
            .close(0u32.into(), b"force reliable writer fault");
        let epoch = SessionEpoch(88);
        let _ = client_qos.try_send_reliable_input(ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: epoch,
            sequence: 1,
            captured_at: MonotonicStamp::new(ClockDomainId(1), 1),
            event: ReliableInputEvent::Enter {
                target_display_id: "primary".into(),
                x: 0,
                y: 0,
            },
        });
        let release = timeout(Duration::from_secs(1), releases.recv())
            .await
            .expect("writer fault must emit typed terminal release")
            .unwrap();
        assert_eq!(release.epoch, epoch);
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert!(client_qos.is_tombstoned(epoch));
    }

    #[tokio::test]
    async fn reliable_sequence_gap_closes_before_full_callback_and_does_not_drop_release() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let generation = ControlConnectionId::new();
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: generation,
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, mut releases) = server_connection.install_qos(server_auth);
        let (client_qos, _client_releases) = client_connection.install_qos(client_auth);
        let epoch = SessionEpoch(91);
        let filler = TerminalReleaseEvent {
            auth: Arc::new(PeerAuthContext {
                peer_id: client_id,
                certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
                control_connection_id: generation,
            }),
            epoch: SessionEpoch(90),
            reason: ReleaseAllReason::SessionEnded,
        };
        let release_tx = server_connection
            .inner
            .qos_receive
            .lock()
            .expect("qos receive context poisoned")
            .as_ref()
            .expect("qos receive context installed")
            .release_emitter
            .target_for_test();
        for _ in 0..8 {
            release_tx.try_send(filler.clone()).unwrap();
        }
        for (sequence, event) in [
            (
                1,
                ReliableInputEvent::Enter {
                    target_display_id: "primary".into(),
                    x: 0,
                    y: 0,
                },
            ),
            (
                3,
                ReliableInputEvent::Key {
                    keycode: 0x41,
                    state: rshare_core::KeyState::Pressed,
                },
            ),
        ] {
            client_qos
                .try_send_reliable_input(ReliableInputFrame {
                    protocol_version: INPUT_PROTOCOL_VERSION,
                    session_epoch: epoch,
                    sequence,
                    captured_at: MonotonicStamp::new(ClockDomainId(1), sequence),
                    event,
                })
                .unwrap();
        }
        timeout(
            Duration::from_secs(1),
            server_connection.inner.connection.closed(),
        )
        .await
        .expect("sequence violation must close before callback capacity is available");
        for _ in 0..8 {
            assert_eq!(releases.recv().await.unwrap().epoch, SessionEpoch(90));
        }
        let release = timeout(Duration::from_secs(1), releases.recv())
            .await
            .expect("reliable gap must fail closed")
            .unwrap();
        assert_eq!(release.epoch, epoch);
        assert_eq!(release.reason, ReleaseAllReason::BackendFailure);
        assert!(
            server_connection
                .inner
                .qos_inbound
                .lock()
                .expect("qos inbound state poisoned")
                .retired_through
                .is_some_and(|retired| retired == (generation, epoch)),
            "receiver must tombstone before invoking release"
        );
    }

    #[tokio::test]
    async fn dropping_last_qos_handle_releases_worker_state() {
        let server_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(DeviceId::new_v4());
        let connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let _remote = incoming.recv().await.unwrap().connection;
        let auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (handle, _releases) = connection.install_qos(auth);
        let probe = handle.drop_probe_for_test();
        let workers = handle.worker_probe_for_test();
        drop(handle);
        tokio::task::yield_now().await;
        assert!(
            probe.released(),
            "QoS workers must not keep their sender-owning state alive"
        );
        timeout(Duration::from_secs(1), async {
            while workers.running() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all QoS writers must stop when the last handle is dropped");
    }

    #[tokio::test]
    async fn cancelled_awaited_lane_during_preface_resets_without_closing_connection() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, _server_releases) = server_connection.install_qos(server_auth);
        let (handle, _client_releases) = client_connection.install_qos(client_auth);
        handle.set_awaited_preface_barrier_for_test(Arc::new(Notify::new()));
        let blocked_handle = handle.clone();
        let send = tokio::spawn(async move {
            blocked_handle
                .send_bulk(super::super::qos::BulkFrame::test_payload(8))
                .await
        });
        timeout(Duration::from_secs(1), async {
            while !handle.awaited_preface_waiting_for_test() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the awaited writer must open a stream and block before writing its preface");

        send.abort();
        let _ = send.await;
        timeout(Duration::from_secs(1), async {
            while handle.awaited_write_counts_for_test().3 == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("preface cancellation must reset the already-open stream with the dedicated code");
        timeout(Duration::from_secs(1), async {
            while server_connection
                .inner
                .qos_awaited_cancel_resets_observed
                .load(Ordering::Acquire)
                == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the peer must observe the dedicated preface cancellation reset");
        assert!(
            server_connection.inner.connection.close_reason().is_none(),
            "preface cancellation must not close the connection"
        );
    }

    #[tokio::test]
    async fn cancelled_blocked_awaited_lane_resets_and_all_workers_stop_on_drop() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let server_connection = incoming.recv().await.unwrap().connection;
        let bulk_reader_barrier = Arc::new(Notify::new());
        server_connection.set_qos_bulk_reader_barrier(bulk_reader_barrier.clone());
        let server_auth = Arc::new(PeerAuthContext {
            peer_id: client_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"client"),
            control_connection_id: ControlConnectionId::new(),
        });
        let client_auth = Arc::new(PeerAuthContext {
            peer_id: server_id,
            certificate_fingerprint: PeerCertificateFingerprint::from_der(b"server"),
            control_connection_id: ControlConnectionId::new(),
        });
        let (_server_qos, _server_releases) = server_connection.install_qos(server_auth);
        let (handle, _client_releases) = client_connection.install_qos(client_auth);
        let workers = handle.worker_probe_for_test();
        let blocked_handle = handle.clone();
        let send = tokio::spawn(async move {
            blocked_handle
                .send_bulk(super::super::qos::BulkFrame::test_payload(512 * 1024))
                .await
        });
        timeout(Duration::from_secs(1), async {
            loop {
                let (started, completed, _, _) = handle.awaited_write_counts_for_test();
                if started > completed && server_connection.qos_bulk_reader_waiting_for_test() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the bulk writer must enter a write blocked by the receiver barrier");
        assert!(
            !send.is_finished(),
            "the caller must still await the physically blocked bulk write"
        );

        send.abort();
        let _ = send.await;
        timeout(Duration::from_secs(1), async {
            loop {
                if handle.awaited_write_counts_for_test().2 != 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropping the caller must cancel the in-flight bulk write");
        timeout(Duration::from_secs(1), async {
            while handle.awaited_write_counts_for_test().3 == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the cancelled writer must successfully reset its stream");
        bulk_reader_barrier.notify_waiters();
        timeout(Duration::from_secs(1), async {
            while server_connection
                .inner
                .qos_awaited_cancel_resets_observed
                .load(Ordering::Acquire)
                == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the peer must observe the dedicated awaited-lane cancel reset");
        assert!(
            server_connection.inner.connection.close_reason().is_none(),
            "an awaited-lane caller cancellation must not close the connection"
        );
        drop(handle);
        timeout(Duration::from_secs(1), async {
            while workers.running() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled awaited write and every QoS worker must stop on handle drop");
    }

    #[test]
    fn retired_old_epoch_does_not_clear_new_active_epoch_and_max_sequence_fails_closed() {
        let generation = ControlConnectionId::new();
        let mut state = QosInboundState::default();
        let frame = |epoch, sequence, event| ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(epoch),
            sequence,
            captured_at: MonotonicStamp::new(ClockDomainId(1), sequence),
            event,
        };
        assert!(matches!(
            accept_reliable_state(
                &mut state,
                generation,
                &frame(
                    1,
                    1,
                    ReliableInputEvent::Enter {
                        target_display_id: "a".into(),
                        x: 0,
                        y: 0,
                    },
                ),
            ),
            ReliableAccept::Accepted
        ));
        assert!(matches!(
            accept_reliable_state(
                &mut state,
                generation,
                &frame(
                    1,
                    2,
                    ReliableInputEvent::ReleaseAll {
                        reason: ReleaseAllReason::SessionEnded,
                    },
                ),
            ),
            ReliableAccept::Accepted
        ));
        assert!(matches!(
            accept_reliable_state(
                &mut state,
                generation,
                &frame(
                    2,
                    u64::MAX,
                    ReliableInputEvent::Enter {
                        target_display_id: "b".into(),
                        x: 0,
                        y: 0,
                    },
                ),
            ),
            ReliableAccept::Accepted
        ));
        assert!(matches!(
            accept_reliable_state(
                &mut state,
                generation,
                &frame(
                    1,
                    3,
                    ReliableInputEvent::Key {
                        keycode: 0x41,
                        state: rshare_core::KeyState::Pressed,
                    },
                ),
            ),
            ReliableAccept::RetiredEpoch
        ));
        assert!(state
            .active
            .is_some_and(|(_, epoch, sequence)| epoch == SessionEpoch(2) && sequence == u64::MAX));
        assert!(matches!(
            accept_reliable_state(
                &mut state,
                generation,
                &frame(
                    2,
                    u64::MAX,
                    ReliableInputEvent::Key {
                        keycode: 0x42,
                        state: rshare_core::KeyState::Pressed,
                    },
                ),
            ),
            ReliableAccept::CurrentViolation {
                epoch: SessionEpoch(2)
            }
        ));
    }

    #[test]
    fn late_frame_after_terminal_epoch_is_a_violation_but_superseded_epoch_is_ignored() {
        let generation = ControlConnectionId::new();
        let frame = |epoch, sequence, event| ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(epoch),
            sequence,
            captured_at: MonotonicStamp::new(ClockDomainId(1), sequence),
            event,
        };
        let enter = |epoch| {
            frame(
                epoch,
                1,
                ReliableInputEvent::Enter {
                    target_display_id: format!("display-{epoch}"),
                    x: 0,
                    y: 0,
                },
            )
        };

        let mut terminal = QosInboundState::default();
        assert!(matches!(
            accept_reliable_state(&mut terminal, generation, &enter(1)),
            ReliableAccept::Accepted
        ));
        assert!(matches!(
            accept_reliable_state(
                &mut terminal,
                generation,
                &frame(
                    1,
                    2,
                    ReliableInputEvent::ReleaseAll {
                        reason: ReleaseAllReason::SessionEnded,
                    },
                ),
            ),
            ReliableAccept::Accepted
        ));
        assert!(matches!(
            accept_reliable_state(&mut terminal, generation, &enter(1)),
            ReliableAccept::CurrentViolation {
                epoch: SessionEpoch(1)
            }
        ));

        let mut superseded = QosInboundState::default();
        assert!(matches!(
            accept_reliable_state(&mut superseded, generation, &enter(1)),
            ReliableAccept::Accepted
        ));
        assert!(matches!(
            accept_reliable_state(&mut superseded, generation, &enter(2)),
            ReliableAccept::EpochAdvanced {
                retired: SessionEpoch(1)
            }
        ));
        assert!(matches!(
            accept_reliable_state(&mut superseded, generation, &enter(1)),
            ReliableAccept::RetiredEpoch
        ));
        assert_eq!(superseded.active, Some((generation, SessionEpoch(2), 1)));
    }

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
        let mut server = QuicTransport::isolated_for_test(server_id);
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
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.require_peer_protocol_handshake();
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::isolated_for_test(client_id);
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
        let mut manager = crate::connection::ConnectionManager::isolated_for_test(server_id);
        let mut events = manager.events().unwrap();
        manager.start_server("127.0.0.1:0").await.unwrap();
        let address = manager.transport_local_addr().unwrap();

        let mut client = QuicTransport::isolated_for_test(client_id);
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
                    crate::connection::ManagerEvent::Connected(ref auth)
                        if auth.peer_id == client_id
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
        let server_transport = QuicTransport::isolated_for_test(server_id)
            .with_test_datagram_reader_barrier(reader_barrier.clone());
        let mut manager =
            crate::connection::ConnectionManager::with_transport(server_id, server_transport);
        let mut events = manager.events().unwrap();
        manager.start_server("127.0.0.1:0").await.unwrap();
        let address = manager.transport_local_addr().unwrap();

        let mut client = QuicTransport::isolated_for_test(client_id);
        let mut client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let datagram = encode_legacy_realtime_message(&Message::MouseMove { x: 13, y: 17 })
            .unwrap()
            .unwrap();
        client_connection
            .inner
            .connection
            .send_datagram(datagram)
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
                    crate::connection::ManagerEvent::Connected(ref auth)
                        if auth.peer_id == client_id
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
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.require_peer_protocol_handshake();
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::isolated_for_test(client_id);
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
        Encryption::reset_default_identity_loads_for_test();
        let first = QuicTransport::isolated_for_test(DeviceId::new_v4());
        let second = QuicTransport::isolated_for_test(DeviceId::new_v4());
        assert!(!first.is_running());
        assert_eq!(Encryption::default_identity_loads_for_test(), 0);
        assert_ne!(first.identity.cert_der, second.identity.cert_der);
        assert_ne!(first.trust_store_path, second.trust_store_path);
        for path in [&first.trust_store_path, &second.trust_store_path] {
            let path = path.as_ref().expect("test trust path must be explicit");
            assert!(
                path.starts_with(
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("target")
                        .join("rshare-state")
                ),
                "test trust state must stay inside the per-crate scratch tree"
            );
        }
    }

    #[tokio::test]
    async fn repeated_start_is_rejected_without_replacing_the_first_listener() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        let server_state = server.state_lifetime.as_ref().unwrap().state_dir.clone();
        server.start_server("127.0.0.1:0").await.unwrap();
        let first_addr = server.local_addr().unwrap();
        let first_task_id = server.server_task.as_ref().unwrap().id();
        let mut incoming = server.incoming();

        let error = server
            .start_server("127.0.0.1:0")
            .await
            .expect_err("a registered server must reject a second start");
        assert!(matches!(
            error.downcast_ref::<QuicServerStartError>(),
            Some(QuicServerStartError::AlreadyRunning)
        ));
        assert_eq!(
            error.to_string(),
            "QUIC transport server is already registered; call close before starting it again"
        );
        assert_eq!(server.local_addr(), Some(first_addr));
        assert!(server.is_running());
        assert_eq!(server.server_task.as_ref().unwrap().id(), first_task_id);

        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_state = client.state_lifetime.as_ref().unwrap().state_dir.clone();
        let mut client_connection = client
            .connect(&first_addr.to_string(), server_id)
            .await
            .expect("the original listener must still complete a real QUIC handshake");
        let mut server_connection = timeout(Duration::from_secs(1), incoming.recv())
            .await
            .expect("the original listener must accept within the deadline")
            .expect("the original listener must publish its connection")
            .connection;
        client_connection.confirm_peer_identity(server_id).unwrap();
        server_connection
            .confirm_inbound_peer_identity(client_id)
            .unwrap();
        assert!(
            server_state.exists(),
            "the original listener connection must retain server state"
        );
        assert!(
            client_state.exists(),
            "the established client connection must retain client state"
        );

        client_connection
            .inner
            .connection
            .close(0u32.into(), b"repeated start lifetime test complete");
        server_connection
            .inner
            .connection
            .close(0u32.into(), b"repeated start lifetime test complete");
        drop(client_connection);
        drop(server_connection);
        server.close().await.unwrap();
        drop(server);
        drop(client);
        drop(incoming);
        timeout(Duration::from_secs(1), async {
            while server_state.exists() || client_state.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all connection and listener owners must release isolated state");
    }

    #[tokio::test]
    async fn finished_server_task_still_requires_explicit_close_before_restart() {
        let mut server = QuicTransport::isolated_for_test(DeviceId::new_v4());
        let server_state = server.state_lifetime.as_ref().unwrap().state_dir.clone();
        fs::create_dir_all(&server_state).unwrap();
        server.start_server("127.0.0.1:0").await.unwrap();
        let first_addr = server.local_addr().unwrap();
        let first_task_id = server.server_task.as_ref().unwrap().id();
        server
            .server_endpoint
            .as_ref()
            .expect("the first endpoint must be registered")
            .close(0u32.into(), b"finish accept loop for restart test");
        timeout(Duration::from_secs(1), async {
            while server.is_running() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("closing the endpoint must finish its accept loop");

        let error = server
            .start_server("not-a-socket-address")
            .await
            .expect_err("registered fields must be rejected before parsing a new bind address");
        assert!(matches!(
            error.downcast_ref::<QuicServerStartError>(),
            Some(QuicServerStartError::AlreadyRunning)
        ));
        assert_eq!(
            error.to_string(),
            "QUIC transport server is already registered; call close before starting it again"
        );
        assert_eq!(server.local_addr(), Some(first_addr));
        assert!(!server.is_running());
        assert_eq!(server.server_task.as_ref().unwrap().id(), first_task_id);

        server.close().await.unwrap();
        assert_eq!(server.local_addr(), None);
        assert!(!server.is_running());
        server
            .start_server("127.0.0.1:0")
            .await
            .expect("explicit close must permit a clean restart");
        assert!(server.is_running());
        server.close().await.unwrap();
        drop(server);
        timeout(Duration::from_secs(1), async {
            while server_state.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("explicit close must release the finished listener state owner");
    }

    #[test]
    fn parallel_isolated_transports_never_share_state_and_cleanup() {
        let workers = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    let transport = QuicTransport::isolated_for_test(DeviceId::new_v4());
                    let trust_path = transport
                        .trust_store_path
                        .clone()
                        .expect("isolated transport must have an explicit trust path");
                    let state_dir = trust_path
                        .parent()
                        .expect("trust path must have a state directory")
                        .to_path_buf();
                    fs::create_dir_all(&state_dir).unwrap();
                    fs::write(
                        state_dir.join("owner"),
                        transport.identity.cert_der.as_slice(),
                    )
                    .unwrap();
                    (transport, trust_path, state_dir)
                })
            })
            .collect::<Vec<_>>();
        let isolated = workers
            .into_iter()
            .map(|worker| worker.join().expect("isolated transport worker panicked"))
            .collect::<Vec<_>>();

        let trust_paths = isolated
            .iter()
            .map(|(_, trust_path, _)| trust_path.clone())
            .collect::<std::collections::HashSet<_>>();
        let identities = isolated
            .iter()
            .map(|(transport, _, _)| transport.identity.cert_der.clone())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(trust_paths.len(), isolated.len());
        assert_eq!(identities.len(), isolated.len());
        assert!(isolated.iter().all(|(_, _, state_dir)| state_dir.exists()));

        let state_dirs = isolated
            .iter()
            .map(|(_, _, state_dir)| state_dir.clone())
            .collect::<Vec<_>>();
        drop(isolated);
        assert!(
            state_dirs.iter().all(|state_dir| !state_dir.exists()),
            "each lifetime owner must clean only its own test state"
        );
    }

    #[tokio::test]
    async fn established_connections_keep_isolated_state_after_transports_drop() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        let server_state = server.state_lifetime.as_ref().unwrap().state_dir.clone();
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_state = client.state_lifetime.as_ref().unwrap().state_dir.clone();
        let mut client_connection = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let mut server_connection = incoming.recv().await.unwrap().connection;
        fs::create_dir_all(&server_state).unwrap();
        fs::create_dir_all(&client_state).unwrap();

        drop(server);
        drop(client);
        assert!(server_state.exists());
        assert!(client_state.exists());
        client_connection.confirm_peer_identity(server_id).unwrap();
        server_connection
            .confirm_inbound_peer_identity(client_id)
            .unwrap();

        client_connection
            .inner
            .connection
            .close(0u32.into(), b"lifetime test complete");
        server_connection
            .inner
            .connection
            .close(0u32.into(), b"lifetime test complete");
        drop(client_connection);
        drop(server_connection);
        timeout(Duration::from_secs(1), async {
            while server_state.exists() || client_state.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the final connection/task owner must clean both state directories");
    }

    #[tokio::test]
    async fn pending_peer_trust_keeps_state_until_confirmed_after_transport_drop() {
        let server_id = DeviceId::new_v4();
        let client_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(client_id);
        let client_state = client.state_lifetime.as_ref().unwrap().state_dir.clone();
        let mut pending = client
            .connect(&address.to_string(), server_id)
            .await
            .unwrap();
        let remote = incoming.recv().await.unwrap().connection;
        fs::create_dir_all(&client_state).unwrap();

        drop(client);
        assert!(client_state.exists());
        pending.confirm_peer_identity(server_id).unwrap();
        assert!(
            pending.trust_store_path.exists(),
            "confirmation after transport drop must still persist trust"
        );

        pending
            .inner
            .connection
            .close(0u32.into(), b"pending trust lifetime test complete");
        remote
            .inner
            .connection
            .close(0u32.into(), b"pending trust lifetime test complete");
        drop(pending);
        drop(remote);
        server.close().await.unwrap();
        drop(server);
        timeout(Duration::from_secs(1), async {
            while client_state.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the confirmed pending trust owner must clean its state");
    }

    #[tokio::test]
    async fn detached_accept_task_keeps_state_until_it_finishes_after_transport_drop() {
        let server_id = DeviceId::new_v4();
        let barrier = Arc::new(Notify::new());
        let mut server = QuicTransport::isolated_for_test(server_id)
            .with_test_accept_task_barrier(barrier.clone());
        let server_state = server.state_lifetime.as_ref().unwrap().state_dir.clone();
        fs::create_dir_all(&server_state).unwrap();
        server.start_server("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let connect = tokio::spawn(async move {
            let mut client = QuicTransport::isolated_for_test(DeviceId::new_v4());
            client.connect(&address.to_string(), server_id).await
        });
        timeout(Duration::from_secs(1), async {
            while !server.accept_task_waiting_for_test() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the detached accept task must reach its barrier");

        drop(server);
        assert!(
            server_state.exists(),
            "the detached accept task must retain isolated state"
        );
        barrier.notify_one();
        let client_connection = connect
            .await
            .expect("client connect task panicked")
            .expect("the per-handshake task must finish the accepted connection");
        let server_connection = incoming
            .recv()
            .await
            .expect("the accepted connection must be published")
            .connection;
        client_connection
            .inner
            .connection
            .close(0u32.into(), b"accept task lifetime test complete");
        server_connection
            .inner
            .connection
            .close(0u32.into(), b"accept task lifetime test complete");
        drop(client_connection);
        drop(server_connection);
        drop(incoming);
        timeout(Duration::from_secs(1), async {
            while server_state.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("state must be cleaned after the detached accept task exits");
    }

    #[test]
    fn isolated_state_drop_is_best_effort_when_external_state_changes() {
        let deleted = QuicTransport::isolated_for_test(DeviceId::new_v4());
        let deleted_state = deleted.state_lifetime.as_ref().unwrap().state_dir.clone();
        fs::create_dir_all(&deleted_state).unwrap();
        fs::remove_dir_all(&deleted_state).unwrap();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(deleted))).is_ok());

        let replaced = QuicTransport::isolated_for_test(DeviceId::new_v4());
        let replaced_state = replaced.state_lifetime.as_ref().unwrap().state_dir.clone();
        fs::create_dir_all(
            replaced_state
                .parent()
                .expect("isolated state must have a scratch parent"),
        )
        .unwrap();
        fs::write(&replaced_state, b"external replacement").unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(replaced))).is_ok(),
            "cleanup failure during unwinding must never panic"
        );
        fs::remove_file(replaced_state).unwrap();

        let unwinding = QuicTransport::isolated_for_test(DeviceId::new_v4());
        let unwinding_state = unwinding.state_lifetime.as_ref().unwrap().state_dir.clone();
        fs::create_dir_all(unwinding_state.parent().unwrap()).unwrap();
        fs::write(&unwinding_state, b"external replacement during unwind").unwrap();
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _transport = unwinding;
            panic!("simulated test panic");
        }));
        assert!(unwind.is_err(), "the simulated panic must be caught");
        fs::remove_file(unwinding_state).unwrap();

        let unsafe_state = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("outside-rshare-state-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&unsafe_state).unwrap();
        let unsafe_owner = StateLifetimeOwner {
            state_dir: unsafe_state.clone(),
            scratch_root: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("rshare-state"),
        };
        drop(unsafe_owner);
        assert!(
            unsafe_state.exists(),
            "cleanup must reject paths outside the isolated scratch root"
        );
        fs::remove_dir_all(unsafe_state).unwrap();

        let scratch_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("rshare-state");
        let non_isolated_state = scratch_root.join("not-an-isolated-uuid");
        fs::create_dir_all(&non_isolated_state).unwrap();
        drop(StateLifetimeOwner {
            state_dir: non_isolated_state.clone(),
            scratch_root,
        });
        assert!(
            non_isolated_state.exists(),
            "cleanup must reject non-UUID paths inside the scratch root"
        );
        fs::remove_dir_all(non_isolated_state).unwrap();
    }

    #[test]
    fn explicit_identity_constructor_does_not_load_default_state() {
        Encryption::reset_default_identity_loads_for_test();
        let _transport =
            QuicTransport::with_identity(DeviceId::new_v4(), generated_test_identity());
        assert_eq!(Encryption::default_identity_loads_for_test(), 0);
    }

    #[tokio::test]
    async fn start_server_marks_transport_running() {
        let mut transport = QuicTransport::isolated_for_test(DeviceId::new_v4());

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
        let mut server = QuicTransport::isolated_for_test(local_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let mut incoming = server.incoming();
        let mut client = QuicTransport::isolated_for_test(remote_id);
        let sender = client
            .connect(&server_addr.to_string(), local_id)
            .await
            .unwrap();
        let receiver = incoming.recv().await.unwrap().connection;
        let pool = ConnectionPool::new(remote_id);
        pool.insert(local_id, sender).await;

        let (blocked_tx, mut blocked_rx) = mpsc::channel(1);
        {
            let mut connections = pool.connections.lock().unwrap();
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

        assert!(
            pool.connections.try_lock().is_ok(),
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
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut fast_client = QuicTransport::isolated_for_test(fast_id);
        let fast_sender = fast_client
            .connect(&server_addr.to_string(), server_id)
            .await
            .unwrap();
        let mut fast_receiver = incoming.recv().await.unwrap().connection;
        let mut fast_messages = fast_receiver.message_channel();

        let mut slow_client = QuicTransport::isolated_for_test(slow_id);
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
            let mut connections = pool.connections.lock().unwrap();
            connections.get_mut(&slow_id).unwrap().outbound = OutboundSender {
                send_channel: blocked_tx,
            };
        }

        let broadcast_pool = pool.clone();
        let broadcast_task = tokio::spawn(async move {
            broadcast_pool
                .broadcast(&Message::Heartbeat {
                    sequence: 7,
                    timestamp: 11,
                })
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
                Some(Message::Heartbeat {
                    sequence: 7,
                    timestamp: 11
                })
            ),
            "fast peer must receive the broadcast payload"
        );
        assert!(
            pool.connections.try_lock().is_ok(),
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
        let mut server = QuicTransport::isolated_for_test(server_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let mut incoming = server.incoming();

        let mut fast_client = QuicTransport::isolated_for_test(fast_id);
        let fast_sender = fast_client
            .connect(&server_addr.to_string(), server_id)
            .await
            .unwrap();
        let mut fast_receiver = incoming.recv().await.unwrap().connection;
        let mut fast_messages = fast_receiver.message_channel();

        let mut slow_client = QuicTransport::isolated_for_test(slow_id);
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
            let mut connections = pool.connections.lock().unwrap();
            connections.get_mut(&slow_id).unwrap().outbound = OutboundSender {
                send_channel: blocked_tx,
            };
        }

        let first = pool
            .try_fanout(&Message::Heartbeat {
                sequence: 1,
                timestamp: 0,
            })
            .await;
        assert!(first.failures.is_empty());
        let mut expected_order = vec![fast_id, slow_id];
        expected_order.sort_by_key(DeviceId::to_string);
        assert_eq!(first.enqueued, expected_order);
        let blocked_first = blocked_rx
            .recv()
            .await
            .expect("slow peer must retain the first frame without acknowledging it");

        let second = pool
            .try_fanout(&Message::Heartbeat {
                sequence: 2,
                timestamp: 0,
            })
            .await;
        assert!(second.failures.is_empty());
        assert_eq!(second.enqueued, expected_order);
        for expected_sequence in [1, 2] {
            assert!(matches!(
                timeout(Duration::from_millis(100), fast_messages.recv())
                    .await
                    .expect("fast peer must receive consecutive fanout frames"),
                Some(Message::Heartbeat {
                    sequence,
                    timestamp: 0
                }) if sequence == expected_sequence
            ));
        }
        assert!(
            pool.connections.try_lock().is_ok(),
            "nonblocking fanout must release the pool lock before peer queue work"
        );

        let third = pool
            .try_fanout(&Message::Heartbeat {
                sequence: 3,
                timestamp: 0,
            })
            .await;
        assert_eq!(third.enqueued, vec![fast_id]);
        assert_eq!(third.failures.len(), 1);
        assert_eq!(third.failures[0].device_id, slow_id);
        assert_eq!(third.failures[0].kind, FanoutEnqueueFailureKind::QueueFull);
        assert!(matches!(
            timeout(Duration::from_millis(100), fast_messages.recv())
                .await
                .expect("fast peer must continue after only the slow queue overflows"),
            Some(Message::Heartbeat {
                sequence: 3,
                timestamp: 0
            })
        ));

        drop(blocked_first);
        drop(slow_receiver);
        fast_receiver.close().await;
        server.close().await.unwrap();
    }

    #[tokio::test]
    async fn legacy_realtime_datagram_never_enters_message_fifo_without_qos_context() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(local_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server
            .server_endpoint
            .as_ref()
            .unwrap()
            .local_addr()
            .unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::isolated_for_test(remote_id);
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

        assert!(
            tokio::time::timeout(Duration::from_millis(100), messages.recv())
                .await
                .is_err(),
            "legacy realtime input must never enter the general Message FIFO"
        );
    }

    #[tokio::test]
    async fn realtime_datagram_send_failure_is_counted_and_never_falls_back() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(local_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server
            .server_endpoint
            .as_ref()
            .unwrap()
            .local_addr()
            .unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::isolated_for_test(remote_id);
        let sender = client
            .connect(&server_addr.to_string(), local_id)
            .await
            .unwrap();
        let receiver = incoming.recv().await.unwrap().connection;
        let dropped_before = sender.datagram_tx_dropped();
        let reliable_resets_before = sender.reliable_stream_reset_count();

        receiver.close().await;
        timeout(Duration::from_secs(1), sender.inner.connection.closed())
            .await
            .expect("sender must observe the peer closing");

        sender
            .send_message(&Message::MouseMove { x: 1, y: 2 })
            .await
            .expect("realtime congestion is a counted drop, not a reliable send error");

        assert_eq!(sender.datagram_tx_dropped(), dropped_before + 1);
        assert_eq!(
            sender.reliable_stream_reset_count(),
            reliable_resets_before,
            "a realtime datagram failure must never touch the reliable stream"
        );
    }

    #[tokio::test]
    async fn oversized_legacy_realtime_encoding_is_counted_and_dropped() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(local_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server
            .server_endpoint
            .as_ref()
            .unwrap()
            .local_addr()
            .unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::isolated_for_test(remote_id);
        let sender = client
            .connect(&server_addr.to_string(), local_id)
            .await
            .unwrap();
        let _receiver = incoming.recv().await.unwrap().connection;
        let dropped_before = sender.datagram_tx_dropped();
        let mut state = rshare_core::GamepadState::neutral(0, 1, 0);
        state.buttons = vec![
            rshare_core::GamepadButtonState {
                button: rshare_core::GamepadButton::South,
                pressed: true,
            };
            400_000
        ];
        let message = Message::GamepadState { state };
        assert!(
            encode_legacy_realtime_message(&message).is_err(),
            "fixture must exceed the temporary legacy codec bound"
        );

        sender
            .send_message(&message)
            .await
            .expect("oversized realtime encoding must be treated as a counted drop");

        assert_eq!(sender.datagram_tx_dropped(), dropped_before + 1);
    }

    #[tokio::test]
    async fn quinn_loopback_sends_key_over_reliable_stream() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut server = QuicTransport::isolated_for_test(local_id);
        server.start_server("127.0.0.1:0").await.unwrap();
        let server_addr = server
            .server_endpoint
            .as_ref()
            .unwrap()
            .local_addr()
            .unwrap();
        let mut incoming = server.incoming();

        let mut client = QuicTransport::isolated_for_test(remote_id);
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
