//! Configuration file handling

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// CLI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    /// Server settings
    pub server: ServerConfig,
    /// Network settings
    pub network: NetworkConfig,
    /// Logging settings
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Whether to start as daemon
    pub daemon: bool,
    /// Log file path
    pub log_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Port to listen on
    pub port: u16,
    /// Bind address
    pub bind_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level
    pub level: String,
    /// Whether to log to file
    pub to_file: bool,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                daemon: false,
                log_file: None,
            },
            network: NetworkConfig {
                port: 4242,
                bind_address: "0.0.0.0".to_string(),
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                to_file: false,
            },
        }
    }
}

/// Get the default configuration file path
pub fn get_config_path() -> Result<PathBuf> {
    let config_dir = directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rshare");

    std::fs::create_dir_all(&config_dir)?;
    Ok(config_dir.join("config.toml"))
}

/// Load configuration from file
pub fn load_config() -> Result<CliConfig> {
    let config_path = get_config_path()?;

    if !config_path.exists() {
        // Create default config
        let default_config = CliConfig::default();
        save_config(&default_config)?;
        return Ok(default_config);
    }

    let content = std::fs::read_to_string(&config_path)?;
    let config: CliConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Save configuration to file
pub fn save_config(config: &CliConfig) -> Result<()> {
    let config_path = get_config_path()?;
    let content = toml::to_string_pretty(config)?;
    std::fs::write(&config_path, content)?;
    Ok(())
}

/// Get a configuration value by key
pub fn get_config_value(key: &str) -> Result<String> {
    let config = load_config()?;

    match key {
        "server.daemon" => Ok(config.server.daemon.to_string()),
        "network.port" => Ok(config.network.port.to_string()),
        "network.bind_address" => Ok(config.network.bind_address.clone()),
        "logging.level" => Ok(config.logging.level.clone()),
        _ => anyhow::bail!("Unknown configuration key: {}", key),
    }
}

/// Set a configuration value by key
pub fn set_config_value(key: &str, value: &str) -> Result<()> {
    let mut config = load_config()?;

    match key {
        "server.daemon" => {
            config.server.daemon = value.parse()?;
        }
        "network.port" => {
            config.network.port = value.parse()?;
        }
        "network.bind_address" => {
            config.network.bind_address = value.to_string();
        }
        "logging.level" => {
            config.logging.level = value.to_string();
        }
        _ => anyhow::bail!("Unknown configuration key: {}", key),
    }

    save_config(&config)?;
    Ok(())
}
