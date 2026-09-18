use anyhow::Result;
use clap::Subcommand;
use rshare_core::{network_audio::*, DeviceId};
#[derive(Subcommand)]
pub enum AudioCommandLine {
    /// Inspect actual endpoints, permissions, virtual devices and backend readiness
    Status,
    /// Enable/disable network audio configuration (does not install drivers)
    Configure {
        #[arg(long)]
        enabled: bool,
        #[arg(long, default_value_t = 3)]
        buffer_ms: u16,
        #[arg(long, default_value_t = 27438)]
        port: u16,
        #[arg(long,default_value_t=true,action=clap::ArgAction::Set)]
        auto_register: bool,
    },
    /// Authorize an operator-approved peer to use one local audio endpoint
    Grant {
        #[arg(long)]
        peer: DeviceId,
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        input: bool,
    },
    /// Revoke access to one local endpoint
    Revoke {
        #[arg(long)]
        peer: DeviceId,
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        input: bool,
    },
    /// Stop one audio session
    Close { session: DeviceId },
}
pub async fn execute(command: AudioCommandLine) -> Result<()> {
    let command = match command {
        AudioCommandLine::Status => AudioCommand::Status,
        AudioCommandLine::Configure {
            enabled,
            buffer_ms,
            port,
            auto_register,
        } => {
            let snapshot =
                rshare_core::daemon_client::request_network_audio(AudioCommand::Status).await?;
            AudioCommand::Configure(AudioConfig {
                enabled,
                buffer_ms,
                media_port: port,
                auto_register,
                grants: snapshot.config.grants,
            })
        }
        AudioCommandLine::Grant {
            peer,
            endpoint,
            input,
        } => AudioCommand::Grant(Grant {
            peer,
            endpoint,
            direction: if input {
                Direction::Input
            } else {
                Direction::Output
            },
        }),
        AudioCommandLine::Revoke {
            peer,
            endpoint,
            input,
        } => AudioCommand::Revoke(Grant {
            peer,
            endpoint,
            direction: if input {
                Direction::Input
            } else {
                Direction::Output
            },
        }),
        AudioCommandLine::Close { session } => AudioCommand::Close { session },
    };
    let snapshot = rshare_core::daemon_client::request_network_audio(command).await?;
    println!("{}", serde_json::to_string_pretty(&snapshot)?);
    Ok(())
}
