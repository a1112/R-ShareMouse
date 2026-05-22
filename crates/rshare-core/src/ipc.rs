//! Local IPC protocol for daemon control and status queries.

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{
    BackendHealth, BackendKind, BackgroundProcessOwner, BackgroundRunMode,
    CapabilityRegistrySnapshot, ControlSessionState, DeviceId, EndpointEvent, EndpointEventFilter,
    EndpointInjectRequest, EndpointInjectResult, EndpointInjectTarget, LayoutGraph,
    LocalAudioCaptureSource, LocalAudioTestRequest, LocalAudioTestResult,
    LocalControlDeviceSnapshot, LocalInputDiagnosticEvent, LocalInputTestRequest,
    LocalInputTestResult, PrivilegeState, ResolvedInputMode, TrayRuntimeState, UsbDeviceDescriptor,
};

/// Default TCP port for localhost daemon IPC.
pub const DEFAULT_IPC_PORT: u16 = 27435;
pub const DEFAULT_LOCAL_CONTROLS_WS_PORT: u16 = 27436;

/// Current daemon status snapshot returned to local clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceStatusSnapshot {
    pub device_id: DeviceId,
    pub device_name: String,
    pub hostname: String,
    pub bind_address: String,
    pub discovery_port: u16,
    pub pid: u32,
    pub discovered_devices: usize,
    pub connected_devices: usize,
    pub healthy: bool,

    // Input backend status fields
    /// The resolved input mode currently in use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mode: Option<ResolvedInputMode>,
    /// Available input backends on this system.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_backends: Option<Vec<BackendKind>>,
    /// Health status of the current backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_health: Option<BackendHealth>,
    /// Current privilege/session state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privilege_state: Option<PrivilegeState>,
    /// Last backend error message (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_backend_error: Option<String>,

    // Alpha-2 session state fields
    /// Current control session state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_state: Option<ControlSessionState>,
    /// Active control target (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_target: Option<DeviceId>,

    /// Process that owns the background service lifecycle.
    #[serde(default = "default_background_owner")]
    pub background_owner: BackgroundProcessOwner,
    /// Current daemon run mode.
    #[serde(default = "default_background_mode")]
    pub background_mode: BackgroundRunMode,
    /// Process that owns tray integration.
    #[serde(default = "default_background_owner")]
    pub tray_owner: BackgroundProcessOwner,
    /// Current tray runtime state.
    #[serde(default = "default_tray_state")]
    pub tray_state: TrayRuntimeState,
    /// True when this snapshot was returned after desktop auto-started the daemon.
    #[serde(default)]
    pub started_by_desktop: bool,

    /// Device transport diagnostics for peer-to-peer control traffic.
    #[serde(default)]
    pub network: NetworkTransportSnapshot,
    /// Daemon-owned latency feedback for local input, remote probes, and transport health.
    #[serde(default)]
    pub latency_feedback: LatencyFeedbackSnapshot,
}

/// Runtime diagnostics for the device-to-device transport.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkTransportSnapshot {
    /// Transport implementation currently used for peer traffic.
    pub transport: String,
    /// Whether QUIC datagrams are available for realtime input.
    pub datagram_available: bool,
    /// True when realtime datagrams are unavailable and input falls back to reliable streams.
    pub realtime_degraded: bool,
    /// Smoothed RTT for the active connection set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<u64>,
    /// Milliseconds since the last realtime datagram was received.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_datagram_rx_ms: Option<u64>,
    /// Count of realtime datagrams dropped before fallback.
    pub datagram_tx_dropped: u64,
    /// Count of reliable stream resets/reopens observed by the runtime.
    pub reliable_stream_reset_count: u64,
    /// Current certificate trust state for active peer connections.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_trust_state: Option<String>,
}

