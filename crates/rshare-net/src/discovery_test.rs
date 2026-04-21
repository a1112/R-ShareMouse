//! Device discovery test utility
//!
//! Run this to test LAN device discovery functionality.

use std::time::Duration;
use tokio::time::timeout;

pub async fn test_discovery() -> anyhow::Result<()> {
    use crate::discovery::{spawn_discovery, DiscoveryConfig, DiscoveryEvent, ServiceDiscovery};

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

    // Run for 60 seconds or until interrupted
    let start = std::time::Instant::now();
    let duration = Duration::from_secs(60);

    while start.elapsed() < duration {
        match timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Some(event)) => match event {
                DiscoveryEvent::DeviceFound(device) => {
                    println!("✓ Device FOUND:");
                    println!("    ID: {}", device.id);
                    println!("    Name: {}", device.name);
                    println!("    Hostname: {}", device.hostname);
                    println!("    Addresses: {:?}", device.addresses);
                    println!("    Last seen: {:?}", device.last_seen);
                    println!();
                }
                DiscoveryEvent::DeviceUpdated(device) => {
                    println!("~ Device UPDATED:");
                    println!("    ID: {}", device.id);
                    println!("    Name: {}", device.name);
                    println!("    Addresses: {:?}", device.addresses);
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
                break;
            }
            Err(_) => {
                // Timeout - print heartbeat
                print!(".");
                std::io::Write::flush(&mut std::io::stdout())?;
            }
        }
    }

    println!();
    println!("Stopping discovery...");
    discovery_task.stop().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Run with: cargo test --package rshare-net -- --ignored
    async fn test_lan_discovery() {
        if let Err(e) = test_discovery().await {
            eprintln!("Discovery test failed: {}", e);
            eprintln!("Note: This test requires network access and may fail in CI.");
        }
    }
}
