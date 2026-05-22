//! Discovery test command

use rshare_core::{daemon_client, DaemonDeviceSnapshot, DeviceId};
use rshare_net::discovery::{spawn_discovery, DiscoveryConfig, DiscoveryEvent, ServiceDiscovery};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;

pub async fn run_discover_test() -> anyhow::Result<()> {
    println!("R-ShareMouse Discovery Test");
    println!("=========================");
    println!();

    if let Ok(status) = daemon_client::request_status().await {
        println!(
            "Daemon is already running (PID {}). Using daemon discovery table.",
            status.pid
        );
        println!("Press Ctrl+C to stop");
        println!();
        return run_daemon_discover_watch().await;
    }

    // Create discovery service
    let local_device_id = uuid::Uuid::new_v4();
    let hostname = hostname::get()
        .unwrap_or_else(|_| "unknown".into())
        .to_string_lossy()
        .to_string();
    let device_name = format!("{}-Test", hostname);

    println!("Local Device:");
    println!("  ID: {}", local_device_id);
    println!("  Name: {}", device_name);
    println!("  Hostname: {}", hostname);
    println!();

    let mut discovery = ServiceDiscovery::new(local_device_id, device_name, hostname);

    // Configure for aggressive discovery
    let config = DiscoveryConfig {
        port: 27432,
        initial_broadcast_interval: Duration::from_millis(500),
        broadcast_interval: Duration::from_secs(2),
        initial_broadcast_count: 6,
        device_timeout: Duration::from_secs(30),
        mdns_enabled: false,
    };
    discovery = discovery.with_config(config);

    // Create channel for events
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Start discovery
    println!("Starting discovery service on port 27432...");
    println!("Broadcast interval: 2 seconds");
    println!("Press Ctrl+C to stop");
    println!();

    let discovery_task = spawn_discovery(discovery, tx);

    println!("Discovery started! Listening for devices...");
    println!("---");

    let mut discovered_count = 0;
    let start = std::time::Instant::now();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!();
                println!("Total devices found: {}", discovered_count);
                println!("Stopping discovery...");
                discovery_task.abort();
                return Ok(());
            }
            event = timeout(Duration::from_secs(1), rx.recv()) => {
                match event {
                    Ok(Some(event)) => match event {
                    DiscoveryEvent::DeviceFound(device) => {
                        discovered_count += 1;
                        println!("✓ Device FOUND ({:?}):", start.elapsed());
                        println!("    ID: {}", device.id);
                        println!("    Name: {}", device.name);
                        println!("    Hostname: {}", device.hostname);
                        println!("    Addresses: {:?}", device.addresses);
                        println!();
                    }
                    DiscoveryEvent::DeviceUpdated(device) => {
                        println!("~ Device UPDATED:");
                        println!("    ID: {}", device.id);
                        println!("    Name: {}", device.name);
                        println!();
                    }
                    DiscoveryEvent::DeviceLost(id) => {
                        println!("✗ Device LOST: {}", id);
                        println!();
                    }
                    DiscoveryEvent::Error(err) => {
                        println!("! Error: {}", err);
                        if is_discovery_port_in_use_error(&err) {
                            println!("Discovery port is already in use. Falling back to daemon IPC...");
                            discovery_task.abort();
                            return run_daemon_discover_watch().await;
                        }
                    }
                    },
                    Ok(None) => {
                        println!("Channel closed");
                        return Ok(());
                    }
                    Err(_) => {
                        print!(".");
                        std::io::Write::flush(&mut std::io::stdout())?;
                    }
                }
            }
        }
    }
}

