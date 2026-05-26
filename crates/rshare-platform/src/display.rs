use anyhow::Result;
use rshare_core::{
    DisplayCaptureRequest, DisplayCaptureResult, DisplayIdentifyRequest, DisplayIdentifyResult,
    DisplayModeInfo, DisplaySettingsUpdateRequest, DisplaySettingsUpdateResult,
    DisplayWriteCapabilities, LocalDisplayInfo, LocalDisplayState,
};
#[cfg(any(not(windows), test))]
use rshare_core::{DisplayOperationStatus, DisplayOrientation};

#[cfg(windows)]
pub fn query_display_state() -> Result<LocalDisplayState> {
    crate::windows::query_display_state()
}

#[cfg(all(target_os = "linux", feature = "x11"))]
pub fn query_display_state() -> Result<LocalDisplayState> {
    if is_linux_wayland_session() {
        if let Ok(state) = linux_gnome_wayland_query_display_state() {
            return Ok(state);
        }
    }
    linux_x11_query_display_state()
}

#[cfg(all(target_os = "linux", not(feature = "x11")))]
pub fn query_display_state() -> Result<LocalDisplayState> {
    if let Ok(state) = linux_gnome_wayland_query_display_state() {
        return Ok(state);
    }
    anyhow::bail!("Linux display enumeration requires the x11 feature")
}

#[cfg(all(not(windows), not(target_os = "linux")))]
pub fn query_display_state() -> Result<LocalDisplayState> {
    Ok(LocalDisplayState::default())
}

#[cfg(windows)]
pub fn capture_display(request: &DisplayCaptureRequest) -> Result<DisplayCaptureResult> {
    crate::windows::capture_display(request)
}

#[cfg(target_os = "linux")]
pub fn capture_display(request: &DisplayCaptureRequest) -> Result<DisplayCaptureResult> {
    Ok(linux_capture_display(request))
}

#[cfg(all(not(windows), not(target_os = "linux")))]
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

#[cfg(target_os = "linux")]
pub fn identify_displays(request: &DisplayIdentifyRequest) -> Result<DisplayIdentifyResult> {
    linux_identify_displays(request)
}

#[cfg(all(not(windows), not(target_os = "linux")))]
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

#[cfg(all(target_os = "linux", feature = "x11"))]
pub fn update_display_settings(
    request: &DisplaySettingsUpdateRequest,
) -> Result<DisplaySettingsUpdateResult> {
    linux_x11_update_display_settings(request)
}

#[cfg(all(not(windows), not(all(target_os = "linux", feature = "x11"))))]
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

