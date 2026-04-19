//! Devices command implementation.

use anyhow::Result;
use colored::Colorize;

use crate::output::{header, kv, status_ok, table_header, table_row, warning};

/// Device information for display.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub addresses: String,
    pub status: String,
    pub last_seen: String,
}

/// Execute the devices command.
pub async fn execute(detailed: bool, watch: bool) -> Result<()> {
    if watch {
        execute_watch(detailed).await?;
    } else {
        execute_list(detailed).await?;
    }

    Ok(())
}

async fn execute_list(detailed: bool) -> Result<()> {
    let devices = fetch_devices().await?;
    render_devices(&devices, detailed);
    Ok(())
}

async fn execute_watch(detailed: bool) -> Result<()> {
    use tokio::time::{interval, Duration};

    warning("Watching for device changes (press Ctrl+C to stop)");
    let mut ticker = interval(Duration::from_secs(2));
    let mut last_count = None;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let devices = fetch_devices().await?;
                print!("\x1B[2J\x1B[1;1H");
                if let Some(previous) = last_count {
                    if previous != devices.len() {
                        warning(&format!("Device count changed: {} -> {}", previous, devices.len()));
                    }
                }
                last_count = Some(devices.len());
                render_devices(&devices, detailed);
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    Ok(())
}

async fn fetch_devices() -> Result<Vec<DeviceInfo>> {
    let snapshots = rshare_core::daemon_client::request_devices().await?;
    Ok(snapshots.into_iter().map(DeviceInfo::from).collect())
}

fn render_devices(devices: &[DeviceInfo], detailed: bool) {
    header("Devices");

    if devices.is_empty() {
        warning("No devices discovered");
        return;
    }

    if detailed {
        print_detailed_devices(devices);
    } else {
        print_device_table(devices);
    }
}

fn print_device_table(devices: &[DeviceInfo]) {
    table_header(&["NAME", "HOSTNAME", "STATUS", "LAST SEEN"]);

    for device in devices {
        let status_icon = match device.status.as_str() {
            "connected" => "🟢",
            "discovered" => "🟡",
            _ => "⚪",
        };

        table_row(&[
            &format!("{} {}", status_icon, device.name),
            &device.hostname,
            &device.status,
            &device.last_seen,
        ]);
    }
}

fn print_detailed_devices(devices: &[DeviceInfo]) {
    for device in devices {
        println!();
        println!("  {}", device.name.bold());
        println!("  {}", "─".repeat(device.name.len() + 4));

        kv("ID", &device.id);
        kv("Hostname", &device.hostname);
        kv("Addresses", &device.addresses);
        kv("Last Seen", &device.last_seen);

        match device.status.as_str() {
            "connected" => status_ok("Status: Connected"),
            "discovered" => println!("  [DISCOVERED] Status: Discovered"),
            other => println!("  [UNKNOWN] Status: {}", other),
        }
    }
}

impl From<rshare_core::DaemonDeviceSnapshot> for DeviceInfo {
    fn from(value: rshare_core::DaemonDeviceSnapshot) -> Self {
        let last_seen = value
            .last_seen_secs
            .map(|secs| format!("{}s ago", secs))
            .unwrap_or_else(|| "unknown".to_string());

        Self {
            id: value.id.to_string(),
            name: value.name,
            hostname: value.hostname,
            addresses: if value.addresses.is_empty() {
                "—".to_string()
            } else {
                value.addresses.join(", ")
            },
            status: if value.connected {
                "connected".to_string()
            } else {
                "discovered".to_string()
            },
            last_seen,
        }
    }
}
