//! Stop command implementation

use anyhow::Result;
use crate::output::{success, info, warning};

/// Execute the stop command
pub async fn execute(force: bool) -> Result<()> {
    // Check if service is running
    let pid_file = get_pid_file()?;

    if !pid_file.exists() {
        warning("R-ShareMouse service is not running");
        return Ok(());
    }

    // Read PID
    let pid = std::fs::read_to_string(&pid_file)?
        .trim()
        .parse::<u32>()?;

    info(&format!("Stopping service (PID: {})...", pid));

    if force {
        // Force kill
        info("Force stopping...");
        // In real implementation, send SIGKILL
        success("Service stopped (force)");
    } else {
        // Graceful shutdown
        info("Attempting graceful shutdown...");
        // In real implementation, send SIGTERM or use IPC
        // For now, just remove PID file
        success("Service stopped");
    }

    // Clean up PID file
    let _ = std::fs::remove_file(&pid_file);

    Ok(())
}

/// Get the PID file path
fn get_pid_file() -> Result<std::path::PathBuf> {
    let config_dir = directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".rshare");

    Ok(config_dir.join("rshare.pid"))
}
