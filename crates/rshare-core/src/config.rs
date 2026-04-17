//! Application configuration.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::protocol::{DeviceId, Direction};

/// Application configuration shared by the GUI and engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub gui: GuiConfig,
    #[serde(default)]
    pub input: InputConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    /// Known device hostnames
    #[serde(default)]
    pub known_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkConfig {
    pub port: u16,
    pub bind_address: String,
    /// Enable mDNS discovery
    pub mdns_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiConfig {
    pub minimize_to_tray: bool,
    pub show_notifications: bool,
    pub start_minimized: bool,
    pub show_tray_icon: bool,
    #[serde(default)]
    pub screen_layout: Vec<ScreenLayoutEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScreenLayoutEntry {
    pub device_id: DeviceId,
    pub direction: Direction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputConfig {
    pub clipboard_sync: bool,
    pub edge_threshold: u32,
    /// Send mouse wheel events
    pub mouse_wheel_sync: bool,
    /// Key delay in milliseconds (for macro protection)
    pub key_delay_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecurityConfig {
    pub password_required: bool,
    /// Enable TLS/SSL encryption
    pub encryption: bool,
    /// Password hash (bcrypt)
    pub password_hash: Option<String>,
    /// Trusted device IDs
    pub trusted_devices: Vec<DeviceId>,
    /// Allow LAN only (prevent WAN access)
    pub lan_only: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            port: 27431,
            bind_address: "0.0.0.0".to_string(),
            mdns_enabled: true,
        }
    }
}

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            minimize_to_tray: true,
            show_notifications: true,
            start_minimized: false,
            show_tray_icon: true,
            screen_layout: Vec::new(),
        }
    }
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            clipboard_sync: true,
            edge_threshold: 10,
            mouse_wheel_sync: true,
            key_delay_ms: 0,
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            password_required: false,
            encryption: true,
            password_hash: None,
            trusted_devices: Vec::new(),
            lan_only: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            network: NetworkConfig::default(),
            gui: GuiConfig::default(),
            input: InputConfig::default(),
            security: SecurityConfig::default(),
            known_devices: Vec::new(),
        }
    }
}

impl Config {
    /// Load configuration from the default config path, creating it if missing.
    pub fn load() -> Result<Self> {
        Self::load_from_path(default_config_path()?)
    }

    /// Save configuration to the default config path.
    pub fn save(&self) -> Result<()> {
        self.save_to_path(default_config_path()?)
    }

    /// Load configuration from a specific path, creating a default file if missing.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            let config = Self::default();
            config.save_to_path(path)?;
            return Ok(config);
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))
    }

    /// Save configuration to a specific path.
    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        std::fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))
    }

    /// Get the bind address for the server
    pub fn bind_address(&self) -> Result<SocketAddr> {
        format!("{}:{}", self.network.bind_address, self.network.port)
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid bind address: {}", e))
    }

    /// Check if a device is trusted
    pub fn is_trusted(&self, device_id: &DeviceId) -> bool {
        self.security.trusted_devices.contains(device_id)
    }

    /// Add a trusted device
    pub fn add_trusted_device(&mut self, device_id: DeviceId) {
        if !self.is_trusted(&device_id) {
            self.security.trusted_devices.push(device_id);
        }
    }

    /// Remove a trusted device
    pub fn remove_trusted_device(&mut self, device_id: &DeviceId) {
        self.security.trusted_devices.retain(|id| id != device_id);
    }

    /// Get the effective edge threshold
    pub fn edge_threshold(&self) -> u32 {
        self.input.edge_threshold.max(1).min(100)
    }

    /// Legacy method for compatibility
    pub fn config_path() -> PathBuf {
        default_config_path().unwrap_or_else(|_| PathBuf::from("config.toml"))
    }
}

/// Get the default configuration file path.
pub fn default_config_path() -> Result<PathBuf> {
    let base_dir = if cfg!(target_os = "macos") {
        dirs::home_dir().map(|p| p.join("Library").join("Application Support"))
    } else if cfg!(target_os = "windows") {
        dirs::config_dir()
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|p| p.join(".config")))
    };

    Ok(base_dir
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rshare")
        .join("config.toml"))
}

/// Hotkey configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HotkeyConfig {
    /// Toggle sharing hotkey (Ctrl+Alt+S by default)
    pub toggle_sharing: Option<String>,
    /// Lock cursor hotkey (Ctrl+Alt+L by default)
    pub lock_cursor: Option<String>,
    /// Hotkey to switch to specific screen
    pub switch_screen: Vec<SwitchScreenHotkey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwitchScreenHotkey {
    pub hotkey: String,
    pub device_id: DeviceId,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            toggle_sharing: Some("Ctrl+Alt+S".to_string()),
            lock_cursor: Some("Ctrl+Alt+L".to_string()),
            switch_screen: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_config_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("rshare-config-test-{}-{}", name, Uuid::new_v4()))
            .join("config.toml")
    }

    #[test]
    fn default_config_matches_expected_values() {
        let config = Config::default();
        assert_eq!(config.network.port, 27431);
        assert_eq!(config.network.bind_address, "0.0.0.0");
        assert!(config.network.mdns_enabled);
        assert!(config.gui.minimize_to_tray);
        assert!(config.gui.show_notifications);
        assert!(config.gui.show_tray_icon);
        assert!(!config.gui.start_minimized);
        assert!(config.gui.screen_layout.is_empty());
        assert!(config.input.clipboard_sync);
        assert_eq!(config.input.edge_threshold, 10);
        assert!(config.input.mouse_wheel_sync);
        assert!(!config.security.password_required);
        assert!(config.security.encryption);
        assert!(config.security.lan_only);
    }

    #[test]
    fn load_creates_missing_config_file() {
        let path = temp_config_path("missing");
        let config = Config::load_from_path(&path).unwrap();
        assert_eq!(config, Config::default());
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_round_trips_config() {
        let path = temp_config_path("roundtrip");
        let mut config = Config::default();
        config.network.port = 4242;
        config.gui.start_minimized = true;
        config.input.edge_threshold = 24;
        config.security.password_required = true;
        config.gui.screen_layout.push(ScreenLayoutEntry {
            device_id: DeviceId::new_v4(),
            direction: Direction::Right,
        });

        config.save_to_path(&path).unwrap();
        let loaded = Config::load_from_path(&path).unwrap();

        assert_eq!(loaded, config);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_bind_address() {
        let config = Config::default();
        let addr = config.bind_address().unwrap();
        assert_eq!(addr.port(), 27431);
        assert_eq!(addr.ip().to_string(), "0.0.0.0");
    }

    #[test]
    fn test_edge_threshold_bounds() {
        let mut config = Config::default();
        config.input.edge_threshold = 0;
        assert_eq!(config.edge_threshold(), 1);

        config.input.edge_threshold = 200;
        assert_eq!(config.edge_threshold(), 100);
    }

    #[test]
    fn test_trusted_devices() {
        let mut config = Config::default();
        let id = DeviceId::new_v4();

        assert!(!config.is_trusted(&id));
        config.add_trusted_device(id);
        assert!(config.is_trusted(&id));
        config.remove_trusted_device(&id);
        assert!(!config.is_trusted(&id));
    }
}
