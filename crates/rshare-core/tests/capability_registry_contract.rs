use rshare_core::{
    BackendHealth, BackendRuntimeState, CapabilityRegistrySnapshot, CapabilityState,
    DeviceCapabilities, DeviceCapabilitySnapshot, EndpointCapabilityKind,
    EndpointCapabilitySnapshot, FeatureConfig, LocalAudioInputDevice, LocalAudioOutputDevice,
    LocalControlDeviceSnapshot, LocalDisplayInfo, NetworkTransportSnapshot, ResolvedInputMode,
};
use uuid::Uuid;

fn capability<'a>(
    capabilities: &'a [EndpointCapabilitySnapshot],
    kind: EndpointCapabilityKind,
) -> &'a EndpointCapabilitySnapshot {
    capabilities
        .iter()
        .find(|capability| capability.kind == kind)
        .unwrap_or_else(|| panic!("missing capability: {kind:?}"))
}

#[test]
fn capability_registry_snapshot_round_trips_json() {
    let local_id = Uuid::nil();
    let registry = CapabilityRegistrySnapshot {
        local_device_id: local_id,
        generated_at_ms: 42,
        devices: vec![DeviceCapabilitySnapshot {
            device_id: local_id,
            device_name: "desktop".to_string(),
            hostname: "desktop-host".to_string(),
            connected: true,
            capabilities: vec![EndpointCapabilitySnapshot::new(
                EndpointCapabilityKind::Input,
                CapabilityState::Available,
            )],
        }],
    };

    let encoded = serde_json::to_string(&registry).unwrap();
    let decoded: CapabilityRegistrySnapshot = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded, registry);
}

