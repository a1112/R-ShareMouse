//! Devices command implementation

use anyhow::Result;
use crate::output::{header, success, kv, table_header, table_row, status_ok, status_err, warning};
use colored::Colorize;

/// Device information for display
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub address: String,
    pub status: String,
    pub latency_ms: Option<u64>,
}

/// Execute the devices command
pub async fn execute(detailed: bool, watch: bool) -> Result<()> {
    if watch {
        execute_watch(detailed).await?;
    } else {
        execute_list(detailed).await?;
    }

    Ok(())
}

/// List all devices
async fn execute_list(detailed: bool) -> Result<()> {
    header("Connected Devices");

    // TODO: Get actual device list from service
    let devices = get_mock_devices();

    if devices.is_empty() {
        warning("No devices connected");
        return Ok(());
    }

    if detailed {
        print_detailed_devices(&devices);
    } else {
        print_device_table(&devices);
    }

    success(&format!("Total: {} device(s)", devices.len()));

    Ok(())
}

/// Watch for device changes
async fn execute_watch(detailed: bool) -> Result<()> {
    use tokio::time::{interval, Duration};

    warning("Watching for device changes (press Ctrl+C to stop)");

    let mut ticker = interval(Duration::from_secs(2));
    let mut last_count = 0usize;

    loop {
        ticker.tick().await;

        // Clear screen
        print!("\x1B[2J\x1B[1;1H");

        header("Connected Devices (Watching)");

        let devices = get_mock_devices();

        if devices.len() != last_count {
            // Device count changed
            if devices.len() > last_count {
                success(&format!("New device connected! Total: {}", devices.len()));
            } else {
                warning(&format!("Device disconnected! Total: {}", devices.len()));
            }
            last_count = devices.len();
        }

        if detailed {
            print_detailed_devices(&devices);
        } else {
            print_device_table(&devices);
        }

        // Check for Ctrl+C
        if tokio::signal::ctrl_c().await.is_ok() {
            break;
        }
    }

    Ok(())
}

/// Print devices in table format
fn print_device_table(devices: &[DeviceInfo]) {
    table_header(&["NAME", "HOSTNAME", "STATUS", "LATENCY"]);

    for device in devices {
        let latency = device.latency_ms
            .map(|l| format!("{}ms", l))
            .unwrap_or_else(|| "—".to_string());

        let status_emoji = match device.status.as_str() {
            "online" => "🟢",
            "offline" => "🔴",
            _ => "⚪",
        };

        table_row(&[
            &format!("{} {}", status_emoji, device.name),
            &device.hostname,
            &device.status,
            &latency,
        ]);
    }
}

/// Print detailed device information
fn print_detailed_devices(devices: &[DeviceInfo]) {
    for device in devices {
        println!();
        println!("  {}", device.name.bold());
        println!("  {}", "─".repeat(device.name.len() + 4));

        kv("ID", &device.id);
        kv("Hostname", &device.hostname);
        kv("Address", &device.address);

        match device.status.as_str() {
            "online" => status_ok("Status: Online"),
            _ => status_err(&format!("Status: {}", device.status)),
        }

        if let Some(latency) = device.latency_ms {
            kv("Latency", &format!("{} ms", latency));
        }
    }
}

/// Get mock device data for demonstration
fn get_mock_devices() -> Vec<DeviceInfo> {
    vec![
        DeviceInfo {
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Desktop-PC".to_string(),
            hostname: "desktop-pc".to_string(),
            address: "192.168.1.100:4242".to_string(),
            status: "online".to_string(),
            latency_ms: Some(12),
        },
        DeviceInfo {
            id: "550e8400-e29b-41d4-a716-446655440001".to_string(),
            name: "MacBook-Pro".to_string(),
            hostname: "macbook-pro".to_string(),
            address: "192.168.1.101:4242".to_string(),
            status: "online".to_string(),
            latency_ms: Some(8),
        },
        DeviceInfo {
            id: "550e8400-e29b-41d4-a716-446655440002".to_string(),
            name: "Work-Laptop".to_string(),
            hostname: "work-laptop".to_string(),
            address: "192.168.1.102:4242".to_string(),
            status: "offline".to_string(),
            latency_ms: None,
        },
    ]
}
