//! Start command implementation

use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;

/// Execute the start command
pub async fn execute(
    daemon: bool,
    _log_file: Option<String>,
    port: Option<u16>,
    bind: Option<String>,
) -> Result<()> {
    use crate::output::{info, success, warning};

    if daemon {
        // Start as daemon
        warning("Daemon mode not yet implemented, running in foreground");
    }

    // Load configuration
    let config_path = get_config_path()?;
    info(&format!("Using config: {}", config_path.display()));

    let bind_address =
        normalize_bind_address(bind.as_deref().unwrap_or("0.0.0.0"), port.unwrap_or(27431));

    success("R-ShareMouse service starting...");
    info(&format!("Discovery UDP: 0.0.0.0:27432"));
    info(&format!("Transport TCP: {}", bind_address));
    info("Press Ctrl+C to stop");

    let local_device_id = uuid::Uuid::new_v4();
    let hostname = hostname::get()
        .unwrap_or_else(|_| "unknown".into())
        .to_string_lossy()
        .to_string();
    let device_name = format!("{}-R-ShareMouse", hostname);

    let manager_config = rshare_net::NetworkManagerConfig {
        bind_address,
        auto_connect: false,
        broadcast_interval: Duration::from_secs(2),
        ..Default::default()
    };

    let mut network_manager =
        rshare_net::NetworkManager::new(local_device_id, device_name, hostname)
            .with_config(manager_config);
    network_manager.start().await?;
    let mut events = network_manager.events();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info("Received shutdown signal");
                break;
            }
            event = events.recv() => {
                match event {
                    Some(rshare_net::NetworkEvent::DeviceFound(device)) => {
                        info(&format!(
                            "Discovered {} ({}) at {:?}",
                            device.name,
                            device.id,
                            device.addresses
                        ));
                    }
                    Some(rshare_net::NetworkEvent::DeviceConnected(id)) => {
                        success(&format!("Device connected: {}", id));
                    }
                    Some(rshare_net::NetworkEvent::DeviceDisconnected(id)) => {
                        warning(&format!("Device disconnected: {}", id));
                    }
                    Some(rshare_net::NetworkEvent::ConnectionError { device_id, error }) => {
                        warning(&format!("Connection error for {}: {}", device_id, error));
                    }
                    Some(rshare_net::NetworkEvent::MessageReceived { from, message }) => {
                        info(&format!("Message from {}: {:?}", from, message));
                    }
                    None => {
                        warning("Network event channel closed");
                        break;
                    }
                }
            }
        }
    }

    network_manager.stop().await?;
    success("Service stopped");

    Ok(())
}

fn normalize_bind_address(bind: &str, port: u16) -> String {
    if bind.parse::<std::net::SocketAddr>().is_ok() {
        bind.to_string()
    } else {
        format!("{}:{}", bind, port)
    }
}

/// Get the configuration file path
fn get_config_path() -> Result<PathBuf> {
    let config_dir = directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rshare");

    // Create config directory if it doesn't exist
    std::fs::create_dir_all(&config_dir)?;

    Ok(config_dir.join("config.toml"))
}