#[test]
fn local_capabilities_report_input_and_diagnostics_available_when_backends_are_healthy() {
    let mut backend = BackendRuntimeState::new();
    backend.selected_mode = Some(ResolvedInputMode::Portable);
    backend.capture_health = BackendHealth::Healthy;
    backend.inject_health = BackendHealth::Healthy;
    backend.update_aggregate_health();

    let mut controls = LocalControlDeviceSnapshot::default();
    controls.keyboard.detected = true;
    controls.mouse.detected = true;
    controls.display.display_count = 1;
    #[cfg(windows)]
    {
        controls.capture_backend.mode = Some(ResolvedInputMode::WindowsNative);
        controls.capture_backend.active = true;
    }
    #[cfg(target_os = "macos")]
    {
        controls.capture_backend.mode = Some(ResolvedInputMode::Portable);
        controls.capture_backend.active = true;
    }
    controls.audio_inputs.push(LocalAudioInputDevice {
        id: "mic".to_string(),
        name: "Microphone".to_string(),
        connected: true,
        ..LocalAudioInputDevice::default()
    });
    controls.audio_outputs.push(LocalAudioOutputDevice {
        id: "speaker".to_string(),
        name: "Speakers".to_string(),
        connected: true,
        ..LocalAudioOutputDevice::default()
    });

    let capabilities = rshare_core::local_capability_snapshots(
        &backend,
        &controls,
        &NetworkTransportSnapshot::default(),
        &FeatureConfig::default(),
    );

    let input = capability(&capabilities, EndpointCapabilityKind::Input);
    #[cfg(any(windows, target_os = "macos"))]
    assert_eq!(input.state, CapabilityState::Available);
    #[cfg(target_os = "macos")]
    assert_eq!(
        input
            .details
            .get("shortcut_suppression")
            .map(String::as_str),
        Some("required")
    );
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        assert_eq!(input.state, CapabilityState::Degraded);
        assert_eq!(
            input
                .details
                .get("shortcut_suppression")
                .map(String::as_str),
            Some("unavailable")
        );
    }
    assert_eq!(
        capability(&capabilities, EndpointCapabilityKind::Diagnostics).state,
        CapabilityState::Available
    );
    assert_eq!(
        capability(&capabilities, EndpointCapabilityKind::DisplayTopology).state,
        CapabilityState::Available
    );
    assert_eq!(
        capability(&capabilities, EndpointCapabilityKind::Audio).state,
        CapabilityState::Available
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_inactive_or_missing_portable_capture_degrades_required_shortcut_suppression() {
    let mut backend = BackendRuntimeState::new();
    backend.selected_mode = Some(ResolvedInputMode::Portable);
    backend.capture_health = BackendHealth::Healthy;
    backend.inject_health = BackendHealth::Healthy;
    backend.update_aggregate_health();

    let mut inactive_controls = LocalControlDeviceSnapshot::default();
    inactive_controls.capture_backend.mode = Some(ResolvedInputMode::Portable);
    let missing_controls = LocalControlDeviceSnapshot::default();

    for controls in [&inactive_controls, &missing_controls] {
        let capabilities = rshare_core::local_capability_snapshots(
            &backend,
            controls,
            &NetworkTransportSnapshot::default(),
            &FeatureConfig::default(),
        );
        let input = capability(&capabilities, EndpointCapabilityKind::Input);

        assert_eq!(input.state, CapabilityState::Degraded);
        assert_eq!(
            input
                .details
                .get("shortcut_suppression")
                .map(String::as_str),
            Some("unavailable")
        );
    }
}

#[cfg(windows)]
#[test]
fn active_filter_capture_reports_shortcut_suppression_as_unavailable() {
    let mut backend = BackendRuntimeState::new();
    backend.selected_mode = Some(ResolvedInputMode::VirtualHid);
    backend.update_aggregate_health();

    let mut controls = LocalControlDeviceSnapshot::default();
    controls.capture_backend.mode = Some(ResolvedInputMode::VirtualHid);
    controls.capture_backend.active = true;

    let capabilities = rshare_core::local_capability_snapshots(
        &backend,
        &controls,
        &NetworkTransportSnapshot::default(),
        &FeatureConfig::default(),
    );
    let input = capability(&capabilities, EndpointCapabilityKind::Input);

    assert_eq!(input.state, CapabilityState::Degraded);
    assert_eq!(
        input
            .details
            .get("shortcut_suppression")
            .map(String::as_str),
        Some("unavailable")
    );
}

#[test]
fn local_display_topology_preserves_multiple_monitor_details() {
    let backend = BackendRuntimeState::new();
    let mut controls = LocalControlDeviceSnapshot::default();
    controls.display.display_count = 2;
    controls.display.primary_width = 2560;
    controls.display.primary_height = 1440;
    controls.display.layout_width = 4480;
    controls.display.layout_height = 1440;
    controls.display.displays = vec![
        LocalDisplayInfo {
            display_id: "primary".to_string(),
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
            primary: true,
            write_capabilities: rshare_core::DisplayWriteCapabilities {
                resolution: true,
                refresh_rate: true,
                capture: true,
                ..rshare_core::DisplayWriteCapabilities::default()
            },
            ..LocalDisplayInfo::default()
        },
        LocalDisplayInfo {
            display_id: "right".to_string(),
            x: 2560,
            y: 0,
            width: 1920,
            height: 1080,
            primary: false,
            ..LocalDisplayInfo::default()
        },
    ];

    let capabilities = rshare_core::local_capability_snapshots(
        &backend,
        &controls,
        &NetworkTransportSnapshot::default(),
        &FeatureConfig::default(),
    );
    let display = capability(&capabilities, EndpointCapabilityKind::DisplayTopology);

    assert_eq!(display.state, CapabilityState::Available);
    assert_eq!(
        display.details.get("display_count").map(String::as_str),
        Some("2")
    );
    assert_eq!(
        display
            .details
            .get("primary_resolution")
            .map(String::as_str),
        Some("2560x1440")
    );
    assert_eq!(
        display.details.get("layout_resolution").map(String::as_str),
        Some("4480x1440")
    );
    assert_eq!(
        display
            .details
            .get("display_geometries")
            .map(String::as_str),
        Some("primary:2560x1440@0,0:primary;right:1920x1080@2560,0")
    );
    assert_eq!(
        display.details.get("settings_writable").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        display.details.get("capture_available").map(String::as_str),
        Some("true")
    );
}

#[test]
fn local_capabilities_keep_usb_and_helper_boundaries_explicit() {
    let backend = BackendRuntimeState::new();
    let controls = LocalControlDeviceSnapshot::default();
    let network = NetworkTransportSnapshot::default();
    let mut features = FeatureConfig::default();

    let disabled =
        rshare_core::local_capability_snapshots(&backend, &controls, &network, &features);
    assert_eq!(
        capability(&disabled, EndpointCapabilityKind::UsbHost).state,
        CapabilityState::Unavailable
    );
    assert_eq!(
        capability(&disabled, EndpointCapabilityKind::UsbReceiver).state,
        CapabilityState::Unavailable
    );
    assert_eq!(
        capability(&disabled, EndpointCapabilityKind::PrivilegedHelper).state,
        CapabilityState::Unavailable
    );

    features.usb_forwarding_experimental = true;
    let enabled = rshare_core::local_capability_snapshots(&backend, &controls, &network, &features);
    let usb_host = capability(&enabled, EndpointCapabilityKind::UsbHost);
    assert_eq!(usb_host.state, CapabilityState::Experimental);
    assert_eq!(
        capability(&enabled, EndpointCapabilityKind::UsbReceiver)
            .health_reason
            .as_deref(),
        Some("receiver-side virtual USB bus not implemented")
    );
}

#[test]
fn remote_capabilities_preserve_peer_advertised_features() {
    let mut advertised = DeviceCapabilities::default();
    advertised.supports_gamepad_capture = true;
    advertised.supports_audio_capture = true;
    advertised.supports_audio_output_control = true;
    advertised.supports_usb_forwarding_experimental = true;

    let capabilities = rshare_core::remote_capability_snapshots(&advertised, true);

    assert_eq!(
        capability(&capabilities, EndpointCapabilityKind::Input).state,
        CapabilityState::Available
    );
    assert_eq!(
        capability(&capabilities, EndpointCapabilityKind::Gamepad).state,
        CapabilityState::Available
    );
    assert_eq!(
        capability(&capabilities, EndpointCapabilityKind::Audio).state,
        CapabilityState::Available
    );
    assert_eq!(
        capability(&capabilities, EndpointCapabilityKind::UsbHost).state,
        CapabilityState::Experimental
    );
    assert_eq!(
        capability(&capabilities, EndpointCapabilityKind::Diagnostics).state,
        CapabilityState::Available
    );
}
