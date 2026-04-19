//! Status command implementation.

use anyhow::Result;
use colored::Colorize;

use crate::output::{header, kv, status_err, status_ok};

/// Execute the status command.
pub async fn execute(detailed: bool) -> Result<()> {
    header("Service Status");

    let manager = rshare_core::service::ServiceManager::new()?;
    if !manager.is_running() {
        status_err("Service Status: Stopped");
        return Ok(());
    }

    match rshare_core::daemon_client::request_status().await {
        Ok(status) => {
            status_ok("Service Status: Running");
            kv("PID", &status.pid.to_string());
            kv("Device", &status.device_name);
            kv("Hostname", &status.hostname);

            if detailed {
                print_detailed_status(&status);
            }
        }
        Err(err) => {
            status_err("Service Status: Unresponsive");
            if let Some(pid) = manager.get_pid() {
                kv("PID", &pid.to_string());
            }
            kv("Error", &err.to_string());
        }
    }

    Ok(())
}

fn print_detailed_status(status: &rshare_core::ServiceStatusSnapshot) {
    println!();
    println!("{}", "Network".bold());
    kv("Listening", &status.bind_address);
    kv("Discovery Port", &status.discovery_port.to_string());
    kv("Discovered Devices", &status.discovered_devices.to_string());
    kv("Connected Devices", &status.connected_devices.to_string());

    println!();
    println!("{}", "Input Backend".bold());
    if let Some(input_mode) = &status.input_mode {
        kv("Mode", &format!("{:?}", input_mode));
    } else {
        kv("Mode", "unknown");
    }
    if let Some(backend_health) = &status.backend_health {
        match backend_health {
            rshare_core::BackendHealth::Healthy => {
                let health = "healthy".green();
                kv("Health", &format!("{}", health));
            }
            rshare_core::BackendHealth::Degraded { reason } => {
                let health = format!("degraded: {:?}", reason).yellow();
                kv("Health", &format!("{}", health));
            }
        }
    }
    if let Some(available) = &status.available_backends {
        let backends: String = available
            .iter()
            .map(|k| format!("{:?}", k))
            .collect::<Vec<_>>()
            .join(", ");
        kv("Available", &backends);
    }
    if let Some(privilege_state) = &status.privilege_state {
        kv("Session", &format!("{:?}", privilege_state));
    }
    if let Some(error) = &status.last_backend_error {
        let err = error.red();
        kv("Last Error", &format!("{}", err));
    }

    println!();
    println!("{}", "Identity".bold());
    kv("Device ID", &status.device_id.to_string());
    kv("Healthy", if status.healthy { "yes" } else { "no" });
}