impl Default for NetworkTransportSnapshot {
    fn default() -> Self {
        Self {
            transport: "quic".to_string(),
            datagram_available: false,
            realtime_degraded: true,
            rtt_ms: None,
            last_datagram_rx_ms: None,
            datagram_tx_dropped: 0,
            reliable_stream_reset_count: 0,
            cert_trust_state: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LatencyFeedbackStatus {
    Idle,
    Pending,
    Healthy,
    Degraded,
    Timeout,
    Unavailable,
}

impl Default for LatencyFeedbackStatus {
    fn default() -> Self {
        Self::Unavailable
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LatencyFeedbackSnapshot {
    #[serde(default)]
    pub generated_at_ms: u64,
    #[serde(default)]
    pub local_input: LocalInputFeedback,
    #[serde(default)]
    pub remote_latency: RemoteLatencyFeedback,
    #[serde(default)]
    pub transport: TransportFeedback,
}

impl Default for LatencyFeedbackSnapshot {
    fn default() -> Self {
        Self {
            generated_at_ms: 0,
            local_input: LocalInputFeedback::default(),
            remote_latency: RemoteLatencyFeedback::default(),
            transport: TransportFeedback::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalInputFeedback {
    #[serde(default)]
    pub status: LatencyFeedbackStatus,
    #[serde(default)]
    pub event_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_event_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_keyboard_event_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_mouse_event_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_path: Option<String>,
}

impl Default for LocalInputFeedback {
    fn default() -> Self {
        Self {
            status: LatencyFeedbackStatus::Unavailable,
            event_count: 0,
            latest_sequence: None,
            latest_event_ms: None,
            latest_keyboard_event_ms: None,
            latest_mouse_event_ms: None,
            capture_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteLatencyFeedback {
    #[serde(default)]
    pub status: LatencyFeedbackStatus,
    #[serde(default)]
    pub devices: Vec<RemoteDeviceLatencyFeedback>,
}

impl Default for RemoteLatencyFeedback {
    fn default() -> Self {
        Self {
            status: LatencyFeedbackStatus::Unavailable,
            devices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteDeviceLatencyFeedback {
    pub device_id: DeviceId,
    #[serde(default)]
    pub status: LatencyFeedbackStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probe_sent_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ack_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_round_trip_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_round_trip_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_one_way_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_processing_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportFeedback {
    #[serde(default)]
    pub status: LatencyFeedbackStatus,
    #[serde(default = "default_transport_name")]
    pub transport: String,
    #[serde(default)]
    pub datagram_available: bool,
    #[serde(default = "default_realtime_degraded")]
    pub realtime_degraded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_datagram_rx_ms: Option<u64>,
    #[serde(default)]
    pub datagram_tx_dropped: u64,
    #[serde(default)]
    pub reliable_stream_reset_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_trust_state: Option<String>,
}

impl Default for TransportFeedback {
    fn default() -> Self {
        Self {
            status: LatencyFeedbackStatus::Unavailable,
            transport: default_transport_name(),
            datagram_available: false,
            realtime_degraded: true,
            rtt_ms: None,
            last_datagram_rx_ms: None,
            datagram_tx_dropped: 0,
            reliable_stream_reset_count: 0,
            cert_trust_state: None,
        }
    }
}

fn default_transport_name() -> String {
    "quic".to_string()
}

fn default_realtime_degraded() -> bool {
    true
}

fn default_background_owner() -> BackgroundProcessOwner {
    BackgroundProcessOwner::Daemon
}

fn default_background_mode() -> BackgroundRunMode {
    BackgroundRunMode::BackgroundProcess
}

fn default_tray_state() -> TrayRuntimeState {
    TrayRuntimeState::Unavailable
}

impl ServiceStatusSnapshot {
    /// Create a baseline healthy status snapshot.
    pub fn new(
        device_id: DeviceId,
        device_name: String,
        hostname: String,
        bind_address: String,
        discovery_port: u16,
        pid: u32,
    ) -> Self {
        Self {
            device_id,
            device_name,
            hostname,
            bind_address,
            discovery_port,
            pid,
            discovered_devices: 0,
            connected_devices: 0,
            healthy: true,
            input_mode: None,
            available_backends: None,
            backend_health: None,
            privilege_state: None,
            last_backend_error: None,
            session_state: None,
            active_target: None,
            background_owner: default_background_owner(),
            background_mode: default_background_mode(),
            tray_owner: default_background_owner(),
            tray_state: default_tray_state(),
            started_by_desktop: false,
            network: NetworkTransportSnapshot::default(),
            latency_feedback: LatencyFeedbackSnapshot::default(),
        }
    }
}

/// Lightweight device snapshot returned by daemon queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonDeviceSnapshot {
    pub id: DeviceId,
    pub name: String,
    pub hostname: String,
    pub addresses: Vec<String>,
    pub connected: bool,
    pub last_seen_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum UsbDescriptorProbeStatus {
    Success,
    DeviceUnavailable,
    ClaimRejected,
    TransferFailed,
    Timeout,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsbDescriptorProbeResult {
    pub status: UsbDescriptorProbeStatus,
    pub message: String,
    pub device_id: DeviceId,
    pub bus_id: String,
    pub request_id: u64,
    pub transfer_id: u64,
    #[serde(default)]
    pub session_id: Option<DeviceId>,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    #[serde(default)]
    pub actual_length: Option<u32>,
    #[serde(default)]
    pub descriptor: Option<UsbDeviceDescriptor>,
    #[serde(default)]
    pub descriptor_bytes: Vec<u8>,
}

/// Client request over localhost IPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DaemonRequest {
    Status,
    Devices,
    Capabilities {
        #[serde(default)]
        device_id: Option<DeviceId>,
    },
    Connect {
        device_id: DeviceId,
    },
    Disconnect {
        device_id: DeviceId,
    },
    GetLayout,
    SetLayout {
        layout: LayoutGraph,
    },
    ListUsbDevices,
    LocalControls,
    SubscribeLocalControls,
    EndpointEvents {
        #[serde(default)]
        filter: EndpointEventFilter,
        #[serde(default)]
        after_sequence: Option<u64>,
        #[serde(default)]
        limit: Option<u16>,
    },
    SubscribeEndpointEvents {
        #[serde(default)]
        filter: EndpointEventFilter,
    },
    InjectEndpointEvent {
        target: EndpointInjectTarget,
        request: EndpointInjectRequest,
    },
    RunLocalInputTest {
        test: LocalInputTestRequest,
    },
    RunRemoteLatencyTest {
        device_id: DeviceId,
    },
    RunRemoteUsbDescriptorProbe {
        device_id: DeviceId,
        bus_id: String,
    },
    SetAudioDefaultOutput {
        endpoint_id: String,
    },
    SetAudioOutputVolume {
        endpoint_id: String,
        volume_percent: u8,
    },
    SetAudioOutputMute {
        endpoint_id: String,
        muted: bool,
    },
    StartAudioCapture {
        source: LocalAudioCaptureSource,
        endpoint_id: Option<String>,
    },
    StopAudioCapture,
    StartAudioForwarding {
        source: LocalAudioCaptureSource,
        endpoint_id: Option<String>,
    },
    StopAudioForwarding,
    RunAudioTest {
        test: LocalAudioTestRequest,
    },
    Shutdown,
}

/// Daemon response over localhost IPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DaemonResponse {
    Status(ServiceStatusSnapshot),
    Devices(Vec<DaemonDeviceSnapshot>),
    Capabilities(CapabilityRegistrySnapshot),
    UsbDevices(Vec<UsbDeviceDescriptor>),
    Layout(LayoutGraph),
    LocalControls(LocalControlDeviceSnapshot),
    LocalControlEvent(LocalInputDiagnosticEvent),
    EndpointEvents(Vec<EndpointEvent>),
    EndpointEvent(EndpointEvent),
    EndpointInjectResult(EndpointInjectResult),
    LocalInputTest(LocalInputTestResult),
    LocalAudioTest(LocalAudioTestResult),
    UsbDescriptorProbe(UsbDescriptorProbeResult),
    Ack,
    Error(String),
}

/// Get the default localhost IPC socket address.
pub fn default_ipc_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_IPC_PORT)
}

pub fn default_local_controls_ws_addr() -> SocketAddr {
    SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        DEFAULT_LOCAL_CONTROLS_WS_PORT,
    )
}

pub fn default_local_controls_ws_url() -> String {
    format!("ws://{}/local-controls", default_local_controls_ws_addr())
}

/// Read a single newline-delimited JSON value from a stream.
pub async fn read_json_line<T, R>(reader: &mut R) -> Result<T>
where
    T: DeserializeOwned,
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let read = reader
            .read(&mut byte)
            .await
            .context("Failed to read IPC stream")?;
        if read == 0 {
            break;
        }

        if byte[0] == b'\n' {
            break;
        }

        buf.push(byte[0]);
    }

    if buf.is_empty() {
        anyhow::bail!("IPC stream closed before receiving a JSON line");
    }

    serde_json::from_slice(&buf).context("Failed to decode IPC JSON line")
}

/// Write a single newline-delimited JSON value to a stream.
pub async fn write_json_line<T, W>(writer: &mut W, value: &T) -> Result<()>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    let mut payload = serde_json::to_vec(value).context("Failed to encode IPC JSON line")?;
    payload.push(b'\n');
    writer
        .write_all(&payload)
        .await
        .context("Failed to write IPC JSON line")?;
    writer
        .flush()
        .await
        .context("Failed to flush IPC JSON line")?;
    Ok(())
}
