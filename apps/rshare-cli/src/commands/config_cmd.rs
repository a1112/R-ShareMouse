//! Config command implementation

use anyhow::Result;
use crate::output::{header, kv, success, error, warning};
use crate::config::{load_config, save_config, get_config_value, set_config_value, get_config_path};
use colored::Colorize;

/// Config subcommands (defined here to avoid circular imports)
#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show current configuration
    Show {
        /// Output as JSON
        #[arg(short, long)]
        json: bool,
    },

    /// Get a configuration value
    Get {
        /// Configuration key (e.g., network.port)
        key: String,
    },

    /// Set a configuration value
    Set {
        /// Configuration key (e.g., network.port)
        key: String,

        /// Configuration value
        value: String,
    },

    /// Reset configuration to defaults
    Reset {
        /// Confirm reset without prompting
        #[arg(short, long)]
        yes: bool,
    },

    /// Edit configuration file
    Edit,
}

use clap::Subcommand;

/// Execute config subcommands
pub async fn execute(cmd: ConfigCommands) -> Result<()> {
    match cmd {
        ConfigCommands::Show { json } => {
            execute_show(json).await?;
        }
        ConfigCommands::Get { key } => {
            execute_get(&key).await?;
        }
        ConfigCommands::Set { key, value } => {
            execute_set(&key, &value).await?;
        }
        ConfigCommands::Reset { yes } => {
            execute_reset(yes).await?;
        }
        ConfigCommands::Edit => {
            execute_edit().await?;
        }
    }

    Ok(())
}

/// Show current configuration
async fn execute_show(json: bool) -> Result<()> {
    let config = load_config()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&config)?);
    } else {
        header("Configuration");

        kv("Config file", &get_config_path()?.display().to_string());

        println!();
        println!("{}", "[server]".bold());
        kv("daemon", &config.server.daemon.to_string());
        if let Some(log) = &config.server.log_file {
            kv("log_file", &log.display().to_string());
        }

        println!();
        println!("{}", "[network]".bold());
        kv("port", &config.network.port.to_string());
        kv("bind_address", &config.network.bind_address);

        println!();
        println!("{}", "[logging]".bold());
        kv("level", &config.logging.level);
        kv("to_file", &config.logging.to_file.to_string());
    }

    Ok(())
}

/// Get a configuration value
async fn execute_get(key: &str) -> Result<()> {
    match get_config_value(key) {
        Ok(value) => {
            println!("{}", value);
            Ok(())
        }
        Err(e) => {
            error(&format!("Error getting config: {}", e));
            Err(e)
        }
    }
}

/// Set a configuration value
async fn execute_set(key: &str, value: &str) -> Result<()> {
    set_config_value(key, value)?;
    success(&format!("Set {} = {}", key, value));
    Ok(())
}

/// Reset configuration to defaults
async fn execute_reset(yes: bool) -> Result<()> {
    if !yes {
        println!("This will reset all configuration to default values.");
        print!("Are you sure? [y/N]: ");
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            warning("Reset cancelled");
            return Ok(());
        }
    }

    // Create default config
    let default_config = crate::config::CliConfig::default();
    save_config(&default_config)?;

    success("Configuration reset to defaults");
    Ok(())
}

/// Edit configuration file
async fn execute_edit() -> Result<()> {
    let config_path = get_config_path()?;

    // Get editor from environment
    let editor = std::env::var("EDITOR")
        .unwrap_or_else(|_| "notepad".to_string());

    warning(&format!("Opening {} with {}...", config_path.display(), editor));

    // Use spawn to wait for editor to close
    let status = std::process::Command::new(&editor)
        .arg(&config_path)
        .status()?;

    if status.success() {
        success("Configuration file updated");
    } else {
        error("Editor exited with non-zero status");
    }

    Ok(())
}
