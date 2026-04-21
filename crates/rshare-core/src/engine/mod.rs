//! Core engine for R-ShareMouse
//!
//! This module contains the main engine that coordinates all subsystems:
//! - Input event capture and forwarding
//! - Screen edge detection and switching
//! - Clipboard synchronization
//! - Connection management

pub mod clipboard;
pub mod forwarding;
pub mod hotkey;
pub mod state_machine;

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{config::Config, Device, DeviceId, Message, ScreenInfo};

/// Discovery event types
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    DeviceFound(Device),
    DeviceUpdated(Device),
    DeviceLost(DeviceId),
    Error(String),
}

/// Trait for connection manager (abstracted from rshare-net)
#[async_trait::async_trait]
pub trait ConnectionManager: Send + Sync {
    async fn start_server(&mut self, bind_addr: &str) -> Result<()>;
    async fn stop_server(&mut self) -> Result<()>;
    async fn connect_to(&mut self, addr: &str) -> Result<()>;
    async fn disconnect_from(&mut self, device_id: DeviceId) -> Result<()>;
    async fn send(&mut self, device_id: DeviceId, message: Message) -> Result<()>;
    async fn broadcast(&mut self, message: Message) -> Result<()>;
}

pub use clipboard::*;
pub use forwarding::*;
pub use hotkey::*;
pub use state_machine::*;

/// Trait for input listener (abstracted from rshare-input)
pub trait InputListener: Send + Sync {
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn is_running(&self) -> bool;
}

/// Trait for input emulator (abstracted from rshare-input)
pub trait InputEmulator: Send + Sync {
    fn move_mouse(&mut self, x: i32, y: i32) -> Result<()>;
    fn press_button(&mut self, button: u8) -> Result<()>;
    fn release_button(&mut self, button: u8) -> Result<()>;
    fn scroll_wheel(&mut self, delta: i32) -> Result<()>;
    fn press_key(&mut self, keycode: u32) -> Result<()>;
    fn release_key(&mut self, keycode: u32) -> Result<()>;
}

/// Engine state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
    Idle,
    Running,
    Paused,
    Error,
}

/// Engine configuration
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Local device information
    pub local_device_id: DeviceId,
    pub local_device_name: String,
    pub local_hostname: String,

    /// Local screen info
    pub local_screen: ScreenInfo,

    /// Application config
    pub app_config: Config,

    /// Whether to capture input when local
    pub capture_when_local: bool,

    /// Whether to show notifications
    pub show_notifications: bool,
}

impl EngineConfig {
    /// Create from app config
    pub fn from_config(
        local_device_id: DeviceId,
        local_device_name: String,
        local_hostname: String,
        local_screen: ScreenInfo,
        app_config: Config,
    ) -> Self {
        Self {
            local_device_id,
            local_device_name,
            local_hostname,
            local_screen,
            app_config,
            capture_when_local: false,
            show_notifications: true,
        }
    }
}

/// Main R-ShareMouse engine
pub struct RShareEngine {
    config: EngineConfig,
    state: Arc<RwLock<EngineState>>,
    connection_manager: Option<Arc<RwLock<dyn ConnectionManager>>>,
    input_listener: Option<Box<dyn InputListener>>,
    input_emulator: Option<Box<dyn InputEmulator>>,
}

impl RShareEngine {
    /// Create a new engine
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(EngineState::Idle)),
            connection_manager: None,
            input_listener: None,
            input_emulator: None,
        }
    }

    /// Set the connection manager
    pub fn set_connection_manager(&mut self, manager: Arc<RwLock<dyn ConnectionManager>>) {
        self.connection_manager = Some(manager);
    }

    /// Get the engine state
    pub async fn state(&self) -> EngineState {
        *self.state.read().await
    }

    /// Set the engine state
    pub async fn set_state(&self, state: EngineState) {
        *self.state.write().await = state;
    }

    /// Get the configuration
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Set the input listener
    pub fn set_input_listener(&mut self, listener: Box<dyn InputListener>) {
        self.input_listener = Some(listener);
    }

    /// Set the input emulator
    pub fn set_input_emulator(&mut self, emulator: Box<dyn InputEmulator>) {
        self.input_emulator = Some(emulator);
    }

    /// Start the engine
    pub async fn start(&mut self) -> Result<()> {
        tracing::info!("Starting R-ShareMouse engine");

        // Start connection manager server if available
        if let Some(mgr) = &self.connection_manager {
            let bind_addr = format!(
                "{}:{}",
                self.config.app_config.network.bind_address, self.config.app_config.network.port
            );

            let mut mgr_ref = mgr.write().await;
            mgr_ref.start_server(&bind_addr).await?;
        }

        self.set_state(EngineState::Running).await;

        Ok(())
    }

    /// Stop the engine
    pub async fn stop(&mut self) -> Result<()> {
        tracing::info!("Stopping R-ShareMouse engine");

        if let Some(listener) = self.input_listener.as_mut() {
            listener.stop()?;
        }

        self.set_state(EngineState::Idle).await;

        Ok(())
    }

    /// Process a discovery event
    pub async fn on_discovery_event(&mut self, event: DiscoveryEvent) -> Result<()> {
        match event {
            DiscoveryEvent::DeviceFound(device) => {
                tracing::info!("Device found: {}", device.name);
                // TODO: Add to device registry
            }
            DiscoveryEvent::DeviceUpdated(device) => {
                tracing::debug!("Device updated: {}", device.name);
            }
            DiscoveryEvent::DeviceLost(id) => {
                tracing::info!("Device lost: {}", id);
                // TODO: Remove from device registry
            }
            DiscoveryEvent::Error(err) => {
                tracing::error!("Discovery error: {}", err);
            }
        }
        Ok(())
    }

    /// Update screen layout from discovered devices
    pub async fn update_screen_layout(&mut self, devices: Vec<Device>) -> Result<()> {
        tracing::info!("Updating screen layout with {} devices", devices.len());

        // TODO: Implement auto-arrange layout
        // TODO: Update edge detector mappings

        Ok(())
    }
}

/// Shared engine reference for use across tasks
pub type SharedEngine = Arc<RwLock<RShareEngine>>;

/// Create a shared engine
pub fn create_shared_engine(config: EngineConfig) -> SharedEngine {
    Arc::new(RwLock::new(RShareEngine::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_state() {
        let config = EngineConfig::from_config(
            DeviceId::new_v4(),
            "Test".to_string(),
            "test".to_string(),
            ScreenInfo::primary(),
            Config::default(),
        );

        let engine = RShareEngine::new(config);
        assert_eq!(*engine.state.blocking_read(), EngineState::Idle);
    }

    #[test]
    fn test_engine_config() {
        let local_id = DeviceId::new_v4();
        let config = EngineConfig::from_config(
            local_id,
            "Test".to_string(),
            "test".to_string(),
            ScreenInfo::primary(),
            Config::default(),
        );

        assert_eq!(config.local_device_id, local_id);
        assert!(config.show_notifications);
    }
}
