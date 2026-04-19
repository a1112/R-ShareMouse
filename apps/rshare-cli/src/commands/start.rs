//! Start command implementation.

use crate::output::{info, success, warning};
use anyhow::Result;

/// Execute the start command.
pub async fn execute(
    daemon: bool,
    _log_file: Option<String>,
    port: Option<u16>,
    bind: Option<String>,
) -> Result<()> {
    let service_manager = rshare_core::service::ServiceManager::new()?;

    if service_manager.is_running() {
        let status = rshare_core::daemon_client::request_status().await.ok();
        if let Some(status) = status {
            warning(&format!(
                "R-ShareMouse service is already running (PID: {})",
                status.pid
            ));
        } else {
            warning("R-ShareMouse service is already running");
        }
        return Ok(());
    }

    if !daemon {
        info("Foreground mode now delegates to the managed daemon service");
    }

    let status = rshare_core::daemon_client::spawn_daemon(port, bind.as_deref()).await?;

    success("R-ShareMouse service started");
    info(&format!("PID: {}", status.pid));
    info(&format!("Listening on: {}", status.bind_address));
    info(&format!("Discovery port: {}", status.discovery_port));

    Ok(())
}
