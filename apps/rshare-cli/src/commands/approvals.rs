//! Target-local peer approval commands.

use anyhow::{Context, Result};
use rshare_core::{
    default_ipc_addr, read_json_frame, write_json_frame, DaemonRequest, DaemonResponse,
};
use tokio::net::TcpStream;

async fn request(request: DaemonRequest) -> Result<DaemonResponse> {
    let mut stream = TcpStream::connect(default_ipc_addr())
        .await
        .with_context(|| format!("Failed to connect to daemon at {}", default_ipc_addr()))?;
    write_json_frame(&mut stream, &request).await?;
    read_json_frame(&mut stream).await
}

pub async fn list() -> Result<()> {
    match request(DaemonRequest::ListPendingPeerApprovals).await? {
        DaemonResponse::PendingPeerApprovals(approvals) => {
            for approval in approvals {
                println!(
                    "{} {} {} expires_at_ms={}",
                    approval.approval_id,
                    approval.device_id,
                    approval.fingerprint,
                    approval.expires_at_ms
                );
            }
            Ok(())
        }
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {other:?}"),
    }
}

pub async fn approve(approval_id: String) -> Result<()> {
    match request(DaemonRequest::ApprovePeer { approval_id }).await? {
        DaemonResponse::Ack => Ok(()),
        DaemonResponse::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {other:?}"),
    }
}