/// Interactive discovery test - runs for 30 seconds
pub async fn run_discover_scan(scan_duration: Duration) -> anyhow::Result<()> {
    println!(
        "R-ShareMouse Discovery Scan ({} seconds)",
        scan_duration.as_secs()
    );
    println!("========================================");
    println!();

    if let Ok(status) = daemon_client::request_status().await {
        println!(
            "Daemon is already running (PID {}). Using daemon discovery table.",
            status.pid
        );
        println!();
        return run_daemon_discover_scan(scan_duration).await;
    }

    let local_device_id = uuid::Uuid::new_v4();
    let hostname = hostname::get()
        .unwrap_or_else(|_| "unknown".into())
        .to_string_lossy()
        .to_string();
    let device_name = format!("{}-Scan", hostname);

    println!("Scanning for devices on this LAN...");
    println!("Local: {} ({})", device_name, hostname);
    println!();

    let mut discovery = ServiceDiscovery::new(local_device_id, device_name, hostname);

    let config = DiscoveryConfig {
        port: 27432,
        initial_broadcast_interval: Duration::from_millis(500),
        broadcast_interval: Duration::from_secs(1),
        initial_broadcast_count: 10,
        device_timeout: Duration::from_secs(10),
        mdns_enabled: false,
    };
    discovery = discovery.with_config(config);

    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    let discovery_task = spawn_discovery(discovery, tx);

    println!("Scanning...");
    println!("---");

    let mut devices = HashMap::new();

    let start = std::time::Instant::now();

    while start.elapsed() < scan_duration {
        match timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Some(DiscoveryEvent::DeviceFound(device))) => {
                devices.insert(device.id, device.clone());
                println!("Found: {} @ {:?}", device.name, device.addresses);
            }
            Ok(Some(DiscoveryEvent::DeviceUpdated(device))) => {
                devices.insert(device.id, device.clone());
            }
            Ok(Some(DiscoveryEvent::DeviceLost(id))) => {
                println!("Lost: {}", id);
                devices.remove(&id);
            }
            Ok(Some(DiscoveryEvent::Error(err))) => {
                println!("Error: {}", err);
                if is_discovery_port_in_use_error(&err) {
                    println!("Discovery port is already in use. Falling back to daemon IPC...");
                    discovery_task.stop().await;
                    return run_daemon_discover_scan(scan_duration.saturating_sub(start.elapsed()))
                        .await;
                }
            }
            _ => {}
        }
    }

    discovery_task.stop().await;

    println!();
    println!("---");
    println!("Scan complete!");
    println!("Total devices found: {}", devices.len());
    if !devices.is_empty() {
        println!();
        for (id, device) in devices {
            println!("  - {} ({})", device.name, id);
            println!("    Addresses: {:?}", device.addresses);
        }
    }

    Ok(())
}

async fn run_daemon_discover_watch() -> anyhow::Result<()> {
    let mut devices: HashMap<DeviceId, DaemonDeviceSnapshot> = HashMap::new();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!();
                print_daemon_discovery_summary(&devices);
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                update_daemon_devices(&mut devices).await?;
            }
        }
    }
}

async fn run_daemon_discover_scan(scan_duration: Duration) -> anyhow::Result<()> {
    let mut devices: HashMap<DeviceId, DaemonDeviceSnapshot> = HashMap::new();
    let start = std::time::Instant::now();

    println!("Scanning daemon discovery table...");
    println!("---");

    while start.elapsed() < scan_duration {
        update_daemon_devices(&mut devices).await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    println!();
    println!("---");
    println!("Scan complete!");
    print_daemon_discovery_summary(&devices);
    Ok(())
}

async fn update_daemon_devices(
    devices: &mut HashMap<DeviceId, DaemonDeviceSnapshot>,
) -> anyhow::Result<()> {
    for device in daemon_client::request_devices().await? {
        let is_new = !devices.contains_key(&device.id);
        if is_new {
            println!("Found: {} @ {:?}", device.name, device.addresses);
        }
        devices.insert(device.id, device);
    }
    Ok(())
}

fn print_daemon_discovery_summary(devices: &HashMap<DeviceId, DaemonDeviceSnapshot>) {
    println!("Total devices found: {}", devices.len());
    if !devices.is_empty() {
        println!();
        for (id, device) in devices {
            println!("  - {} ({})", device.name, id);
            println!("    Hostname: {}", device.hostname);
            println!("    Addresses: {:?}", device.addresses);
            println!("    Connected: {}", device.connected);
        }
    }
}

fn is_discovery_port_in_use_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("os error 10048")
        || lower.contains("address already in use")
        || lower.contains("only one usage of each socket address")
        || lower.contains("每个套接字地址")
}

#[cfg(test)]
mod tests {
    use super::is_discovery_port_in_use_error;

    #[test]
    fn detects_windows_discovery_port_conflict() {
        assert!(is_discovery_port_in_use_error(
            "通常每个套接字地址(协议/网络地址/端口)只允许使用一次。 (os error 10048)"
        ));
    }

    #[test]
    fn detects_english_discovery_port_conflict() {
        assert!(is_discovery_port_in_use_error(
            "Only one usage of each socket address is normally permitted. (os error 10048)"
        ));
        assert!(is_discovery_port_in_use_error("address already in use"));
    }

    #[test]
    fn ignores_unrelated_discovery_errors() {
        assert!(!is_discovery_port_in_use_error("permission denied"));
        assert!(!is_discovery_port_in_use_error("network unreachable"));
    }
}
