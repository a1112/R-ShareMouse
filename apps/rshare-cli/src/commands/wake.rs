use anyhow::{Context, Result};
use clap::Subcommand;
use rshare_core::daemon_client;
use rshare_core::{DeviceId, WakeAttemptStatus, WakeTargetInput};
use std::net::Ipv4Addr;
use std::time::Duration;

#[derive(Subcommand)]
pub enum WakeCommandLine {
    /// List saved wake targets, including offline devices
    List,
    /// Save a standalone device or bind a R-ShareMouse peer
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        mac: String,
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        peer_id: Option<DeviceId>,
    },
    /// Update a saved target
    Edit {
        id: DeviceId,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        mac: Option<String>,
        #[arg(long, conflicts_with = "clear_ip")]
        ip: Option<Ipv4Addr>,
        #[arg(long)]
        clear_ip: bool,
        #[arg(long, conflicts_with = "standalone")]
        peer_id: Option<DeviceId>,
        #[arg(long)]
        standalone: bool,
    },
    /// Remove a saved target
    Remove { id: DeviceId },
    /// Send a magic packet and wait for confirmation
    Send { id: DeviceId },
}

pub async fn execute(command: WakeCommandLine) -> Result<i32> {
    match command {
        WakeCommandLine::List => {
            for target in daemon_client::request_wake_targets().await? {
                println!(
                    "{}  {}  {}  {}  {}",
                    target.id,
                    target.name,
                    target.mac,
                    target
                        .ipv4
                        .map(|ip| ip.to_string())
                        .unwrap_or_else(|| "-".into()),
                    target
                        .peer_id
                        .map(|id| format!("peer {id}"))
                        .unwrap_or_else(|| "standalone".into())
                );
            }
            Ok(0)
        }
        WakeCommandLine::Add {
            name,
            mac,
            ip,
            peer_id,
        } => {
            let saved = daemon_client::request_save_wake_target(WakeTargetInput {
                id: None,
                name,
                mac,
                peer_id,
                ipv4: ip,
            })
            .await?;
            println!("Saved wake target {} ({})", saved.name, saved.id);
            Ok(0)
        }
        WakeCommandLine::Edit {
            id,
            name,
            mac,
            ip,
            clear_ip,
            peer_id,
            standalone,
        } => {
            let current = daemon_client::request_wake_targets()
                .await?
                .into_iter()
                .find(|target| target.id == id)
                .context("Wake target not found")?;
            let saved = daemon_client::request_save_wake_target(WakeTargetInput {
                id: Some(id),
                name: name.unwrap_or(current.name),
                mac: mac.unwrap_or(current.mac),
                peer_id: if standalone {
                    None
                } else {
                    peer_id.or(current.peer_id)
                },
                ipv4: if clear_ip { None } else { ip.or(current.ipv4) },
            })
            .await?;
            println!("Updated wake target {} ({})", saved.name, saved.id);
            Ok(0)
        }
        WakeCommandLine::Remove { id } => {
            daemon_client::request_delete_wake_target(id).await?;
            println!("Removed wake target {id}");
            Ok(0)
        }
        WakeCommandLine::Send { id } => {
            let mut attempt = daemon_client::request_wake_target(id).await?;
            println!("{}", attempt.message);
            while attempt.status == WakeAttemptStatus::Waiting {
                tokio::time::sleep(Duration::from_secs(2)).await;
                attempt = daemon_client::request_wake_attempt(attempt.id).await?;
            }
            println!("{}", attempt.message);
            Ok(exit_code_for_status(attempt.status))
        }
    }
}

fn exit_code_for_status(status: WakeAttemptStatus) -> i32 {
    match status {
        WakeAttemptStatus::Confirmed => 0,
        WakeAttemptStatus::Unconfirmed => 2,
        WakeAttemptStatus::Failed => 1,
        WakeAttemptStatus::Waiting => unreachable!("caller must wait for a terminal status"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_failure_and_send_failure_have_distinct_exit_codes() {
        assert_eq!(exit_code_for_status(WakeAttemptStatus::Confirmed), 0);
        assert_eq!(exit_code_for_status(WakeAttemptStatus::Unconfirmed), 2);
        assert_eq!(exit_code_for_status(WakeAttemptStatus::Failed), 1);
    }
}
