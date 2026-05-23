use anyhow::Result;
use rshare_core::{
    DisplayCaptureRequest, DisplayCaptureResult, DisplayIdentifyRequest, DisplayIdentifyResult,
    DisplayOperationStatus, DisplaySettingsUpdateRequest, DisplaySettingsUpdateResult,
    LocalDisplayState,
};

pub fn query_display_state() -> Result<LocalDisplayState> {
    Ok(LocalDisplayState::default())
}

pub fn capture_display(request: &DisplayCaptureRequest) -> Result<DisplayCaptureResult> {
    Ok(unsupported_capture(
        &request.display_id,
        "display capture is not implemented on this platform",
    ))
}

pub fn identify_displays(_request: &DisplayIdentifyRequest) -> Result<DisplayIdentifyResult> {
    Ok(DisplayIdentifyResult {
        status: DisplayOperationStatus::Unsupported,
        message: Some("display identification is not implemented on this platform".to_string()),
    })
}

pub fn update_display_settings(
    request: &DisplaySettingsUpdateRequest,
) -> Result<DisplaySettingsUpdateResult> {
    if request.scale_percent.is_some() {
        return Ok(scale_requires_system_settings());
    }

    Ok(DisplaySettingsUpdateResult {
        status: DisplayOperationStatus::Unsupported,
        message: Some("display settings updates are not implemented on this platform".to_string()),
    })
}

#[cfg(windows)]
pub fn open_display_settings() -> Result<()> {
    crate::windows::open_display_settings()
}

#[cfg(target_os = "macos")]
pub fn open_display_settings() -> Result<()> {
    use anyhow::Context;
    use std::process::Command;

    Command::new("open")
        .args(["x-apple.systempreferences:com.apple.preference.displays"])
        .spawn()
        .context("Failed to open display settings")?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn open_display_settings() -> Result<()> {
    use std::process::Command;

    let commands: &[&[&str]] = &[
        &["gnome-control-center", "display"],
        &["systemsettings", "5"],
        &["xfce4-display-settings"],
        &["lxrandr"],
    ];

    for cmd in commands {
        if Command::new(cmd[0]).args(&cmd[1..]).spawn().is_ok() {
            return Ok(());
        }
    }

    anyhow::bail!("No supported display settings command found")
}

#[cfg(windows)]
pub fn get_dpi_scaling() -> f64 {
    crate::windows::get_dpi_scaling()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn get_dpi_scaling() -> f64 {
    1.0
}

fn unsupported_capture(display_id: &str, message: impl Into<String>) -> DisplayCaptureResult {
    DisplayCaptureResult {
        status: DisplayOperationStatus::Unsupported,
        display_id: display_id.to_string(),
        mime_type: None,
        width: None,
        height: None,
        bytes: Vec::new(),
        message: Some(message.into()),
    }
}

fn scale_requires_system_settings() -> DisplaySettingsUpdateResult {
    DisplaySettingsUpdateResult {
        status: DisplayOperationStatus::RequiresSystemSettings,
        message: Some("display scale changes require system display settings".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rshare_core::DisplayOperationStatus;

    #[test]
    fn unsupported_capture_result_names_display() {
        let result = unsupported_capture("display-1", "not implemented");

        assert_eq!(result.status, DisplayOperationStatus::Unsupported);
        assert_eq!(result.display_id, "display-1");
        assert!(result.bytes.is_empty());
    }

    #[test]
    fn scale_update_requires_system_settings() {
        let result = scale_requires_system_settings();

        assert_eq!(
            result.status,
            DisplayOperationStatus::RequiresSystemSettings
        );
    }
}
