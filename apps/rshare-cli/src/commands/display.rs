use anyhow::{bail, Result};
use clap::Subcommand;
use rshare_core::{
    LocalDisplayInfo, LocalDisplayState, VirtualDisplayCreateRequest,
    VirtualDisplayOperationResult, VirtualDisplayRemoveRequest, VirtualDisplaySnapshot,
    VirtualDisplayStatus,
};

const REFRESH_RATE_MATCH_TOLERANCE_MILLIHZ: u32 = 1_000;
const SUPPORTED_VIRTUAL_DISPLAY_MODES: &[(u32, u32, u32)] = &[
    (1920, 1080, 60_000),
    (1920, 1080, 144_000),
    (1920, 1080, 90_000),
    (2560, 1440, 144_000),
    (2560, 1440, 90_000),
    (2560, 1440, 60_000),
    (3840, 2160, 60_000),
    (1600, 900, 60_000),
    (1280, 720, 90_000),
    (1280, 720, 60_000),
    (1024, 768, 75_000),
    (1024, 768, 60_000),
];

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
    /// List virtual displays known to the daemon
    List,
    /// List virtual display modes supported by the bundled Windows IDD driver
    Modes,
    /// Create or retry creating a daemon-managed virtual display
    Create {
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long, default_value_t = 1920)]
        width: u32,
        #[arg(long, default_value_t = 1080)]
        height: u32,
        #[arg(long, default_value_t = 60000)]
        refresh_rate_millihz: u32,
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a daemon-managed virtual display
    Remove {
        #[arg(long, default_value = "rshare-vdisplay-1")]
        id: String,
    },
    /// Verify the active virtual display appears in the Windows display topology
    Verify {
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        width: Option<u32>,
        #[arg(long)]
        height: Option<u32>,
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
        VirtualDisplayCommand::List => {
            let displays = rshare_core::daemon_client::request_virtual_displays().await?;
            println!("{}", format_virtual_display_list(&displays));
            Ok(())
        }
        VirtualDisplayCommand::Modes => {
            println!("{}", format_supported_virtual_display_modes());
            Ok(())
        }
        VirtualDisplayCommand::Create {
            id,
            mode,
            width,
            height,
            refresh_rate_millihz,
            name,
        } => {
            let (width, height, refresh_rate_millihz) = resolve_create_virtual_display_mode(
                mode.as_deref(),
                width,
                height,
                refresh_rate_millihz,
            )?;
            let result = rshare_core::daemon_client::request_create_virtual_display(
                VirtualDisplayCreateRequest {
                    id,
                    width,
                    height,
                    refresh_rate_millihz: Some(refresh_rate_millihz),
                    name,
                },
            )
            .await?;
            println!("{}", format_virtual_display_operation(&result));
            Ok(())
        }
        VirtualDisplayCommand::Remove { id } => {
            let result = rshare_core::daemon_client::request_remove_virtual_display(
                VirtualDisplayRemoveRequest { id },
            )
            .await?;
            println!("{}", format_virtual_display_operation(&result));
            Ok(())
        }
        VirtualDisplayCommand::Verify {
            mode,
            width,
            height,
            refresh_rate_millihz,
        } => {
            let (width, height, refresh_rate_millihz) = resolve_verify_virtual_display_mode(
                mode.as_deref(),
                width,
                height,
                refresh_rate_millihz,
            )?;
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

fn format_supported_virtual_display_modes() -> String {
    let mut lines = vec!["supported virtual display modes:".to_string()];
    lines.extend(
        SUPPORTED_VIRTUAL_DISPLAY_MODES
            .iter()
            .map(|(width, height, refresh)| format!("{width}x{height}@{refresh}")),
    );
    lines.join("\n")
}

fn parse_virtual_display_mode(mode: &str) -> Result<(u32, u32, u32)> {
    let Some((resolution, refresh)) = mode.trim().split_once('@') else {
        bail!("virtual display mode must use WIDTHxHEIGHT@REFRESH_MILLIHZ");
    };
    let Some((width, height)) = resolution
        .split_once('x')
        .or_else(|| resolution.split_once('X'))
    else {
        bail!("virtual display mode must use WIDTHxHEIGHT@REFRESH_MILLIHZ");
    };

    let width = width.trim().parse::<u32>()?;
    let height = height.trim().parse::<u32>()?;
    let refresh = refresh.trim().parse::<u32>()?;
    ensure_supported_virtual_display_mode(width, height, refresh)?;
    Ok((width, height, refresh))
}

fn resolve_create_virtual_display_mode(
    mode: Option<&str>,
    width: u32,
    height: u32,
    refresh_rate_millihz: u32,
) -> Result<(u32, u32, u32)> {
    if let Some(mode) = mode {
        return parse_virtual_display_mode(mode);
    }

    ensure_supported_virtual_display_mode(width, height, refresh_rate_millihz)?;
    Ok((width, height, refresh_rate_millihz))
}

fn resolve_verify_virtual_display_mode(
    mode: Option<&str>,
    width: Option<u32>,
    height: Option<u32>,
    refresh_rate_millihz: Option<u32>,
) -> Result<(u32, u32, Option<u32>)> {
    if let Some(mode) = mode {
        let (width, height, refresh_rate_millihz) = parse_virtual_display_mode(mode)?;
        return Ok((width, height, Some(refresh_rate_millihz)));
    }

    let Some(width) = width else {
        bail!("virtual display verify requires --width and --height, or --mode WIDTHxHEIGHT@REFRESH_MILLIHZ");
    };
    let Some(height) = height else {
        bail!("virtual display verify requires --width and --height, or --mode WIDTHxHEIGHT@REFRESH_MILLIHZ");
    };
    if let Some(refresh_rate_millihz) = refresh_rate_millihz {
        ensure_supported_virtual_display_mode(width, height, refresh_rate_millihz)?;
    }
    Ok((width, height, refresh_rate_millihz))
}

fn ensure_supported_virtual_display_mode(
    width: u32,
    height: u32,
    refresh_rate_millihz: u32,
) -> Result<()> {
    if SUPPORTED_VIRTUAL_DISPLAY_MODES
        .iter()
        .any(|mode| *mode == (width, height, refresh_rate_millihz))
    {
        return Ok(());
    }

    bail!(
        "unsupported virtual display mode {width}x{height}@{refresh_rate_millihz}\n{}",
        format_supported_virtual_display_modes()
    )
}

fn format_virtual_display_list(displays: &[VirtualDisplaySnapshot]) -> String {
    if displays.is_empty() {
        return "no virtual displays reported by daemon".to_string();
    }

    displays
        .iter()
        .map(format_virtual_display_snapshot)
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_virtual_display_operation(result: &VirtualDisplayOperationResult) -> String {
    let mut lines = vec![format!("virtual display operation: {:?}", result.status)];
    if let Some(display) = result.display.as_ref() {
        lines.push(format_virtual_display_snapshot(display));
    }
    if let Some(message) = result
        .message
        .as_deref()
        .filter(|message| !message.is_empty())
    {
        lines.push(format!("message: {message}"));
    }
    lines.join("\n")
}

fn format_virtual_display_snapshot(display: &VirtualDisplaySnapshot) -> String {
    let refresh = display
        .refresh_rate_millihz
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let display_id = display.display_id.as_deref().unwrap_or("unmatched");
    let name = display.name.as_deref().unwrap_or("unnamed");
    let message = display
        .message
        .as_deref()
        .filter(|message| !message.is_empty())
        .map(|message| format!(" message=\"{message}\""))
        .unwrap_or_default();

    format!(
        "{} {:?} {}x{}@{} display_id={} name=\"{}\"{}",
        display.id,
        display.status,
        display.width,
        display.height,
        refresh,
        display_id,
        name,
        message
    )
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
            .map(|expected| {
                display
                    .refresh_rate_millihz
                    .map(|actual| refresh_rate_matches(actual, expected))
                    .unwrap_or(false)
            })
            .unwrap_or(true)
}

fn refresh_rate_matches(actual_millihz: u32, expected_millihz: u32) -> bool {
    actual_millihz.abs_diff(expected_millihz) <= REFRESH_RATE_MATCH_TOLERANCE_MILLIHZ
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

    #[test]
    fn verify_virtual_display_rejects_active_snapshot_without_system_display_id() {
        let virtual_display = VirtualDisplaySnapshot {
            id: "rshare-vdisplay-1".to_string(),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
            status: VirtualDisplayStatus::Active,
            display_id: None,
            message: None,
        };
        let result = verify_virtual_display_topology(
            &[virtual_display],
            &LocalDisplayState::default(),
            1920,
            1080,
            Some(60000),
        );

        assert!(result
            .unwrap_err()
            .to_string()
            .contains("has no Windows display id yet"));
    }

    #[test]
    fn verify_virtual_display_accepts_small_refresh_rounding_delta() {
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
                refresh_rate_millihz: Some(59940),
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

    #[test]
    fn format_virtual_display_list_includes_system_identity_and_mode() {
        let output = format_virtual_display_list(&[VirtualDisplaySnapshot {
            id: "rshare-vdisplay-1".to_string(),
            width: 2560,
            height: 1440,
            refresh_rate_millihz: Some(144000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
            status: VirtualDisplayStatus::Active,
            display_id: Some("windows-display-rshare".to_string()),
            message: None,
        }]);

        assert!(output.contains("rshare-vdisplay-1"));
        assert!(output.contains("Active"));
        assert!(output.contains("2560x1440@144000"));
        assert!(output.contains("windows-display-rshare"));
    }

    #[test]
    fn format_virtual_display_operation_includes_result_snapshot() {
        let output =
            format_virtual_display_operation(&rshare_core::VirtualDisplayOperationResult {
                status: rshare_core::VirtualDisplayOperationStatus::Created,
                display: Some(VirtualDisplaySnapshot {
                    id: "rshare-vdisplay-1".to_string(),
                    width: 1920,
                    height: 1080,
                    refresh_rate_millihz: Some(60000),
                    name: Some("R-ShareMouse Virtual Display".to_string()),
                    status: VirtualDisplayStatus::Active,
                    display_id: Some("windows-display-rshare".to_string()),
                    message: None,
                }),
                message: Some("created".to_string()),
            });

        assert!(output.contains("Created"));
        assert!(output.contains("rshare-vdisplay-1"));
        assert!(output.contains("1920x1080@60000"));
        assert!(output.contains("created"));
    }

    #[test]
    fn format_supported_virtual_display_modes_lists_driver_modes() {
        let output = format_supported_virtual_display_modes();

        assert!(output.contains("supported virtual display modes"));
        assert!(output.contains("3840x2160@60000"));
        assert!(output.contains("2560x1440@144000"));
        assert!(output.contains("1920x1080@60000"));
        assert!(output.contains("1024x768@60000"));
    }

    #[test]
    fn parse_virtual_display_mode_accepts_supported_mode_string() {
        let mode = parse_virtual_display_mode("1920x1080@60000")
            .expect("supported mode string should parse");

        assert_eq!(mode, (1920, 1080, 60_000));
    }

    #[test]
    fn parse_virtual_display_mode_rejects_invalid_or_unsupported_modes() {
        assert!(parse_virtual_display_mode("1920x1080").is_err());
        assert!(parse_virtual_display_mode("1234x567@60000").is_err());
    }
}
