use anyhow::{bail, Result};
use clap::Subcommand;
use rshare_core::{
    LocalDisplayInfo, LocalDisplayState, VirtualDisplaySnapshot, VirtualDisplayStatus,
};

#[derive(Subcommand)]
pub enum DisplayCommand {
    /// Validate virtual display visibility through daemon display state
    Virtual {
        #[command(subcommand)]
        command: VirtualDisplayCommand,
    },
}

#[derive(Subcommand)]
pub enum VirtualDisplayCommand {
    /// Verify the active virtual display appears in the Windows display topology
    Verify {
        #[arg(long)]
        width: u32,
        #[arg(long)]
        height: u32,
        #[arg(long)]
        refresh_rate_millihz: Option<u32>,
    },
}

pub async fn execute(command: DisplayCommand) -> Result<()> {
    match command {
        DisplayCommand::Virtual { command } => execute_virtual(command).await,
    }
}

async fn execute_virtual(command: VirtualDisplayCommand) -> Result<()> {
    match command {
        VirtualDisplayCommand::Verify {
            width,
            height,
            refresh_rate_millihz,
        } => {
            let virtual_displays = rshare_core::daemon_client::request_virtual_displays().await?;
            let local_controls = rshare_core::daemon_client::request_local_controls().await?;
            let summary = verify_virtual_display_topology(
                &virtual_displays,
                &local_controls.display,
                width,
                height,
                refresh_rate_millihz,
            )?;
            println!("{summary}");
            Ok(())
        }
    }
}

fn verify_virtual_display_topology(
    virtual_displays: &[VirtualDisplaySnapshot],
    local_display: &LocalDisplayState,
    width: u32,
    height: u32,
    refresh_rate_millihz: Option<u32>,
) -> Result<String> {
    let Some(virtual_display) = virtual_displays
        .iter()
        .find(|display| matches!(display.status, VirtualDisplayStatus::Active))
    else {
        bail!("no active virtual display reported by daemon");
    };

    let Some(display_id) = virtual_display.display_id.as_deref() else {
        bail!(
            "active virtual display {} has no Windows display id yet",
            virtual_display.id
        );
    };

    let Some(system_display) = local_display
        .displays
        .iter()
        .find(|display| display.display_id == display_id)
    else {
        bail!("virtual display id {display_id} was not found in daemon display topology");
    };

    if !system_display.active {
        bail!("virtual display id {display_id} is present but inactive in display topology");
    }

    if !display_matches_mode(system_display, width, height, refresh_rate_millihz) {
        bail!(
            "virtual display id {display_id} has mode {}x{}@{:?}, expected {width}x{height}@{:?}",
            system_display.width,
            system_display.height,
            system_display.refresh_rate_millihz,
            refresh_rate_millihz
        );
    }

    Ok(format!(
        "virtual display verified in system topology: {} {}x{}@{}",
        display_id,
        system_display.width,
        system_display.height,
        system_display
            .refresh_rate_millihz
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    ))
}

fn display_matches_mode(
    display: &LocalDisplayInfo,
    width: u32,
    height: u32,
    refresh_rate_millihz: Option<u32>,
) -> bool {
    display.width == width
        && display.height == height
        && refresh_rate_millihz
            .map(|expected| display.refresh_rate_millihz == Some(expected))
            .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rshare_core::{
        DisplayModeInfo, LocalDisplayInfo, LocalDisplayState, VirtualDisplaySnapshot,
        VirtualDisplayStatus,
    };

    #[test]
    fn verify_virtual_display_requires_matching_system_display_mode() {
        let virtual_display = VirtualDisplaySnapshot {
            id: "rshare-vdisplay-1".to_string(),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
            status: VirtualDisplayStatus::Active,
            display_id: Some("windows-display-rshare".to_string()),
            message: None,
        };
        let local_display = LocalDisplayState {
            displays: vec![LocalDisplayInfo {
                display_id: "windows-display-rshare".to_string(),
                width: 1920,
                height: 1080,
                refresh_rate_millihz: Some(60000),
                modes: vec![DisplayModeInfo {
                    width: 2560,
                    height: 1440,
                    refresh_rate_millihz: Some(144000),
                    orientation: Default::default(),
                    bits_per_pixel: Some(32),
                }],
                active: true,
                ..Default::default()
            }],
            ..Default::default()
        };

        let result = verify_virtual_display_topology(
            &[virtual_display],
            &local_display,
            1920,
            1080,
            Some(60000),
        );

        assert!(result.is_ok(), "{result:?}");
    }
}
