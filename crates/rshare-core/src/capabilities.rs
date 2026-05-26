//! Daemon-owned endpoint capability registry models.
//!
//! The registry is a compatibility layer over the current Alpha runtime. It
//! exposes a single authoritative capability snapshot without replacing the
//! existing status, local controls, or endpoint event IPC contracts.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    BackendHealth, BackendRuntimeState, DeviceCapabilities, DeviceId, FeatureConfig,
    LocalControlDeviceSnapshot, NetworkTransportSnapshot, PrivilegeState,
};

/// Product capability categories reserved by the daemon contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EndpointCapabilityKind {
    Input,
    Clipboard,
    Gamepad,
    Audio,
    DisplayTopology,
    UsbHost,
    UsbReceiver,
    PrivilegedHelper,
    Diagnostics,
}

/// Runtime state of one capability on one endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityState {
    Available,
    Degraded,
    Unavailable,
    Experimental,
}

/// Capability snapshot for a single capability on a single device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointCapabilitySnapshot {
    pub kind: EndpointCapabilityKind,
    pub version: u16,
    pub state: CapabilityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_state: Option<PrivilegeState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_state: Option<String>,
    #[serde(default)]
    pub details: BTreeMap<String, String>,
}

impl EndpointCapabilitySnapshot {
    pub fn new(kind: EndpointCapabilityKind, state: CapabilityState) -> Self {
        Self {
            kind,
            version: 1,
            state,
            health_reason: None,
            permission_state: None,
            latency_ms: None,
            last_event_at_ms: None,
            transport_state: None,
            details: BTreeMap::new(),
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.health_reason = Some(reason.into());
        self
    }

    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }
}

/// Capability snapshot for one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCapabilitySnapshot {
    pub device_id: DeviceId,
    pub device_name: String,
    pub hostname: String,
    pub connected: bool,
    #[serde(default)]
    pub capabilities: Vec<EndpointCapabilitySnapshot>,
}

/// Full daemon-owned capability registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRegistrySnapshot {
    pub local_device_id: DeviceId,
    pub generated_at_ms: u64,
    #[serde(default)]
    pub devices: Vec<DeviceCapabilitySnapshot>,
}

