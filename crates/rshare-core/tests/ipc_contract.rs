use rshare_core::{
    ipc::{
        default_ipc_addr, read_json_line, write_json_line, DaemonDeviceSnapshot, DaemonRequest,
        DaemonResponse, ServiceStatusSnapshot,
    },
    service::{pid_file_path, state_dir},
    BackgroundProcessOwner, BackgroundRunMode, LocalAudioCaptureSource, LocalAudioCaptureStatus,
    LocalAudioInputDevice, LocalAudioInputKind, LocalAudioOutputDevice, LocalAudioTestRequest,
    LocalControlDeviceSnapshot, LocalInputDeviceKind, LocalInputDiagnosticEvent,
    LocalInputEventSource, LocalInputTestKind, LocalInputTestRequest, TrayRuntimeState,
    UsbDescriptorProbeResult, UsbDescriptorProbeStatus, UsbDeviceDescriptor, UsbDeviceSpeed,
};
use std::collections::BTreeMap;
use tokio::io::duplex;
use uuid::Uuid;

#[tokio::test]
async fn daemon_requests_round_trip_over_json_lines() {
    let (mut writer, mut reader) = duplex(1024);
    let request = DaemonRequest::Status;

    write_json_line(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_line(&mut reader).await.unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn daemon_connect_request_round_trips_target_device() {
    let (mut writer, mut reader) = duplex(1024);
    let request = DaemonRequest::Connect {
        device_id: Uuid::nil(),
    };

    write_json_line(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_line(&mut reader).await.unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn local_control_requests_round_trip_over_json_lines() {
    let (mut writer, mut reader) = duplex(1024);
    let request = DaemonRequest::RunLocalInputTest {
        test: LocalInputTestRequest {
            kind: LocalInputTestKind::KeyboardShift,
        },
    };

    write_json_line(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_line(&mut reader).await.unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn audio_control_requests_round_trip_over_json_lines() {
    let requests = [
        DaemonRequest::SetAudioOutputVolume {
            endpoint_id: "endpoint-1".to_string(),
            volume_percent: 42,
        },
        DaemonRequest::SetAudioOutputMute {
            endpoint_id: "endpoint-1".to_string(),
            muted: true,
        },
        DaemonRequest::StartAudioCapture {
            source: LocalAudioCaptureSource::Loopback,
            endpoint_id: Some("endpoint-1".to_string()),
        },
        DaemonRequest::StartAudioForwarding {
            source: LocalAudioCaptureSource::Microphone,
            endpoint_id: None,
        },
        DaemonRequest::RunAudioTest {
            test: LocalAudioTestRequest::default(),
        },
    ];

    for request in requests {
        let (mut writer, mut reader) = duplex(4096);
        write_json_line(&mut writer, &request).await.unwrap();
        let decoded: DaemonRequest = read_json_line(&mut reader).await.unwrap();
        assert_eq!(decoded, request);
    }
}

#[tokio::test]
async fn usb_device_requests_round_trip_over_json_lines() {
    let requests = [
        DaemonRequest::ListUsbDevices,
        DaemonRequest::RunRemoteUsbDescriptorProbe {
            device_id: Uuid::nil(),
            bus_id: "usb:1-2".to_string(),
        },
    ];

    for request in requests {
        let (mut writer, mut reader) = duplex(4096);
        write_json_line(&mut writer, &request).await.unwrap();
        let decoded: DaemonRequest = read_json_line(&mut reader).await.unwrap();

        assert_eq!(decoded, request);
    }
}

#[tokio::test]
async fn daemon_responses_round_trip_device_payloads() {
    let (mut writer, mut reader) = duplex(4096);
    let response = DaemonResponse::Devices(vec![DaemonDeviceSnapshot {
        id: Uuid::nil(),
        name: "desktop".to_string(),
        hostname: "desktop-host".to_string(),
        addresses: vec!["192.168.1.10:27431".to_string()],
        connected: false,
        last_seen_secs: Some(4),
    }]);

    write_json_line(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_line(&mut reader).await.unwrap();

    assert_eq!(decoded, response);
}

#[tokio::test]
async fn daemon_responses_round_trip_usb_device_payloads() {
    let (mut writer, mut reader) = duplex(4096);
    let response = DaemonResponse::UsbDevices(vec![UsbDeviceDescriptor {
        bus_id: r#"\\?\usb#vid_045e&pid_028e#123456"#.to_string(),
        vendor_id: 0x045e,
        product_id: 0x028e,
        class_code: 0,
        subclass_code: 0,
        protocol_code: 0,
        manufacturer: Some("vendor".to_string()),
        product: Some("device".to_string()),
        serial_number: Some("123456".to_string()),
        usb_version_bcd: 0x0200,
        device_version_bcd: 0x0100,
        speed: UsbDeviceSpeed::High,
        active_configuration: Some(1),
        container_id: None,
        capture_exclusive_required: true,
        configurations: Vec::new(),
        endpoints: Vec::new(),
    }]);

    write_json_line(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_line(&mut reader).await.unwrap();

    assert_eq!(decoded, response);
}

#[tokio::test]
async fn daemon_responses_round_trip_usb_descriptor_probe_payload() {
    let (mut writer, mut reader) = duplex(4096);
    let response = DaemonResponse::UsbDescriptorProbe(UsbDescriptorProbeResult {
        status: UsbDescriptorProbeStatus::Success,
        message: "descriptor read".to_string(),
        device_id: Uuid::nil(),
        bus_id: "usb:1-2".to_string(),
        request_id: 1,
        transfer_id: 2,
        session_id: Some(Uuid::nil()),
        elapsed_ms: Some(4),
        actual_length: Some(18),
        descriptor: None,
        descriptor_bytes: vec![18, 1, 0, 2],
    });

    write_json_line(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_line(&mut reader).await.unwrap();

    assert_eq!(decoded, response);
}

#[tokio::test]
async fn local_control_response_round_trips_snapshot_payload() {
    let (mut writer, mut reader) = duplex(4096);
    let mut snapshot = LocalControlDeviceSnapshot::default();
    snapshot.keyboard.detected = true;
    snapshot.keyboard.last_key = Some("ShiftLeft".to_string());
    snapshot.audio_outputs.push(LocalAudioOutputDevice {
        id: "audio-default".to_string(),
        name: "Speakers".to_string(),
        source: "Windows Core Audio".to_string(),
        connected: true,
        default: true,
        volume_percent: Some(42),
        muted: Some(false),
        ..LocalAudioOutputDevice::default()
    });
    snapshot.audio_inputs.push(LocalAudioInputDevice {
        id: "loopback-default".to_string(),
        name: "System sound".to_string(),
        source: "Windows WASAPI loopback".to_string(),
        kind: LocalAudioInputKind::Loopback,
        connected: true,
        default: true,
        level_peak: 7,
        level_rms: 3,
        sample_rate: Some(48_000),
        channel_count: Some(2),
        ..LocalAudioInputDevice::default()
    });
    snapshot.audio_capture_state.status = LocalAudioCaptureStatus::CapturingLocal;
    let response = DaemonResponse::LocalControls(snapshot.clone());

    write_json_line(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_line(&mut reader).await.unwrap();

    assert_eq!(decoded, response);
}

#[test]
fn local_control_snapshot_defaults_missing_fields_to_safe_values() {
    let snapshot: LocalControlDeviceSnapshot = serde_json::from_str("{}").unwrap();

    assert_eq!(snapshot.sequence, 0);
    assert!(!snapshot.keyboard.detected);
    assert!(!snapshot.mouse.detected);
    assert!(snapshot.gamepads.is_empty());
    assert_eq!(snapshot.virtual_gamepad.status, "not_implemented");
    assert_eq!(snapshot.driver.status, "unavailable");
    assert!(snapshot.driver.device_path.is_none());
    assert!(snapshot.keyboard_devices.is_empty());
    assert!(snapshot.audio_inputs.is_empty());
    assert!(snapshot.audio_outputs.is_empty());
    assert!(snapshot.usb_devices.is_empty());
    assert!(snapshot.remote_usb_devices.is_empty());
    assert_eq!(
        snapshot.audio_capture_state.status,
        LocalAudioCaptureStatus::Idle
    );
    assert!(!snapshot.audio_stream_state.active);
}

#[test]
fn local_input_event_round_trips_driver_metadata() {
    let event = LocalInputDiagnosticEvent {
        sequence: 7,
        timestamp_ms: 42,
        device_kind: LocalInputDeviceKind::Keyboard,
        event_kind: "key".to_string(),
        summary: "driver key packet".to_string(),
        device_id: Some("driver:keyboard:001".to_string()),
        device_instance_id: Some("HID\\VID_0001&PID_0002".to_string()),
        capture_path: Some("rshare-filter".to_string()),
        source: LocalInputEventSource::DriverTest,
        payload: BTreeMap::new(),
    };

    let encoded = serde_json::to_string(&event).unwrap();
    let decoded: LocalInputDiagnosticEvent = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded, event);
}

#[test]
fn default_status_snapshot_starts_empty_and_healthy() {
    let snapshot = ServiceStatusSnapshot::new(
        Uuid::nil(),
        "desktop".to_string(),
        "desktop-host".to_string(),
        "0.0.0.0:27431".to_string(),
        27432,
        42,
    );

    assert_eq!(snapshot.discovered_devices, 0);
    assert_eq!(snapshot.connected_devices, 0);
    assert!(snapshot.healthy);
    assert_eq!(snapshot.pid, 42);
}

#[test]
fn default_status_snapshot_reports_daemon_owned_background_runtime() {
    let snapshot = ServiceStatusSnapshot::new(
        Uuid::nil(),
        "desktop".to_string(),
        "desktop-host".to_string(),
        "0.0.0.0:27431".to_string(),
        27432,
        42,
    );

    assert_eq!(snapshot.background_owner, BackgroundProcessOwner::Daemon);
    assert_eq!(
        snapshot.background_mode,
        BackgroundRunMode::BackgroundProcess
    );
    assert_eq!(snapshot.tray_owner, BackgroundProcessOwner::Daemon);
    assert_eq!(snapshot.tray_state, TrayRuntimeState::Unavailable);
    assert!(!snapshot.started_by_desktop);
}

#[test]
fn service_paths_live_under_rshare_state_dir() {
    let state_dir = state_dir().unwrap();
    let pid_file = pid_file_path().unwrap();

    assert!(state_dir.ends_with("rshare"));
    assert_eq!(pid_file.parent(), Some(state_dir.as_path()));
}

#[test]
fn default_ipc_addr_binds_to_loopback() {
    let addr = default_ipc_addr();

    assert!(addr.ip().is_loopback());
    assert_eq!(addr.port(), 27435);
}
