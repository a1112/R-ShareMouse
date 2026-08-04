//! R-ShareMouse CLI application
//!
//! Command-line interface for R-ShareMouse.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
mod config;
mod output;

const UNKNOWN_BUILD_VALUE: &str = "unknown";
const CLI_BUILD_INFO_PREFIX: &str = "rshare-build:";

fn build_metadata() -> String {
    let git_hash = option_env!("RSHARE_BUILD_GIT_HASH").unwrap_or(UNKNOWN_BUILD_VALUE);
    let build_timestamp = option_env!("RSHARE_BUILD_TIMESTAMP").unwrap_or(UNKNOWN_BUILD_VALUE);
    let dirty = option_env!("RSHARE_BUILD_DIRTY").unwrap_or("");
    let dirty_suffix = if dirty.is_empty() {
        String::new()
    } else {
        format!(" ({dirty})")
    };

    format!(
        "{CLI_BUILD_INFO_PREFIX} version={} commit={} time={}{}",
        env!("CARGO_PKG_VERSION"),
        git_hash,
        build_timestamp,
        dirty_suffix
    )
}

use commands::{approvals, config_cmd, devices, discover, display, doctor, start, stop, usb};
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

    /// Display and virtual display tools
    Display {
        #[command(subcommand)]
        display_cmd: display::DisplayCommand,
    },

    /// Run dual-machine readiness diagnostics
    Doctor {
        /// Connect all discovered peers before checking event and injection readiness
        #[arg(long)]
        connect: bool,

        /// Run a safe remote Shift loopback inject probe against the first connected peer
        #[arg(long)]
        inject: bool,

        /// Endpoint event limit per peer
        #[arg(long, default_value = "64")]
        endpoint_events: u16,

        /// Return a non-zero exit code when any blocking check is found
        #[arg(long)]
        strict: bool,
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

    /// List or approve inbound peer requests on this target.
    Approvals {
        #[command(subcommand)]
        approval_cmd: ApprovalCommands,
    },
}

#[derive(Subcommand)]
enum ApprovalCommands {
    List,
    Approve { approval_id: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("{}", build_metadata());
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
        Commands::Display { display_cmd } => {
            display::execute(display_cmd).await?;
        }
        Commands::Doctor {
            connect,
            inject,
            endpoint_events,
            strict,
        } => {
            doctor::execute(connect, inject, endpoint_events, strict).await?;
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
        Commands::Approvals { approval_cmd } => match approval_cmd {
            ApprovalCommands::List => approvals::list().await?,
            ApprovalCommands::Approve { approval_id } => approvals::approve(approval_id).await?,
        },
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_approval_list_and_approve_commands() {
        let list = Cli::try_parse_from(["rshare", "approvals", "list"]).unwrap();
        assert!(matches!(
            list.command,
            Commands::Approvals {
                approval_cmd: ApprovalCommands::List
            }
        ));
        let approve = Cli::try_parse_from(["rshare", "approvals", "approve", "opaque-id"]).unwrap();
        assert!(matches!(
            approve.command,
            Commands::Approvals {
                approval_cmd: ApprovalCommands::Approve { approval_id }
            } if approval_id == "opaque-id"
        ));
    }
}
