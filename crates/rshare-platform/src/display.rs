use anyhow::Result;
#[cfg(any(not(windows), test))]
use rshare_core::DisplayOperationStatus;
use rshare_core::{
    DisplayCaptureRequest, DisplayCaptureResult, DisplayIdentifyRequest, DisplayIdentifyResult,
    DisplaySettingsUpdateRequest, DisplaySettingsUpdateResult, LocalDisplayState,
};

#[cfg(windows)]
pub fn query_display_state() -> Result<LocalDisplayState> {
    crate::windows::query_display_state()
}

#[cfg(not(windows))]
pub fn query_display_state() -> Result<LocalDisplayState> {
    Ok(LocalDisplayState::default())
}

#[cfg(windows)]
pub fn capture_display(request: &DisplayCaptureRequest) -> Result<DisplayCaptureResult> {
    crate::windows::capture_display(request)
}

#[cfg(not(windows))]
pub fn capture_display(request: &DisplayCaptureRequest) -> Result<DisplayCaptureResult> {
    Ok(unsupported_capture(
        &request.display_id,
        "display capture is not implemented on this platform",
    ))
}

#[cfg(windows)]
pub fn identify_displays(request: &DisplayIdentifyRequest) -> Result<DisplayIdentifyResult> {
    crate::windows::identify_displays(request)
}

#[cfg(not(windows))]
pub fn identify_displays(_request: &DisplayIdentifyRequest) -> Result<DisplayIdentifyResult> {
    Ok(DisplayIdentifyResult {
        status: DisplayOperationStatus::Unsupported,
        message: Some("display identification is not implemented on this platform".to_string()),
    })
}

#[cfg(windows)]
pub fn update_display_settings(
    request: &DisplaySettingsUpdateRequest,
) -> Result<DisplaySettingsUpdateResult> {
    crate::windows::update_display_settings(request)
}

#[cfg(not(windows))]
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

#[cfg(any(not(windows), test))]
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

pub(crate) fn fit_thumbnail_size(width: u32, height: u32, max_width: u32) -> (u32, u32) {
    if width == 0 || height == 0 || max_width == 0 {
        return (0, 0);
    }

    if width <= max_width {
        return (width, height);
    }

    let scaled_width = max_width;
    let scaled_height = scale_dimension(height, max_width, width);
    (scaled_width.max(1), scaled_height.max(1))
}

pub(crate) fn clamp_identify_duration_ms(duration_ms: Option<u32>) -> u32 {
    duration_ms.unwrap_or(2500).clamp(500, 10_000)
}

fn scale_dimension(value: u32, numerator: u32, denominator: u32) -> u32 {
    ((u64::from(value) * u64::from(numerator) + u64::from(denominator / 2))
        / u64::from(denominator)) as u32
}

#[cfg(any(not(windows), test))]
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

    #[test]
    fn thumbnail_size_fits_landscape_display_to_max_width() {
        assert_eq!(fit_thumbnail_size(3840, 2160, 640), (640, 360));
    }

    #[test]
    fn thumbnail_size_fits_portrait_display_to_max_width() {
        assert_eq!(fit_thumbnail_size(1080, 1920, 480), (480, 853));
    }

    #[test]
    fn identify_duration_defaults_to_2500_ms() {
        assert_eq!(clamp_identify_duration_ms(None), 2500);
    }

    #[test]
    fn identify_duration_has_500_ms_minimum() {
        assert_eq!(clamp_identify_duration_ms(Some(100)), 500);
    }

    #[test]
    fn identify_duration_has_10000_ms_maximum() {
        assert_eq!(clamp_identify_duration_ms(Some(30_000)), 10_000);
    }
}
