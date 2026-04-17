//! R-ShareMouse daemon service
//!
//! Background service that handles input sharing and device discovery.

use anyhow::Result;
use rshare_core::{DeviceId, ScreenInfo};
use rshare_net::NetworkManager;
use std::time::Duration;
use tokio::signal;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("R-ShareMouse daemon starting...");

    // Get local device info
    let hostname = hostname::get()
        .unwrap_or_else(|_| "unknown".into())
        .to_string_lossy()
        .to_string();

    let device_id = DeviceId::new_v4();
    let device_name = format!("{}-R-ShareMouse", hostname);

    // Create network manager
    let mut network_manager = NetworkManager::new(
        device_id,
        device_name.clone(),
        hostname.clone(),
    );

    // Get network events
    let mut events = network_manager.events();

    // Start the network manager
    network_manager.start().await?;

    tracing::info!("Daemon started as device {} ({})", device_name, device_id);
    tracing::info!("Listening for connections on 0.0.0.0:27431");
    tracing::info!("Device discovery on port 27432");

    // Handle network events
    let event_handler = async move {
        while let Some(event) = events.recv().await {
            match event {
                rshare_net::NetworkEvent::DeviceFound(device) => {
                    tracing::info!("Device found: {} ({})", device.name, device.id);
                }
                rshare_net::NetworkEvent::DeviceConnected(id) => {
                    tracing::info!("Device connected: {}", id);
                }
                rshare_net::NetworkEvent::DeviceDisconnected(id) => {
                    tracing::info!("Device disconnected: {}", id);
                }
                rshare_net::NetworkEvent::MessageReceived { from, message } => {
                    tracing::debug!("Message from {:?}: {:?}", from, message);
                }
                rshare_net::NetworkEvent::ConnectionError { device_id, error } => {
                    tracing::warn!("Connection error to {}: {}", device_id, error);
                }
            }
        }
    };

    // Shutdown handler
    let shutdown = async {
        signal::ctrl_c().await.expect("failed to install CTRL+C handler");
        tracing::info!("Shutdown signal received");
    };

    // Wait for either event handler or shutdown
    tokio::select! {
        _ = event_handler => {}
        _ = shutdown => {}
    }

    // Stop the network manager
    network_manager.stop().await?;

    tracing::info!("R-ShareMouse daemon stopped");
    Ok(())
}
