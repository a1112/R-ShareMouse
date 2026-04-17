//! Application configuration.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Application configuration shared by the GUI and engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub network: NetworkConfig,
    pub gui: GuiConfig,
    pub input: InputConfig,
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkConfig {
    pub port: u16,
    pub bind_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiConfig {
    pub minimize_to_tray: bool,
    pub show_notifications: bool,
    pub start_minimized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputConfig {
    pub clipboard_sync: bool,
    pub edge_threshold: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecurityConfig {
    pub password_required: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            network: NetworkConfig {
                port: 27431,
                bind_address: "0.0.0.0".to_string(),
            },
            gui: GuiConfig {
                minimize_to_tray: true,
                show_notifications: true,
                start_minimized: false,
            },
            input: InputConfig {
                clipboard_sync: true,
                edge_threshold: 10,
            },
            security: SecurityConfig {
                password_required: false,
            },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "rshare-config-test-{}-{}",
                name,
                uuid::Uuid::new_v4()
            ))
            .join("config.toml")
    }

    #[test]
    fn default_config_matches_expected_values() {
        let config = Config::default();
        assert_eq!(config.network.port, 27431);
        assert_eq!(config.network.bind_address, "0.0.0.0");
        assert!(config.gui.minimize_to_tray);
        assert!(config.gui.show_notifications);
        assert!(!config.gui.start_minimized);
        assert!(config.input.clipboard_sync);
        assert_eq!(config.input.edge_threshold, 10);
        assert!(!config.security.password_required);
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

        config.save_to_path(&path).unwrap();
        let loaded = Config::load_from_path(&path).unwrap();

        assert_eq!(loaded, config);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