/// Derive local endpoint capabilities from current daemon runtime snapshots.
pub fn local_capability_snapshots(
    backend: &BackendRuntimeState,
    controls: &LocalControlDeviceSnapshot,
    network: &NetworkTransportSnapshot,
    features: &FeatureConfig,
) -> Vec<EndpointCapabilitySnapshot> {
    let mut capabilities = Vec::new();

    let mut input = if backend.has_end_to_end_path() {
        EndpointCapabilitySnapshot::new(EndpointCapabilityKind::Input, CapabilityState::Available)
    } else if backend.selected_mode.is_some() {
        EndpointCapabilitySnapshot::new(EndpointCapabilityKind::Input, CapabilityState::Degraded)
            .with_reason(backend_health_reason(&backend.aggregate_health))
    } else {
        EndpointCapabilitySnapshot::new(EndpointCapabilityKind::Input, CapabilityState::Unavailable)
            .with_reason("input backend unavailable")
    };
    input.permission_state = Some(backend.privilege_state);
    if let Some(mode) = backend.selected_mode {
        input = input.with_detail("mode", format!("{mode:?}"));
    }
    capabilities.push(input);

    capabilities.push(EndpointCapabilitySnapshot::new(
        EndpointCapabilityKind::Clipboard,
        CapabilityState::Available,
    ));

    let gamepad_state = if controls.gamepads.iter().any(|gamepad| gamepad.connected) {
        CapabilityState::Available
    } else {
        CapabilityState::Unavailable
    };
    capabilities.push(
        EndpointCapabilitySnapshot::new(EndpointCapabilityKind::Gamepad, gamepad_state)
            .with_detail("devices", controls.gamepads.len().to_string()),
    );

    let audio_available = features.audio_capture
        || features.audio_forwarding
        || controls.audio_inputs.iter().any(|device| device.connected)
        || controls.audio_outputs.iter().any(|device| device.connected);
    let audio_state = if audio_available {
        CapabilityState::Available
    } else {
        CapabilityState::Unavailable
    };
    capabilities.push(
        EndpointCapabilitySnapshot::new(EndpointCapabilityKind::Audio, audio_state)
            .with_detail("inputs", controls.audio_inputs.len().to_string())
            .with_detail("outputs", controls.audio_outputs.len().to_string()),
    );

    let display_state =
        if controls.display.display_count > 0 || !controls.display.displays.is_empty() {
            CapabilityState::Available
        } else {
            CapabilityState::Unavailable
        };
    let display_count = controls
        .display
        .display_count
        .max(controls.display.displays.len());
    let mut display_topology =
        EndpointCapabilitySnapshot::new(EndpointCapabilityKind::DisplayTopology, display_state)
            .with_detail("display_count", display_count.to_string())
            .with_detail(
                "primary_resolution",
                format!(
                    "{}x{}",
                    controls.display.primary_width, controls.display.primary_height
                ),
            )
            .with_detail(
                "layout_resolution",
                format!(
                    "{}x{}",
                    controls.display.layout_width, controls.display.layout_height
                ),
            )
            .with_detail(
                "virtual_origin",
                format!(
                    "{},{}",
                    controls.display.virtual_x, controls.display.virtual_y
                ),
            );
    if !controls.display.displays.is_empty() {
        let display_settings_writable = controls.display.displays.iter().any(|display| {
            display.write_capabilities.resolution
                || display.write_capabilities.refresh_rate
                || display.write_capabilities.orientation
                || display.write_capabilities.primary
                || display.write_capabilities.position
                || display.write_capabilities.scale
        });
        let display_capture_available = controls
            .display
            .displays
            .iter()
            .any(|display| display.write_capabilities.capture);
        display_topology = display_topology
            .with_detail(
                "display_geometries",
                controls
                    .display
                    .displays
                    .iter()
                    .map(|display| {
                        format!(
                            "{}:{}x{}@{},{}{}",
                            display.display_id,
                            display.width,
                            display.height,
                            display.x,
                            display.y,
                            if display.primary { ":primary" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(";"),
            )
            .with_detail("settings_writable", display_settings_writable.to_string())
            .with_detail("capture_available", display_capture_available.to_string());
    }
    capabilities.push(display_topology);

    let usb_host = if features.usb_forwarding_experimental {
        EndpointCapabilitySnapshot::new(
            EndpointCapabilityKind::UsbHost,
            CapabilityState::Experimental,
        )
        .with_reason("experimental USB host path enabled")
    } else {
        EndpointCapabilitySnapshot::new(
            EndpointCapabilityKind::UsbHost,
            CapabilityState::Unavailable,
        )
        .with_reason("experimental USB forwarding disabled")
    };
    capabilities.push(usb_host);

    capabilities.push(
        EndpointCapabilitySnapshot::new(
            EndpointCapabilityKind::UsbReceiver,
            CapabilityState::Unavailable,
        )
        .with_reason("receiver-side virtual USB bus not implemented"),
    );

    capabilities.push(
        EndpointCapabilitySnapshot::new(
            EndpointCapabilityKind::PrivilegedHelper,
            CapabilityState::Unavailable,
        )
        .with_reason("privileged helper not implemented"),
    );

    let diagnostics_state = if network.transport.is_empty() {
        CapabilityState::Degraded
    } else {
        CapabilityState::Available
    };
    let mut diagnostics =
        EndpointCapabilitySnapshot::new(EndpointCapabilityKind::Diagnostics, diagnostics_state)
            .with_detail("transport", network.transport.clone())
            .with_detail("datagram_available", network.datagram_available.to_string())
            .with_detail("realtime_degraded", network.realtime_degraded.to_string());
    diagnostics.latency_ms = network.rtt_ms;
    diagnostics.transport_state = Some(if network.realtime_degraded {
        "degraded".to_string()
    } else {
        "healthy".to_string()
    });
    capabilities.push(diagnostics);

    capabilities
}

/// Derive a peer capability list from advertised protocol capabilities.
pub fn remote_capability_snapshots(
    advertised: &DeviceCapabilities,
    connected: bool,
) -> Vec<EndpointCapabilitySnapshot> {
    let connection_state = if connected {
        CapabilityState::Available
    } else {
        CapabilityState::Degraded
    };

    vec![
        EndpointCapabilitySnapshot::new(EndpointCapabilityKind::Input, connection_state)
            .with_detail("hotkeys", advertised.supports_hotkeys.to_string()),
        EndpointCapabilitySnapshot::new(
            EndpointCapabilityKind::Clipboard,
            if advertised.supports_clipboard {
                connection_state
            } else {
                CapabilityState::Unavailable
            },
        ),
        EndpointCapabilitySnapshot::new(
            EndpointCapabilityKind::Gamepad,
            if advertised.supports_gamepad_capture || advertised.supports_gamepad_inject {
                connection_state
            } else {
                CapabilityState::Unavailable
            },
        )
        .with_detail("max_gamepads", advertised.max_gamepads.to_string()),
        EndpointCapabilitySnapshot::new(
            EndpointCapabilityKind::Audio,
            if advertised.supports_audio_capture
                || advertised.supports_audio_output_control
                || advertised.supports_audio_forwarding
            {
                connection_state
            } else {
                CapabilityState::Unavailable
            },
        )
        .with_detail("formats", advertised.audio_formats.len().to_string()),
        EndpointCapabilitySnapshot::new(EndpointCapabilityKind::DisplayTopology, connection_state)
            .with_detail("max_devices", advertised.max_devices.to_string()),
        EndpointCapabilitySnapshot::new(
            EndpointCapabilityKind::UsbHost,
            if advertised.supports_usb_forwarding_experimental {
                CapabilityState::Experimental
            } else {
                CapabilityState::Unavailable
            },
        ),
        EndpointCapabilitySnapshot::new(
            EndpointCapabilityKind::UsbReceiver,
            CapabilityState::Unavailable,
        )
        .with_reason("receiver-side virtual USB bus not implemented"),
        EndpointCapabilitySnapshot::new(
            EndpointCapabilityKind::PrivilegedHelper,
            CapabilityState::Unavailable,
        )
        .with_reason("privileged helper not implemented"),
        EndpointCapabilitySnapshot::new(EndpointCapabilityKind::Diagnostics, connection_state),
    ]
}

fn backend_health_reason(health: &BackendHealth) -> String {
    match health {
        BackendHealth::Healthy => "healthy".to_string(),
        BackendHealth::Degraded { reason } => format!("{reason:?}"),
    }
}
