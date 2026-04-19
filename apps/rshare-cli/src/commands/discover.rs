//! Discovery test command

use std::time::Duration;
use tokio::time::timeout;

pub async fn run_discover_test() -> anyhow::Result<()> {
    use rshare_net::discovery::{DiscoveryConfig, DiscoveryEvent, ServiceDiscovery};

    println!("R-ShareMouse Discovery Test");
    println!("=========================");
    println!();

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
        broadcast_interval: Duration::from_secs(2),
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

    discovery.start_with_channel(tx).await?;

    println!("Discovery started! Listening for devices...");
    println!("---");

    let mut discovered_count = 0;
    let start = std::time::Instant::now();

    loop {
        match timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Some(event)) => match event {
                DiscoveryEvent::DeviceFound(device) => {
                    discovered_count += 1;
                    println!(
                        "✓ Device FOUND #{} ({:?}):",
                        discovered_count,
                        start.elapsed()
                    );
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
                }
            },
            Ok(None) => {
                println!("Channel closed");
                return Ok(());
            }
            Err(_) => {
                // Timeout - print heartbeat
                print!(".");
                std::io::Write::flush(&mut std::io::stdout())?;
            }
        }
    }
}

/// Interactive discovery test - runs for 30 seconds
pub async fn run_discover_scan(scan_duration: Duration) -> anyhow::Result<()> {
    use rshare_net::discovery::{DiscoveryConfig, DiscoveryEvent, ServiceDiscovery};

    println!(
        "R-ShareMouse Discovery Scan ({} seconds)",
        scan_duration.as_secs()
    );
    println!("========================================");
    println!();

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
        broadcast_interval: Duration::from_secs(1), // Aggressive
        device_timeout: Duration::from_secs(10),
        mdns_enabled: false,
    };
    discovery = discovery.with_config(config);

    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    discovery.start_with_channel(tx).await?;

    println!("Scanning...");
    println!("---");

    let mut devices = std::collections::HashMap::new();

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
            }
            _ => {}
        }
    }

    discovery.stop().await.ok();

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
