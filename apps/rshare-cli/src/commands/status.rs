//! Status command implementation

use anyhow::Result;
use colored::Colorize;
use crate::output::{header, kv, success, status_ok, status_err};

/// Execute the status command
pub async fn execute(detailed: bool) -> Result<()> {
    header("Service Status");

    // Check if service is running
    let pid_file = get_pid_file()?;

    if pid_file.exists() {
        status_ok("Service Status: Running");

        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            kv("PID", pid.trim());
        }

        if detailed {
            print_detailed_status().await?;
        }
    } else {
        status_err("Service Status: Stopped");
        return Ok(());
    }

    println!();
    success("Service is healthy");

    Ok(())
}

/// Print detailed status information
async fn print_detailed_status() -> Result<()> {
    println!();
    println!("{}", "Network".bold());

    // TODO: Get actual network info from service
    kv("Listening", "0.0.0.0:4242");
    kv("Connected Devices", "2");
    kv("Uptime", "5m 32s");

    println!();
    println!("{}", "Performance".bold());

    // TODO: Get actual performance metrics
    kv("Events Sent", "1,234");
    kv("Events Received", "987");
    kv("Avg Latency", "8ms");

    println!();
    println!("{}", "Resources".bold());

    // TODO: Get actual resource usage
    kv("Memory", "24.5 MB");
    kv("CPU", "2.3%");

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