#[cfg(all(target_os = "linux", feature = "x11"))]
fn linux_x11_query_display_state() -> Result<LocalDisplayState> {
    use std::ptr;
    use x11::{xlib, xrandr};

    unsafe {
        let display = xlib::XOpenDisplay(ptr::null());
        if display.is_null() {
            anyhow::bail!("Failed to open X11 display for XRandR enumeration");
        }
        let _guard = X11DisplayGuard(display);

        let mut event_base = 0;
        let mut error_base = 0;
        if xrandr::XRRQueryExtension(display, &mut event_base, &mut error_base) == 0 {
            anyhow::bail!("XRandR extension is not available");
        }

        let screen = xlib::XDefaultScreen(display);
        let root = xlib::XRootWindow(display, screen);
        let resources = xrandr::XRRGetScreenResourcesCurrent(display, root);
        if resources.is_null() {
            anyhow::bail!("XRandR returned no screen resources");
        }
        let _resources_guard = X11ScreenResourcesGuard(resources);
        let primary_output = xrandr::XRRGetOutputPrimary(display, root);
        let outputs =
            std::slice::from_raw_parts((*resources).outputs, positive_len((*resources).noutput));
        let mut displays = Vec::new();

        for &output in outputs {
            let output_info = xrandr::XRRGetOutputInfo(display, resources, output);
            if output_info.is_null() {
                continue;
            }
            let _output_guard = X11OutputInfoGuard(output_info);
            if (*output_info).connection != xrandr::RR_Connected as u16 || (*output_info).crtc == 0
            {
                continue;
            }

            let crtc_info = xrandr::XRRGetCrtcInfo(display, resources, (*output_info).crtc);
            if crtc_info.is_null() {
                continue;
            }
            let _crtc_guard = X11CrtcInfoGuard(crtc_info);
            if (*crtc_info).width == 0 || (*crtc_info).height == 0 {
                continue;
            }

            let output_name = x11_name((*output_info).name, (*output_info).nameLen);
            let current_mode = xrandr_mode_by_id(resources, (*crtc_info).mode);
            let modes = xrandr_output_modes(resources, output_info);
            let (dpi_x, dpi_y) = xrandr_dpi(
                (*crtc_info).width,
                (*crtc_info).height,
                (*output_info).mm_width as u32,
                (*output_info).mm_height as u32,
            );

            displays.push(LocalDisplayInfo {
                display_id: linux_display_id(&output_name, output),
                target_id: Some(output.to_string()),
                device_name: Some(output_name.clone()),
                friendly_name: Some(output_name),
                x: (*crtc_info).x,
                y: (*crtc_info).y,
                width: (*crtc_info).width,
                height: (*crtc_info).height,
                work_x: (*crtc_info).x,
                work_y: (*crtc_info).y,
                work_width: (*crtc_info).width,
                work_height: (*crtc_info).height,
                primary: primary_output == output || (primary_output == 0 && displays.is_empty()),
                orientation: xrandr_orientation((*crtc_info).rotation),
                dpi_x,
                dpi_y,
                raw_dpi_x: dpi_x,
                raw_dpi_y: dpi_y,
                refresh_rate_millihz: current_mode.and_then(xrandr_refresh_rate_millihz),
                bits_per_pixel: None,
                active: true,
                modes,
                write_capabilities: DisplayWriteCapabilities {
                    resolution: true,
                    refresh_rate: true,
                    orientation: true,
                    primary: false,
                    position: false,
                    scale: false,
                    capture: true,
                },
                ..LocalDisplayInfo::default()
            });
        }

        if displays.is_empty() {
            anyhow::bail!("XRandR returned no connected active displays");
        }

        displays.sort_by_key(|display| (!display.primary, display.x, display.y));
        let min_x = displays.iter().map(|display| display.x).min().unwrap_or(0);
        let min_y = displays.iter().map(|display| display.y).min().unwrap_or(0);
        let max_x = displays
            .iter()
            .map(|display| display.x.saturating_add(display.width as i32))
            .max()
            .unwrap_or(0);
        let max_y = displays
            .iter()
            .map(|display| display.y.saturating_add(display.height as i32))
            .max()
            .unwrap_or(0);
        let primary = displays
            .iter()
            .find(|display| display.primary)
            .unwrap_or(&displays[0]);

        Ok(LocalDisplayState {
            display_count: displays.len(),
            virtual_x: min_x,
            virtual_y: min_y,
            primary_width: primary.width,
            primary_height: primary.height,
            layout_width: max_x.saturating_sub(min_x).max(0) as u32,
            layout_height: max_y.saturating_sub(min_y).max(0) as u32,
            displays,
        })
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn linux_x11_update_display_settings(
    request: &DisplaySettingsUpdateRequest,
) -> Result<DisplaySettingsUpdateResult> {
    use std::ptr;
    use x11::{xlib, xrandr};

    if request.scale_percent.is_some() {
        return Ok(scale_requires_system_settings());
    }

    unsafe {
        let display = xlib::XOpenDisplay(ptr::null());
        if display.is_null() {
            return Ok(DisplaySettingsUpdateResult {
                status: DisplayOperationStatus::ApplyFailed,
                message: Some("Failed to open X11 display for XRandR update".to_string()),
            });
        }
        let _guard = X11DisplayGuard(display);
        let screen = xlib::XDefaultScreen(display);
        let root = xlib::XRootWindow(display, screen);
        let resources = xrandr::XRRGetScreenResources(display, root);
        if resources.is_null() {
            return Ok(DisplaySettingsUpdateResult {
                status: DisplayOperationStatus::ApplyFailed,
                message: Some("XRandR returned no screen resources".to_string()),
            });
        }
        let _resources_guard = X11ScreenResourcesGuard(resources);
        let Some(target) = find_xrandr_output(display, resources, &request.display_id) else {
            return Ok(DisplaySettingsUpdateResult {
                status: DisplayOperationStatus::InvalidDisplay,
                message: Some(format!("display '{}' was not found", request.display_id)),
            });
        };
        let _output_guard = X11OutputInfoGuard(target.output_info);
        let _crtc_guard = X11CrtcInfoGuard(target.crtc_info);

        let mode = match find_requested_xrandr_mode(resources, target.output_info, request) {
            Some(mode) => mode,
            None => {
                return Ok(DisplaySettingsUpdateResult {
                    status: DisplayOperationStatus::InvalidMode,
                    message: Some(
                        "requested display mode is not available on this output".to_string(),
                    ),
                });
            }
        };

        let mut outputs = std::slice::from_raw_parts(
            (*target.crtc_info).outputs,
            positive_len((*target.crtc_info).noutput),
        )
        .to_vec();
        if outputs.is_empty() {
            outputs.push(target.output);
        }
        let x = request.x.unwrap_or((*target.crtc_info).x);
        let y = request.y.unwrap_or((*target.crtc_info).y);
        let rotation = request
            .orientation
            .map(xrandr_rotation_from_orientation)
            .unwrap_or((*target.crtc_info).rotation);
        let status = xrandr::XRRSetCrtcConfig(
            display,
            resources,
            (*target.output_info).crtc,
            xlib::CurrentTime,
            x,
            y,
            mode.id,
            rotation,
            outputs.as_mut_ptr(),
            outputs.len() as i32,
        );
        xlib::XFlush(display);

        if status == xrandr::RRSetConfigSuccess {
            Ok(DisplaySettingsUpdateResult {
                status: DisplayOperationStatus::Success,
                message: Some("Linux XRandR display settings applied".to_string()),
            })
        } else {
            Ok(DisplaySettingsUpdateResult {
                status: DisplayOperationStatus::ApplyFailed,
                message: Some(format!(
                    "XRandR rejected the display update with status {status}"
                )),
            })
        }
    }
}

#[cfg(target_os = "linux")]
fn is_linux_wayland_session() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|value| value.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn linux_gnome_wayland_query_display_state() -> Result<LocalDisplayState> {
    use std::process::Command;

    let output = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Mutter.DisplayConfig",
            "--object-path",
            "/org/gnome/Mutter/DisplayConfig",
            "--method",
            "org.gnome.Mutter.DisplayConfig.GetCurrentState",
        ])
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "GNOME display config query failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_gnome_display_config_state(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct GnomeMonitorState {
    connector: String,
    vendor: Option<String>,
    product: Option<String>,
    serial: Option<String>,
    display_name: Option<String>,
    modes: Vec<GnomeDisplayMode>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct GnomeDisplayMode {
    width: u32,
    height: u32,
    refresh_rate_millihz: Option<u32>,
    scale_percent: Option<u32>,
    current: bool,
    preferred: bool,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct GnomeLogicalMonitorState {
    x: i32,
    y: i32,
    scale_percent: Option<u32>,
    transform: u32,
    primary: bool,
    connectors: Vec<String>,
}

#[cfg(target_os = "linux")]
fn parse_gnome_display_config_state(text: &str) -> Result<LocalDisplayState> {
    let tuple = strip_wrapped(text.trim(), '(', ')')
        .ok_or_else(|| anyhow::anyhow!("GNOME display state did not return a tuple"))?;
    let parts = split_top_level(tuple, ',');
    if parts.len() < 3 {
        anyhow::bail!("GNOME display state tuple is missing monitor lists");
    }

    let monitor_entries = strip_wrapped(parts[1].trim(), '[', ']')
        .map(split_gvariant_list)
        .unwrap_or_default();
    let logical_entries = strip_wrapped(parts[2].trim(), '[', ']')
        .map(split_gvariant_list)
        .unwrap_or_default();
    let monitors = monitor_entries
        .iter()
        .filter_map(|entry| parse_gnome_monitor(entry).ok())
        .collect::<Vec<_>>();
    let logical_monitors = logical_entries
        .iter()
        .filter_map(|entry| parse_gnome_logical_monitor(entry).ok())
        .collect::<Vec<_>>();

    if monitors.is_empty() {
        anyhow::bail!("GNOME display state did not include physical monitors");
    }

    let mut displays = Vec::new();
    for (index, monitor) in monitors.iter().enumerate() {
        let mode = monitor
            .modes
            .iter()
            .find(|mode| mode.current)
            .or_else(|| monitor.modes.iter().find(|mode| mode.preferred))
            .or_else(|| monitor.modes.first());
        let Some(mode) = mode else {
            continue;
        };
        let logical = logical_monitors.iter().find(|logical| {
            logical
                .connectors
                .iter()
                .any(|name| name == &monitor.connector)
        });
        let primary = logical.map(|logical| logical.primary).unwrap_or(index == 0);
        let scale_percent = logical
            .and_then(|logical| logical.scale_percent)
            .or(mode.scale_percent);

        displays.push(LocalDisplayInfo {
            display_id: format!("wayland-{}", monitor.connector),
            adapter_id: monitor.vendor.clone(),
            target_id: monitor.product.clone().or_else(|| monitor.serial.clone()),
            device_name: Some(monitor.connector.clone()),
            friendly_name: monitor
                .display_name
                .clone()
                .or_else(|| Some(monitor.connector.clone())),
            x: logical.map(|logical| logical.x).unwrap_or(0),
            y: logical.map(|logical| logical.y).unwrap_or(0),
            width: mode.width,
            height: mode.height,
            work_x: logical.map(|logical| logical.x).unwrap_or(0),
            work_y: logical.map(|logical| logical.y).unwrap_or(0),
            work_width: mode.width,
            work_height: mode.height,
            primary,
            orientation: logical
                .map(|logical| gnome_transform_orientation(logical.transform))
                .unwrap_or_default(),
            scale_percent,
            refresh_rate_millihz: mode.refresh_rate_millihz,
            active: true,
            modes: gnome_display_modes(&monitor.modes),
            write_capabilities: DisplayWriteCapabilities {
                capture: true,
                ..DisplayWriteCapabilities::default()
            },
            ..LocalDisplayInfo::default()
        });
    }

    if displays.is_empty() {
        anyhow::bail!("GNOME display state did not include active modes");
    }

    displays.sort_by_key(|display| (!display.primary, display.x, display.y));
    Ok(display_state_from_displays(displays))
}

#[cfg(target_os = "linux")]
fn gnome_display_modes(modes: &[GnomeDisplayMode]) -> Vec<DisplayModeInfo> {
    let mut result = Vec::new();
    for mode in modes {
        let display_mode = DisplayModeInfo {
            width: mode.width,
            height: mode.height,
            refresh_rate_millihz: mode.refresh_rate_millihz,
            orientation: DisplayOrientation::Landscape,
            bits_per_pixel: None,
        };
        if !result.iter().any(|existing: &DisplayModeInfo| {
            existing.width == display_mode.width
                && existing.height == display_mode.height
                && existing.refresh_rate_millihz == display_mode.refresh_rate_millihz
                && existing.orientation == display_mode.orientation
                && existing.bits_per_pixel == display_mode.bits_per_pixel
        }) {
            result.push(display_mode);
        }
    }
    result
}

#[cfg(target_os = "linux")]
fn parse_gnome_monitor(entry: &str) -> Result<GnomeMonitorState> {
    let parts = split_top_level(
        strip_wrapped(entry.trim(), '(', ')').ok_or_else(|| anyhow::anyhow!("monitor tuple"))?,
        ',',
    );
    if parts.len() < 2 {
        anyhow::bail!("monitor entry is incomplete");
    }
    let identity = parse_quoted_strings(parts[0]);
    let connector = identity
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("monitor connector is missing"))?;
    let modes = strip_wrapped(parts[1].trim(), '[', ']')
        .map(split_gvariant_list)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| parse_gnome_mode(entry).ok())
        .collect::<Vec<_>>();
    let display_name = parts
        .get(2)
        .and_then(|props| gvariant_property_string(props, "display-name"));

    Ok(GnomeMonitorState {
        connector,
        vendor: identity.get(1).cloned(),
        product: identity.get(2).cloned(),
        serial: identity.get(3).cloned(),
        display_name,
        modes,
    })
}

