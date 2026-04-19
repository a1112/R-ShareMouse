//! Stop command implementation.

use anyhow::Result;
use std::time::Duration;

use crate::output::{info, success, warning};

/// Execute the stop command.
pub async fn execute(force: bool) -> Result<()> {
    let manager = rshare_core::service::ServiceManager::new()?;

    if !manager.is_running() {
        warning("R-ShareMouse service is not running");
        return Ok(());
    }

    if force {
        info("Force stopping service...");
        manager.stop().await?;
        success("Service stopped (force)");
        return Ok(());
    }

    info("Requesting graceful shutdown...");
    if let Err(err) = rshare_core::daemon_client::request_shutdown().await {
        warning(&format!(
            "Graceful shutdown request failed ({}), falling back to process stop",
            err
        ));
        manager.stop().await?;
        success("Service stopped");
        return Ok(());
    }

    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if !manager.is_running() {
            success("Service stopped");
            return Ok(());
        }
    }

    warning("Service did not exit after graceful shutdown, forcing stop");
    manager.stop().await?;
    success("Service stopped");
    Ok(())
}
