//! R-ShareMouse CLI application
//!
//! Command-line interface for R-ShareMouse.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
mod config;
mod output;

use commands::{config_cmd, devices, discover, start, stop, usb};
use config_cmd::ConfigCommands;

#[derive(Parser)]
#[command(name = "rshare")]
#[command(about = "R-ShareMouse - Cross-platform mouse and keyboard sharing", long_about = None)]
#[command(version)]
#[command(propagate_version = true)]
struct Cli {
    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Quiet mode (no output)
    #[arg(short, long)]
    quiet: bool,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the R-ShareMouse service
    Start {
        /// Run in background (daemon mode)
        #[arg(short, long)]
        daemon: bool,

        /// Log file path
        #[arg(short, long)]
        log_file: Option<String>,

        /// Port to listen on
        #[arg(short, long)]
        port: Option<u16>,

        /// Bind address
        #[arg(short, long)]
        bind: Option<String>,
    },

    /// Stop the R-ShareMouse service
    Stop {
        /// Force stop without graceful shutdown
        #[arg(short, long)]
        force: bool,
    },

    /// Restart the R-ShareMouse service
    Restart {
        /// Run in background (daemon mode)
        #[arg(short, long)]
        daemon: bool,
    },

    /// Show connected devices
    Devices {
        /// Show detailed information
        #[arg(short, long)]
        detailed: bool,

        /// Watch for device changes
        #[arg(short, long)]
        watch: bool,
    },

    /// Show service status
    Status {
        /// Show detailed status including network info
        #[arg(short, long)]
        detailed: bool,
    },

    /// Configuration management
    Config {
        #[command(subcommand)]
        config_cmd: ConfigCommands,
    },

    /// Show logs
    Logs {
        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: usize,

        /// Follow log output
        #[arg(short, long)]
        follow: bool,
    },

    /// Discover devices on the LAN
    Discover {
        /// Scan duration in seconds (default: 30)
        #[arg(short, long, default_value = "30")]
        duration: u64,

        /// Continuous mode (don't stop until Ctrl+C)
        #[arg(short, long)]
        continuous: bool,
    },

    /// Experimental USB forwarding tools
    Usb {
        #[command(subcommand)]
        usb_cmd: usb::UsbCommands,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Setup tracing
    let log_level = if cli.verbose {
        tracing::Level::DEBUG
    } else if cli.quiet {
        tracing::Level::WARN
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .with_target(false)
        .init();

    // Run command
    match cli.command {
        Commands::Start {
            daemon,
            log_file,
            port,
            bind,
        } => {
            start::execute(daemon, log_file, port, bind).await?;
        }
        Commands::Stop { force } => {
            stop::execute(force).await?;
        }
        Commands::Restart { daemon } => {
            stop::execute(false).await?;
            start::execute(daemon, None, None, None).await?;
        }
        Commands::Devices { detailed, watch } => {
            devices::execute(detailed, watch).await?;
        }
        Commands::Status { detailed } => {
            commands::status::execute(detailed).await?;
        }
        Commands::Config { config_cmd } => {
            config_cmd::execute(config_cmd).await?;
        }
        Commands::Logs { lines, follow } => {
            commands::logs::execute(lines, follow).await?;
        }
        Commands::Discover {
            duration,
            continuous,
        } => {
            if continuous {
                discover::run_discover_test().await?;
            } else {
                discover::run_discover_scan(std::time::Duration::from_secs(duration)).await?;
            }
        }
        Commands::Usb { usb_cmd } => {
            usb::execute(usb_cmd).await?;
        }
    }

    Ok(())
}
