//! Local IPC protocol for daemon control and status queries.

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{
    BackendHealth, BackendKind, DeviceId, PrivilegeState, ResolvedInputMode,
};

/// Default TCP port for localhost daemon IPC.
pub const DEFAULT_IPC_PORT: u16 = 27435;

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

/// Client request over localhost IPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DaemonRequest {
    Status,
    Devices,
    Connect { device_id: DeviceId },
    Disconnect { device_id: DeviceId },
    Shutdown,
}

/// Daemon response over localhost IPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DaemonResponse {
    Status(ServiceStatusSnapshot),
    Devices(Vec<DaemonDeviceSnapshot>),
    Ack,
    Error(String),
}

/// Get the default localhost IPC socket address.
pub fn default_ipc_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_IPC_PORT)
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
        let read = reader.read(&mut byte).await.context("Failed to read IPC stream")?;
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
    writer.flush().await.context("Failed to flush IPC JSON line")?;
    Ok(())
}
