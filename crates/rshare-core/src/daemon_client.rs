//! Local daemon control helpers shared by CLI and GUI.

use anyhow::{Context, Result};
use futures_util::StreamExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async, tungstenite::Message as WsMessage, MaybeTlsStream, WebSocketStream,
};

use crate::{
    default_ipc_addr, default_local_controls_ws_url, read_json_line, write_json_line,
    DaemonDeviceSnapshot, DaemonRequest, DaemonResponse, LayoutGraph, LocalControlDeviceSnapshot,
    LocalInputTestRequest, LocalInputTestResult, ServiceStatusSnapshot,
};

async fn send_request(request: DaemonRequest) -> Result<DaemonResponse> {
    let mut stream = TcpStream::connect(default_ipc_addr())
        .await
        .with_context(|| format!("Failed to connect to daemon at {}", default_ipc_addr()))?;

    write_json_line(&mut stream, &request).await?;
    read_json_line(&mut stream).await
}

pub async fn request_status() -> Result<ServiceStatusSnapshot> {
    match send_request(DaemonRequest::Status).await? {
        DaemonResponse::Status(status) => Ok(status),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub async fn request_devices() -> Result<Vec<DaemonDeviceSnapshot>> {
    match send_request(DaemonRequest::Devices).await? {
        DaemonResponse::Devices(devices) => Ok(devices),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub async fn request_shutdown() -> Result<()> {
    match send_request(DaemonRequest::Shutdown).await? {
        DaemonResponse::Ack => Ok(()),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub async fn request_connect(device_id: crate::DeviceId) -> Result<()> {
    match send_request(DaemonRequest::Connect { device_id }).await? {
        DaemonResponse::Ack => Ok(()),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub async fn request_disconnect(device_id: crate::DeviceId) -> Result<()> {
    match send_request(DaemonRequest::Disconnect { device_id }).await? {
        DaemonResponse::Ack => Ok(()),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub async fn wait_until_ready(timeout: Duration) -> Result<ServiceStatusSnapshot> {
    let deadline = Instant::now() + timeout;
    loop {
        match request_status().await {
            Ok(status) => return Ok(status),
            Err(err) if Instant::now() < deadline => {
                tracing::debug!("Daemon not ready yet: {}", err);
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

pub async fn spawn_daemon(port: Option<u16>, bind: Option<&str>) -> Result<ServiceStatusSnapshot> {
    let daemon_binary = find_daemon_binary()?;

    let mut command = tokio::process::Command::new(&daemon_binary);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Some(port) = port {
        command.env("RSHARE_PORT", port.to_string());
    }
    if let Some(bind) = bind {
        command.env("RSHARE_BIND", bind);
    }

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
        .spawn()
        .with_context(|| format!("Failed to spawn daemon: {}", daemon_binary.display()))?;

    wait_until_ready(Duration::from_secs(5)).await
}

pub fn find_daemon_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("RSHARE_DAEMON_BIN") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path);
        }
    }

    let current_exe = std::env::current_exe().context("Failed to locate current executable")?;
    let daemon_name = if cfg!(windows) {
        "rshare-daemon.exe"
    } else {
        "rshare-daemon"
    };

    for dir in current_exe.ancestors().take(4).filter(|path| path.is_dir()) {
        let candidate = dir.join(daemon_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "Could not find {} near {}. Build rshare-daemon first or set RSHARE_DAEMON_BIN.",
        daemon_name,
        current_exe.display()
    )
}

pub async fn request_layout() -> Result<LayoutGraph> {
    match send_request(DaemonRequest::GetLayout).await? {
        DaemonResponse::Layout(layout) => Ok(layout),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub async fn request_set_layout(layout: LayoutGraph) -> Result<()> {
    match send_request(DaemonRequest::SetLayout { layout }).await? {
        DaemonResponse::Ack => Ok(()),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub async fn request_local_controls() -> Result<LocalControlDeviceSnapshot> {
    match send_request(DaemonRequest::LocalControls).await? {
        DaemonResponse::LocalControls(snapshot) => Ok(snapshot),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub async fn request_local_input_test(test: LocalInputTestRequest) -> Result<LocalInputTestResult> {
    match send_request(DaemonRequest::RunLocalInputTest { test }).await? {
        DaemonResponse::LocalInputTest(result) => Ok(result),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub async fn request_remote_latency_test(
    device_id: crate::DeviceId,
) -> Result<LocalInputTestResult> {
    match send_request(DaemonRequest::RunRemoteLatencyTest { device_id }).await? {
        DaemonResponse::LocalInputTest(result) => Ok(result),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub async fn subscribe_local_controls() -> Result<TcpStream> {
    let mut stream = TcpStream::connect(default_ipc_addr())
        .await
        .with_context(|| format!("Failed to connect to daemon at {}", default_ipc_addr()))?;
    write_json_line(&mut stream, &DaemonRequest::SubscribeLocalControls).await?;
    Ok(stream)
}

pub async fn read_local_control_event(stream: &mut TcpStream) -> Result<DaemonResponse> {
    let response: DaemonResponse = read_json_line(stream).await?;
    match response {
        DaemonResponse::LocalControls(_) | DaemonResponse::LocalControlEvent(_) => Ok(response),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {:?}", other),
    }
}

pub type LocalControlsWsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub async fn subscribe_local_controls_ws() -> Result<LocalControlsWsStream> {
    let url = default_local_controls_ws_url();
    let (stream, _) = connect_async(url.as_str())
        .await
        .with_context(|| format!("Failed to connect to {url}"))?;
    Ok(stream)
}

pub async fn read_local_control_ws_event(
    stream: &mut LocalControlsWsStream,
) -> Result<DaemonResponse> {
    while let Some(message) = stream.next().await {
        match message? {
            WsMessage::Text(text) => {
                let response: DaemonResponse = serde_json::from_str(&text)?;
                return match response {
                    DaemonResponse::LocalControls(_) | DaemonResponse::LocalControlEvent(_) => {
                        Ok(response)
                    }
                    DaemonResponse::Error(message) => anyhow::bail!(message),
                    other => anyhow::bail!("Unexpected daemon websocket response: {:?}", other),
                };
            }
            WsMessage::Close(_) => anyhow::bail!("Local controls websocket closed"),
            _ => {}
        }
    }
    anyhow::bail!("Local controls websocket ended")
}