#[cfg(target_os = "linux")]
fn parse_gnome_mode(entry: &str) -> Result<GnomeDisplayMode> {
    let parts = split_top_level(
        strip_wrapped(entry.trim(), '(', ')').ok_or_else(|| anyhow::anyhow!("mode tuple"))?,
        ',',
    );
    if parts.len() < 5 {
        anyhow::bail!("mode entry is incomplete");
    }
    Ok(GnomeDisplayMode {
        width: parse_gvariant_u32(parts[1])?,
        height: parse_gvariant_u32(parts[2])?,
        refresh_rate_millihz: parse_gvariant_f64(parts[3])
            .ok()
            .map(|value| (value * 1000.0).round() as u32)
            .filter(|value| *value > 0),
        scale_percent: parse_gvariant_f64(parts[4])
            .ok()
            .map(|value| (value * 100.0).round() as u32)
            .filter(|value| *value > 0),
        current: entry.contains("'is-current': <true>"),
        preferred: entry.contains("'is-preferred': <true>"),
    })
}

#[cfg(target_os = "linux")]
fn parse_gnome_logical_monitor(entry: &str) -> Result<GnomeLogicalMonitorState> {
    let parts = split_top_level(
        strip_wrapped(entry.trim(), '(', ')')
            .ok_or_else(|| anyhow::anyhow!("logical monitor tuple"))?,
        ',',
    );
    if parts.len() < 6 {
        anyhow::bail!("logical monitor entry is incomplete");
    }
    let identity_values = parse_quoted_strings(parts[5]);
    let connectors = identity_values
        .chunks(4)
        .filter_map(|chunk| chunk.first().cloned())
        .collect::<Vec<_>>();
    Ok(GnomeLogicalMonitorState {
        x: parse_gvariant_i32(parts[0])?,
        y: parse_gvariant_i32(parts[1])?,
        scale_percent: parse_gvariant_f64(parts[2])
            .ok()
            .map(|value| (value * 100.0).round() as u32)
            .filter(|value| *value > 0),
        transform: parse_gvariant_u32(parts[3])?,
        primary: parts[4].trim() == "true",
        connectors,
    })
}

#[cfg(target_os = "linux")]
fn display_state_from_displays(displays: Vec<LocalDisplayInfo>) -> LocalDisplayState {
    let min_x = displays.iter().map(|display| display.x).min().unwrap_or(0);
    let min_y = displays.iter().map(|display| display.y).min().unwrap_or(0);
    let max_x = displays
        .iter()
        .map(|display| display.x.saturating_add(display.width as i32))
        .max()
        .unwrap_or(0);
    let max_y = displays
        .iter()
        .map(|display| display.y.saturating_add(display.height as i32))
        .max()
        .unwrap_or(0);
    let primary = displays
        .iter()
        .find(|display| display.primary)
        .unwrap_or(&displays[0]);

    LocalDisplayState {
        display_count: displays.len(),
        virtual_x: min_x,
        virtual_y: min_y,
        primary_width: primary.width,
        primary_height: primary.height,
        layout_width: max_x.saturating_sub(min_x).max(0) as u32,
        layout_height: max_y.saturating_sub(min_y).max(0) as u32,
        displays,
    }
}

#[cfg(target_os = "linux")]
fn strip_wrapped(value: &str, left: char, right: char) -> Option<&str> {
    let value = value.trim();
    value
        .strip_prefix(left)
        .and_then(|value| value.strip_suffix(right))
}

#[cfg(target_os = "linux")]
fn split_gvariant_list(value: &str) -> Vec<&str> {
    split_top_level(value, ',')
        .into_iter()
        .filter(|entry| !entry.trim().is_empty())
        .collect()
}

#[cfg(target_os = "linux")]
fn split_top_level(value: &str, delimiter: char) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut paren = 0_i32;
    let mut bracket = 0_i32;
    let mut brace = 0_i32;
    let mut angle = 0_i32;
    let mut in_string = false;
    let mut previous = '\0';

    for (index, ch) in value.char_indices() {
        if in_string {
            if ch == '\'' && previous != '\\' {
                in_string = false;
            }
            previous = ch;
            continue;
        }

        match ch {
            '\'' => in_string = true,
            '(' => paren += 1,
            ')' => paren -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            '<' => angle += 1,
            '>' => angle -= 1,
            _ if ch == delimiter && paren == 0 && bracket == 0 && brace == 0 && angle == 0 => {
                result.push(value[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
        previous = ch;
    }
    result.push(value[start..].trim());
    result
}

#[cfg(target_os = "linux")]
fn parse_quoted_strings(value: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut in_string = false;
    let mut start = 0;
    let mut previous = '\0';
    for (index, ch) in value.char_indices() {
        if in_string {
            if ch == '\'' && previous != '\\' {
                result.push(value[start..index].replace("\\'", "'"));
                in_string = false;
            }
        } else if ch == '\'' {
            in_string = true;
            start = index + ch.len_utf8();
        }
        previous = ch;
    }
    result
}

#[cfg(target_os = "linux")]
fn gvariant_property_string(value: &str, key: &str) -> Option<String> {
    let marker = format!("'{key}': <'");
    let start = value.find(&marker)? + marker.len();
    let rest = &value[start..];
    let end = rest.find("'>")?;
    Some(rest[..end].to_string())
}

#[cfg(target_os = "linux")]
fn parse_gvariant_i32(value: &str) -> Result<i32> {
    parse_gvariant_number(value)?
        .parse::<i32>()
        .map_err(Into::into)
}

#[cfg(target_os = "linux")]
fn parse_gvariant_u32(value: &str) -> Result<u32> {
    parse_gvariant_number(value)?
        .parse::<u32>()
        .map_err(Into::into)
}

#[cfg(target_os = "linux")]
fn parse_gvariant_f64(value: &str) -> Result<f64> {
    parse_gvariant_number(value)?
        .parse::<f64>()
        .map_err(Into::into)
}

#[cfg(target_os = "linux")]
fn parse_gvariant_number(value: &str) -> Result<&str> {
    value
        .split_whitespace()
        .last()
        .ok_or_else(|| anyhow::anyhow!("missing numeric value"))
}

#[cfg(target_os = "linux")]
fn gnome_transform_orientation(transform: u32) -> DisplayOrientation {
    match transform {
        1 => DisplayOrientation::Portrait,
        2 => DisplayOrientation::LandscapeFlipped,
        3 => DisplayOrientation::PortraitFlipped,
        _ => DisplayOrientation::Landscape,
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
struct X11DisplayGuard(*mut x11::xlib::Display);

#[cfg(all(target_os = "linux", feature = "x11"))]
impl Drop for X11DisplayGuard {
    fn drop(&mut self) {
        unsafe {
            x11::xlib::XCloseDisplay(self.0);
        }
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
struct X11ScreenResourcesGuard(*mut x11::xrandr::XRRScreenResources);

#[cfg(all(target_os = "linux", feature = "x11"))]
impl Drop for X11ScreenResourcesGuard {
    fn drop(&mut self) {
        unsafe {
            x11::xrandr::XRRFreeScreenResources(self.0);
        }
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
struct X11OutputInfoGuard(*mut x11::xrandr::XRROutputInfo);

#[cfg(all(target_os = "linux", feature = "x11"))]
impl Drop for X11OutputInfoGuard {
    fn drop(&mut self) {
        unsafe {
            x11::xrandr::XRRFreeOutputInfo(self.0);
        }
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
struct X11CrtcInfoGuard(*mut x11::xrandr::XRRCrtcInfo);

#[cfg(all(target_os = "linux", feature = "x11"))]
impl Drop for X11CrtcInfoGuard {
    fn drop(&mut self) {
        unsafe {
            x11::xrandr::XRRFreeCrtcInfo(self.0);
        }
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
struct XrandrOutputTarget {
    output: x11::xrandr::RROutput,
    output_info: *mut x11::xrandr::XRROutputInfo,
    crtc_info: *mut x11::xrandr::XRRCrtcInfo,
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn positive_len(value: i32) -> usize {
    usize::try_from(value.max(0)).unwrap_or(0)
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn x11_name(name: *const std::os::raw::c_char, len: i32) -> String {
    if name.is_null() || len <= 0 {
        return "Unknown Display".to_string();
    }
    let bytes = unsafe { std::slice::from_raw_parts(name as *const u8, positive_len(len)) };
    String::from_utf8_lossy(bytes).trim().to_string()
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn linux_display_id(output_name: &str, output: x11::xrandr::RROutput) -> String {
    let clean_name = output_name.trim();
    if clean_name.is_empty() || clean_name == "Unknown Display" {
        format!("x11-output-{output}")
    } else {
        format!("x11-{clean_name}")
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
unsafe fn xrandr_mode_by_id(
    resources: *mut x11::xrandr::XRRScreenResources,
    mode_id: x11::xrandr::RRMode,
) -> Option<&'static x11::xrandr::XRRModeInfo> {
    if resources.is_null() || mode_id == 0 {
        return None;
    }
    std::slice::from_raw_parts((*resources).modes, positive_len((*resources).nmode))
        .iter()
        .find(|mode| mode.id == mode_id)
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn xrandr_refresh_rate_millihz(mode: &x11::xrandr::XRRModeInfo) -> Option<u32> {
    let total = u64::from(mode.hTotal).checked_mul(u64::from(mode.vTotal))?;
    if mode.dotClock == 0 || total == 0 {
        return None;
    }
    let millihz = (u64::from(mode.dotClock) * 1000 + total / 2) / total;
    u32::try_from(millihz).ok().filter(|value| *value > 0)
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn xrandr_display_mode(mode: &x11::xrandr::XRRModeInfo) -> DisplayModeInfo {
    DisplayModeInfo {
        width: mode.width,
        height: mode.height,
        refresh_rate_millihz: xrandr_refresh_rate_millihz(mode),
        orientation: DisplayOrientation::Landscape,
        bits_per_pixel: None,
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
unsafe fn xrandr_output_modes(
    resources: *mut x11::xrandr::XRRScreenResources,
    output_info: *mut x11::xrandr::XRROutputInfo,
) -> Vec<DisplayModeInfo> {
    if output_info.is_null() {
        return Vec::new();
    }
    let mut modes = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mode_ids =
        std::slice::from_raw_parts((*output_info).modes, positive_len((*output_info).nmode));
    for &mode_id in mode_ids {
        let Some(mode) = xrandr_mode_by_id(resources, mode_id) else {
            continue;
        };
        let mode = xrandr_display_mode(mode);
        let key = (mode.width, mode.height, mode.refresh_rate_millihz);
        if seen.insert(key) {
            modes.push(mode);
        }
    }
    modes.sort_by_key(|mode| {
        (
            mode.width,
            mode.height,
            mode.refresh_rate_millihz.unwrap_or(0),
        )
    });
    modes
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn xrandr_dpi(
    width: u32,
    height: u32,
    millimeters_width: u32,
    millimeters_height: u32,
) -> (Option<u32>, Option<u32>) {
    fn one_axis(pixels: u32, millimeters: u32) -> Option<u32> {
        if pixels == 0 || millimeters == 0 {
            return None;
        }
        Some(
            ((u64::from(pixels) * 254 + u64::from(millimeters) * 5) / (u64::from(millimeters) * 10))
                as u32,
        )
    }

    (
        one_axis(width, millimeters_width),
        one_axis(height, millimeters_height),
    )
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn xrandr_orientation(rotation: x11::xrandr::Rotation) -> DisplayOrientation {
    let rotation = i32::from(rotation);
    if rotation & x11::xrandr::RR_Rotate_90 != 0 {
        DisplayOrientation::Portrait
    } else if rotation & x11::xrandr::RR_Rotate_180 != 0 {
        DisplayOrientation::LandscapeFlipped
    } else if rotation & x11::xrandr::RR_Rotate_270 != 0 {
        DisplayOrientation::PortraitFlipped
    } else {
        DisplayOrientation::Landscape
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
fn xrandr_rotation_from_orientation(orientation: DisplayOrientation) -> x11::xrandr::Rotation {
    match orientation {
        DisplayOrientation::Portrait => x11::xrandr::RR_Rotate_90 as x11::xrandr::Rotation,
        DisplayOrientation::LandscapeFlipped => x11::xrandr::RR_Rotate_180 as x11::xrandr::Rotation,
        DisplayOrientation::PortraitFlipped => x11::xrandr::RR_Rotate_270 as x11::xrandr::Rotation,
        DisplayOrientation::Landscape => x11::xrandr::RR_Rotate_0 as x11::xrandr::Rotation,
    }
}

#[cfg(all(target_os = "linux", feature = "x11"))]
unsafe fn find_xrandr_output(
    display: *mut x11::xlib::Display,
    resources: *mut x11::xrandr::XRRScreenResources,
    display_id: &str,
) -> Option<XrandrOutputTarget> {
    let outputs =
        std::slice::from_raw_parts((*resources).outputs, positive_len((*resources).noutput));
    for &output in outputs {
        let output_info = x11::xrandr::XRRGetOutputInfo(display, resources, output);
        if output_info.is_null() {
            continue;
        }
        if (*output_info).connection != x11::xrandr::RR_Connected as u16 || (*output_info).crtc == 0
        {
            x11::xrandr::XRRFreeOutputInfo(output_info);
            continue;
        }
        let output_name = x11_name((*output_info).name, (*output_info).nameLen);
        let id = linux_display_id(&output_name, output);
        if id != display_id && output_name != display_id {
            x11::xrandr::XRRFreeOutputInfo(output_info);
            continue;
        }

        let crtc_info = x11::xrandr::XRRGetCrtcInfo(display, resources, (*output_info).crtc);
        if crtc_info.is_null() {
            x11::xrandr::XRRFreeOutputInfo(output_info);
            continue;
        }

        return Some(XrandrOutputTarget {
            output,
            output_info,
            crtc_info,
        });
    }
    None
}

#[cfg(all(target_os = "linux", feature = "x11"))]
unsafe fn find_requested_xrandr_mode(
    resources: *mut x11::xrandr::XRRScreenResources,
    output_info: *mut x11::xrandr::XRROutputInfo,
    request: &DisplaySettingsUpdateRequest,
) -> Option<&'static x11::xrandr::XRRModeInfo> {
    let mode_ids =
        std::slice::from_raw_parts((*output_info).modes, positive_len((*output_info).nmode));
    let requested_width = request.width;
    let requested_height = request.height;
    let requested_refresh = request.refresh_rate_millihz;

    mode_ids
        .iter()
        .filter_map(|&mode_id| xrandr_mode_by_id(resources, mode_id))
        .find(|mode| {
            if requested_width.is_some_and(|width| width != mode.width) {
                return false;
            }
            if requested_height.is_some_and(|height| height != mode.height) {
                return false;
            }
            if let Some(refresh) = requested_refresh {
                let Some(mode_refresh) = xrandr_refresh_rate_millihz(mode) else {
                    return false;
                };
                return mode_refresh.abs_diff(refresh) <= 50;
            }
            true
        })
}

#[cfg(target_os = "linux")]
fn linux_capture_display(request: &DisplayCaptureRequest) -> DisplayCaptureResult {
    let state = match query_display_state() {
        Ok(state) => state,
        Err(error) => {
            return DisplayCaptureResult {
                status: DisplayOperationStatus::ApplyFailed,
                display_id: request.display_id.clone(),
                mime_type: None,
                width: None,
                height: None,
                bytes: Vec::new(),
                message: Some(format!(
                    "Display enumeration failed before capture: {error}"
                )),
            };
        }
    };
    let display = state
        .displays
        .iter()
        .find(|display| display.display_id == request.display_id)
        .or_else(|| {
            state
                .displays
                .iter()
                .find(|display| display.primary && request.display_id == "primary")
        });
    let Some(display) = display else {
        return DisplayCaptureResult {
            status: DisplayOperationStatus::InvalidDisplay,
            display_id: request.display_id.clone(),
            mime_type: None,
            width: None,
            height: None,
            bytes: Vec::new(),
            message: Some(format!("display '{}' was not found", request.display_id)),
        };
    };

    let shell_result = linux_gnome_shell_capture_area(display);
    if shell_result.status == DisplayOperationStatus::Success {
        return shell_result;
    }
    if matches!(
        shell_result.status,
        DisplayOperationStatus::PermissionDenied
            | DisplayOperationStatus::Unsupported
            | DisplayOperationStatus::ApplyFailed
    ) {
        return linux_portal_capture_display_via_screencast(
            display,
            shell_result.message.as_deref(),
            request.max_width,
        );
    }
    shell_result
}

#[cfg(target_os = "linux")]
fn linux_identify_displays(request: &DisplayIdentifyRequest) -> Result<DisplayIdentifyResult> {
    use std::process::Command;

    let state = match query_display_state() {
        Ok(state) => state,
        Err(error) => {
            return Ok(DisplayIdentifyResult {
                status: DisplayOperationStatus::ApplyFailed,
                message: Some(format!(
                    "Display enumeration failed before identification: {error}"
                )),
            });
        }
    };
    if state.displays.is_empty() {
        return Ok(DisplayIdentifyResult {
            status: DisplayOperationStatus::InvalidDisplay,
            message: Some("no active displays are available to identify".to_string()),
        });
    }

    let duration_ms = clamp_identify_duration_ms(request.duration_ms);
    let lines = state
        .displays
        .iter()
        .enumerate()
        .map(|(index, display)| {
            let name = display
                .friendly_name
                .as_deref()
                .or(display.device_name.as_deref())
                .unwrap_or(display.display_id.as_str());
            format!(
                "{}. {} - {}x{} @ {},{}",
                index + 1,
                name,
                display.width,
                display.height,
                display.x,
                display.y
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let seconds = ((duration_ms + 999) / 1000).max(1).to_string();
    if Command::new("zenity")
        .args([
            "--info",
            "--no-wrap",
            "--title",
            "R-ShareMouse display identification",
            "--timeout",
            &seconds,
            "--text",
            &lines,
        ])
        .spawn()
        .is_ok()
    {
        return Ok(DisplayIdentifyResult {
            status: DisplayOperationStatus::Success,
            message: Some(format!(
                "identifying {} display(s) for {duration_ms} ms",
                state.displays.len()
            )),
        });
    }

    if Command::new("notify-send")
        .args([
            "--expire-time",
            &duration_ms.to_string(),
            "R-ShareMouse displays",
            &lines,
        ])
        .spawn()
        .is_ok()
    {
        return Ok(DisplayIdentifyResult {
            status: DisplayOperationStatus::Success,
            message: Some(format!(
                "display identification notification shown for {} display(s)",
                state.displays.len()
            )),
        });
    }

    Ok(DisplayIdentifyResult {
        status: DisplayOperationStatus::Unsupported,
        message: Some("display identification requires zenity or notify-send on Linux".to_string()),
    })
}

#[cfg(target_os = "linux")]
fn linux_gnome_shell_capture_area(display: &LocalDisplayInfo) -> DisplayCaptureResult {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let capture_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let file_path = std::env::temp_dir().join(format!(
        "rshare-display-capture-{}-{capture_id}.png",
        std::process::id()
    ));
    let file_arg = file_path.to_string_lossy().to_string();
    let x = display.x.to_string();
    let y = display.y.to_string();
    let width = gnome_screenshot_dimension(display.width, display.scale_percent).to_string();
    let height = gnome_screenshot_dimension(display.height, display.scale_percent).to_string();
    let output = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell.Screenshot",
            "--object-path",
            "/org/gnome/Shell/Screenshot",
            "--method",
            "org.gnome.Shell.Screenshot.ScreenshotArea",
            &x,
            &y,
            &width,
            &height,
            "false",
            &file_arg,
        ])
        .output();

    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return DisplayCaptureResult {
                status: DisplayOperationStatus::Unsupported,
                display_id: display.display_id.clone(),
                mime_type: None,
                width: None,
                height: None,
                bytes: Vec::new(),
                message: Some(format!(
                    "Failed to invoke GNOME screenshot service: {error}"
                )),
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let status = if stderr.contains("AccessDenied") || stderr.contains("not allowed") {
            DisplayOperationStatus::PermissionDenied
        } else {
            DisplayOperationStatus::ApplyFailed
        };
        let _ = std::fs::remove_file(&file_path);
        return DisplayCaptureResult {
            status,
            display_id: display.display_id.clone(),
            mime_type: None,
            width: None,
            height: None,
            bytes: Vec::new(),
            message: Some(format!("GNOME screenshot capture failed: {stderr}")),
        };
    }

    match std::fs::read(&file_path) {
        Ok(bytes) => {
            let _ = std::fs::remove_file(&file_path);
            DisplayCaptureResult {
                status: DisplayOperationStatus::Success,
                display_id: display.display_id.clone(),
                mime_type: Some("image/png".to_string()),
                width: Some(gnome_screenshot_dimension(
                    display.width,
                    display.scale_percent,
                )),
                height: Some(gnome_screenshot_dimension(
                    display.height,
                    display.scale_percent,
                )),
                bytes,
                message: Some("Display screenshot captured".to_string()),
            }
        }
        Err(error) => DisplayCaptureResult {
            status: DisplayOperationStatus::ApplyFailed,
            display_id: display.display_id.clone(),
            mime_type: None,
            width: None,
            height: None,
            bytes: Vec::new(),
            message: Some(format!(
                "GNOME screenshot service did not write capture: {error}"
            )),
        },
    }
}

#[cfg(target_os = "linux")]
fn linux_portal_capture_display_via_screencast(
    display: &LocalDisplayInfo,
    previous_error: Option<&str>,
    max_width: Option<u32>,
) -> DisplayCaptureResult {
    use std::process::Command;

    let output = Command::new("python3")
        .args([
            "-c",
            LINUX_PORTAL_SCREENCAST_CAPTURE_SCRIPT,
            &max_width.unwrap_or(0).to_string(),
        ])
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return linux_wayland_capture_requires_screencast(
                display,
                Some(&format!(
                    "Failed to invoke xdg-desktop-portal ScreenCast helper: {error}"
                )),
                previous_error,
            );
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let status = if stderr.contains("denied")
            || stderr.contains("cancelled")
            || stderr.contains("timed out")
        {
            DisplayOperationStatus::PermissionDenied
        } else {
            DisplayOperationStatus::ApplyFailed
        };
        let mut message = if stderr.is_empty() {
            "xdg-desktop-portal ScreenCast capture failed".to_string()
        } else {
            format!("xdg-desktop-portal ScreenCast capture failed: {stderr}")
        };
        if let Some(previous_error) = previous_error {
            message.push_str(" GNOME Shell capture failed first: ");
            message.push_str(previous_error);
        }
        return DisplayCaptureResult {
            status,
            display_id: display.display_id.clone(),
            mime_type: None,
            width: None,
            height: None,
            bytes: Vec::new(),
            message: Some(message),
        };
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        return DisplayCaptureResult {
            status: DisplayOperationStatus::ApplyFailed,
            display_id: display.display_id.clone(),
            mime_type: None,
            width: None,
            height: None,
            bytes: Vec::new(),
            message: Some(
                "xdg-desktop-portal ScreenCast helper returned no capture path".to_string(),
            ),
        };
    }

    match std::fs::read(&path) {
        Ok(bytes) => {
            let _ = std::fs::remove_file(&path);
            let dimensions = png_dimensions(&bytes);
            DisplayCaptureResult {
                status: DisplayOperationStatus::Success,
                display_id: display.display_id.clone(),
                mime_type: Some("image/png".to_string()),
                width: dimensions.map(|(width, _)| width),
                height: dimensions.map(|(_, height)| height),
                bytes,
                message: Some(
                    "Display texture captured via xdg-desktop-portal ScreenCast".to_string(),
                ),
            }
        }
        Err(error) => DisplayCaptureResult {
            status: DisplayOperationStatus::ApplyFailed,
            display_id: display.display_id.clone(),
            mime_type: None,
            width: None,
            height: None,
            bytes: Vec::new(),
            message: Some(format!(
                "ScreenCast capture file could not be read: {error}"
            )),
        },
    }
}

#[cfg(target_os = "linux")]
fn linux_wayland_capture_requires_screencast(
    display: &LocalDisplayInfo,
    screencast_error: Option<&str>,
    previous_error: Option<&str>,
) -> DisplayCaptureResult {
    let mut message = "Automatic display texture capture requires xdg-desktop-portal ScreenCast/PipeWire authorization on Linux Wayland; the manual Screenshot portal is intentionally not used.".to_string();
    if let Some(screencast_error) = screencast_error {
        message.push_str(" ScreenCast helper failed: ");
        message.push_str(screencast_error);
    }
    if let Some(previous_error) = previous_error {
        message.push_str(" GNOME Shell capture failed first: ");
        message.push_str(previous_error);
    }

    DisplayCaptureResult {
        status: DisplayOperationStatus::PermissionDenied,
        display_id: display.display_id.clone(),
        mime_type: None,
        width: None,
        height: None,
        bytes: Vec::new(),
        message: Some(message),
    }
}

#[cfg(target_os = "linux")]
const LINUX_PORTAL_SCREENCAST_CAPTURE_SCRIPT: &str = r#"
import gi
import os
import subprocess
import sys
import tempfile

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
loop = GLib.MainLoop()
unique_name = bus.get_unique_name().replace(":", "").replace(".", "_")
responses = {}
session_handle = None

def request_path(token):
    return f"/org/freedesktop/portal/desktop/request/{unique_name}/{token}"

def on_response(connection, sender_name, object_path, interface_name, signal_name, parameters, user_data):
    response, results = parameters.unpack()
    responses[object_path] = (response, results)
    loop.quit()

def wait_response(path, timeout_seconds):
    if path not in responses:
        source_id = GLib.timeout_add_seconds(timeout_seconds, lambda: (loop.quit(), False)[1])
        loop.run()
        if GLib.main_context_default().find_source_by_id(source_id) is not None:
            GLib.source_remove(source_id)
    if path not in responses:
        raise RuntimeError("portal response timed out")
    response, results = responses.pop(path)
    if response != 0:
        raise RuntimeError(f"portal denied or cancelled: {response}")
    return results

def call_request(method, signature, value, token, timeout_seconds):
    path = request_path(token)
    sub_id = bus.signal_subscribe(
        "org.freedesktop.portal.Desktop",
        "org.freedesktop.portal.Request",
        "Response",
        path,
        None,
        Gio.DBusSignalFlags.NONE,
        on_response,
        None,
    )
    try:
        bus.call_sync(
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.ScreenCast",
            method,
            GLib.Variant(signature, value),
            GLib.VariantType.new("(o)"),
            Gio.DBusCallFlags.NONE,
            timeout_seconds * 1000,
            None,
        )
        return wait_response(path, timeout_seconds)
    finally:
        bus.signal_unsubscribe(sub_id)

def close_session():
    if not session_handle:
        return
    try:
        bus.call_sync(
            "org.freedesktop.portal.Desktop",
            session_handle,
            "org.freedesktop.portal.Session",
            "Close",
            GLib.Variant("()", ()),
            None,
            Gio.DBusCallFlags.NONE,
            1000,
            None,
        )
    except Exception:
        pass

try:
    try:
        max_width = int(sys.argv[1]) if len(sys.argv) > 1 else 0
    except Exception:
        max_width = 0
    suffix = str(os.getpid())
    create_token = "rshare_create_" + suffix
    session_token = "rshare_session_" + suffix
    created = call_request(
        "CreateSession",
        "(a{sv})",
        ({
            "handle_token": GLib.Variant("s", create_token),
            "session_handle_token": GLib.Variant("s", session_token),
        },),
        create_token,
        10,
    )
    session_handle = created["session_handle"]
    select_token = "rshare_select_" + suffix
    call_request(
        "SelectSources",
        "(oa{sv})",
        (session_handle, {
            "handle_token": GLib.Variant("s", select_token),
            "types": GLib.Variant("u", 1),
            "multiple": GLib.Variant("b", False),
            "cursor_mode": GLib.Variant("u", 2),
        }),
        select_token,
        30,
    )
    start_token = "rshare_start_" + suffix
    started = call_request(
        "Start",
        "(osa{sv})",
        (session_handle, "", {"handle_token": GLib.Variant("s", start_token)}),
        start_token,
        90,
    )
    streams = started.get("streams")
    if not streams:
        raise RuntimeError("ScreenCast portal returned no streams")
    node_id, properties = streams[0]
    stream_width = 0
    stream_height = 0
    size = properties.get("size")
    if size and len(size) == 2:
        stream_width = int(size[0])
        stream_height = int(size[1])

    fd_variant, fd_list = bus.call_with_unix_fd_list_sync(
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.ScreenCast",
        "OpenPipeWireRemote",
        GLib.Variant("(oa{sv})", (session_handle, {})),
        GLib.VariantType.new("(h)"),
        Gio.DBusCallFlags.NONE,
        10000,
        None,
        None,
    )
    fd = fd_list.get(fd_variant.unpack()[0])
    output = tempfile.NamedTemporaryFile(prefix="rshare-display-screencast-", suffix=".png", delete=False)
    output.close()
    pipeline = [
        "gst-launch-1.0",
        "-q",
        "pipewiresrc",
        f"fd={fd}",
        f"path={node_id}",
        "num-buffers=1",
        "do-timestamp=true",
        "!",
        "videoconvert",
        "!",
    ]
    if max_width > 0 and stream_width > max_width and stream_height > 0:
        scaled_height = max(1, round(stream_height * max_width / stream_width))
        pipeline.extend([
            "videoscale",
            "!",
            f"video/x-raw,width={max_width},height={scaled_height}",
            "!",
        ])
    pipeline.extend(["pngenc", "!", "filesink", f"location={output.name}"])
    result = subprocess.run(
        pipeline,
        pass_fds=(fd,),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=20,
    )
    if result.returncode != 0:
        stderr = result.stderr.decode(errors="replace").strip()
        raise RuntimeError(f"gst-launch failed: {stderr}")
    print(output.name)
except Exception as exc:
    print(str(exc), file=sys.stderr)
    sys.exit(2)
finally:
    close_session()
"#;

#[cfg(target_os = "linux")]
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

#[cfg(target_os = "linux")]
fn gnome_screenshot_dimension(value: u32, scale_percent: Option<u32>) -> u32 {
    match scale_percent {
        Some(125) => scale_dimension(value, 4, 5).max(1),
        Some(133) => scale_dimension(value, 3, 4).max(1),
        Some(150) => scale_dimension(value, 2, 3).max(1),
        Some(175) => scale_dimension(value, 4, 7).max(1),
        Some(200) => scale_dimension(value, 1, 2).max(1),
        Some(percent) if percent > 0 && percent != 100 => {
            scale_dimension(value, 100, percent).max(1)
        }
        _ => value,
    }
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

#[cfg(any(all(not(windows), not(target_os = "linux")), test))]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn gnome_screenshot_dimension_uses_logical_scaled_size() {
        assert_eq!(gnome_screenshot_dimension(2560, Some(133)), 1920);
        assert_eq!(gnome_screenshot_dimension(1600, Some(133)), 1200);
        assert_eq!(gnome_screenshot_dimension(3840, Some(200)), 1920);
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

    #[cfg(all(target_os = "linux", feature = "x11"))]
    #[test]
    fn linux_xrandr_refresh_rate_uses_mode_totals() {
        let mode = x11::xrandr::XRRModeInfo {
            id: 42,
            width: 1920,
            height: 1080,
            dotClock: 148_500_000,
            hSyncStart: 2008,
            hSyncEnd: 2052,
            hTotal: 2200,
            hSkew: 0,
            vSyncStart: 1084,
            vSyncEnd: 1089,
            vTotal: 1125,
            name: std::ptr::null_mut(),
            nameLength: 0,
            modeFlags: 0,
        };

        assert_eq!(xrandr_refresh_rate_millihz(&mode), Some(60_000));
    }

    #[cfg(all(target_os = "linux", feature = "x11"))]
    #[test]
    fn linux_xrandr_dpi_uses_physical_size() {
        assert_eq!(xrandr_dpi(1920, 1080, 344, 194), (Some(142), Some(141)));
    }

    #[cfg(all(target_os = "linux", feature = "x11"))]
    #[test]
    fn linux_display_ids_are_stable_for_output_names() {
        assert_eq!(linux_display_id("HDMI-1", 72), "x11-HDMI-1");
        assert_eq!(linux_display_id("", 72), "x11-output-72");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn gnome_wayland_display_state_reports_physical_resolution() {
        let raw = r#"(uint32 2, [(('eDP-1', 'CSO', '0x1600', '0x00000000'), [('2560x1600@165.019', 2560, 1600, 165.01876831054688, 1.3333333730697632, [1.0, 1.25, 1.3333333730697632], {'is-current': <true>}), ('2560x1600@60.008', 2560, 1600, 60.007980346679688, 1.3333333730697632, [1.0, 1.25, 1.3333333730697632], {'is-preferred': <true>})], {'display-name': <'Built-in Display'>})], [(0, 0, 1.3333333730697632, uint32 0, true, [('eDP-1', 'CSO', '0x1600', '0x00000000')], @a{sv} {})], {'layout-mode': <uint32 1>})"#;

        let state = parse_gnome_display_config_state(raw).unwrap();

        assert_eq!(state.display_count, 1);
        assert_eq!(state.primary_width, 2560);
        assert_eq!(state.primary_height, 1600);
        assert_eq!(state.displays[0].display_id, "wayland-eDP-1");
        assert_eq!(state.displays[0].width, 2560);
        assert_eq!(state.displays[0].height, 1600);
        assert_eq!(state.displays[0].refresh_rate_millihz, Some(165_019));
        assert_eq!(state.displays[0].scale_percent, Some(133));
        assert_eq!(state.displays[0].modes.len(), 2);
        assert_eq!(
            state.displays[0].friendly_name.as_deref(),
            Some("Built-in Display")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn gnome_wayland_display_modes_are_deduplicated() {
        let raw = r#"(uint32 2, [(('eDP-1', 'CSO', '0x1600', '0x00000000'), [('2560x1600@165.019', 2560, 1600, 165.01876831054688, 1.3333333730697632, [1.0, 1.25, 1.3333333730697632], {'is-current': <true>}), ('2560x1600@165.019', 2560, 1600, 165.01876831054688, 1.3333333730697632, [1.0, 1.25, 1.3333333730697632], @a{sv} {}), ('2560x1600@60.008', 2560, 1600, 60.007980346679688, 1.3333333730697632, [1.0, 1.25, 1.3333333730697632], {'is-preferred': <true>}), ('2560x1600@60.008', 2560, 1600, 60.007980346679688, 1.3333333730697632, [1.0, 1.25, 1.3333333730697632], @a{sv} {})], {'display-name': <'Built-in Display'>})], [(0, 0, 1.3333333730697632, uint32 0, true, [('eDP-1', 'CSO', '0x1600', '0x00000000')], @a{sv} {})], {'layout-mode': <uint32 1>})"#;

        let state = parse_gnome_display_config_state(raw).unwrap();

        assert_eq!(state.displays[0].modes.len(), 2);
        assert_eq!(
            state.displays[0].modes[0].refresh_rate_millihz,
            Some(165_019)
        );
        assert_eq!(
            state.displays[0].modes[1].refresh_rate_millihz,
            Some(60_008)
        );
    }
}
