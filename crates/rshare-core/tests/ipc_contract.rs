use rshare_core::{
    ipc::{
        default_ipc_addr, default_mobile_gateway_addr, read_json_frame, write_json_frame,
        DaemonDeviceSnapshot, DaemonRequest, DaemonResponse, ServiceStatusSnapshot,
    },
    service::{pid_file_path, state_dir},
    BackgroundProcessOwner, BackgroundRunMode, CapabilityRegistrySnapshot, DeviceAttribution,
    DeviceCapabilitySnapshot, DisplayCaptureRequest, DisplayCaptureResult, DisplayIdentifyRequest,
    DisplayIdentifyResult, DisplayOperationStatus, DisplayOrientation,
    DisplaySettingsUpdateRequest, DisplaySettingsUpdateResult, EndpointCapabilityKind,
    EndpointCapabilitySnapshot, EndpointDeviceRef, EndpointEvent, EndpointEventDirection,
    EndpointEventFilter, EndpointEventKind, EndpointEventPayload, EndpointEventSource,
    EndpointInjectMode, EndpointInjectRequest, EndpointInjectResult, EndpointInjectTarget,
    LocalAudioCaptureSource, LocalAudioCaptureStatus, LocalAudioEndpointFormFactor,
    LocalAudioInputDevice, LocalAudioInputKind, LocalAudioOutputDevice, LocalAudioTestRequest,
    LocalControlDeviceSnapshot, LocalInputDeviceKind, LocalInputDiagnosticEvent,
    LocalInputEventSource, LocalInputTestKind, LocalInputTestRequest, MobileAccessSnapshot,
    TrayRuntimeState, UsbDescriptorProbeResult, UsbDescriptorProbeStatus, UsbDeviceDescriptor,
    UsbDeviceSpeed, VirtualDisplayCreateRequest, VirtualDisplayOperationResult,
    VirtualDisplayOperationStatus, VirtualDisplayRemoveRequest, VirtualDisplaySnapshot,
    VirtualDisplayStatus,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;
use tokio::io::{duplex, AsyncWriteExt};
use tokio::time::timeout;
use uuid::Uuid;

#[test]
fn display_capture_binary_is_correlated_and_length_checked() {
    let descriptor = rshare_core::DisplayCaptureDescriptor {
        capture_id: Uuid::from_u128(0x00112233445566778899aabbccddeeff),
        display_id: "display-1".to_string(),
        mime_type: "image/png".to_string(),
        width: 900,
        height: 506,
        byte_length: 4,
    };
    let body = rshare_core::encode_display_capture_binary(
        &descriptor,
        bytes::Bytes::from_static(&[137, 80, 78, 71]),
    )
    .unwrap();
    let decoded = rshare_core::decode_display_capture_binary(&descriptor, body.clone()).unwrap();
    assert_eq!(&decoded[..], &[137, 80, 78, 71]);

    let mut wrong_id = rshare_core::encode_display_capture_binary(
        &descriptor,
        bytes::Bytes::from_static(&[137, 80, 78, 71]),
    )
    .unwrap()
    .to_vec();
    wrong_id[0] ^= 0xff;
    assert!(
        rshare_core::decode_display_capture_binary(&descriptor, bytes::Bytes::from(wrong_id))
            .is_err()
    );

    let mut wrong_length = descriptor.clone();
    wrong_length.byte_length += 1;
    assert!(rshare_core::decode_display_capture_binary(&wrong_length, body).is_err());
}

#[test]
fn display_capture_response_serializes_metadata_then_binary_without_json_bytes() {
    let descriptor = rshare_core::DisplayCaptureDescriptor {
        capture_id: Uuid::from_u128(0x00112233445566778899aabbccddeeff),
        display_id: "display-1".to_string(),
        mime_type: "image/png".to_string(),
        width: 900,
        height: 506,
        byte_length: 4,
    };
    let result = DisplayCaptureResult {
        request_id: Uuid::from_u128(7),
        status: DisplayOperationStatus::Success,
        message: None,
        payload: Some(descriptor.clone()),
        blob: Some(rshare_core::DisplayCaptureBlob {
            descriptor,
            bytes: bytes::Bytes::from_static(&[137, 80, 78, 71]),
        }),
    };
    let response = rshare_core::encode_display_capture_response(&result).unwrap();
    let json_len = u32::from_be_bytes(response[..4].try_into().unwrap()) as usize;
    assert_eq!(response[4], rshare_core::IpcEnvelopeKind::Json as u8);
    let json = &response[5..5 + json_len];
    let value: serde_json::Value = serde_json::from_slice(json).unwrap();
    assert!(value.get("bytes").is_none());
    assert!(value.get("blob").is_none());
    assert_eq!(value["payload"]["byte_length"], 4);

    let binary_offset = 5 + json_len;
    assert_eq!(
        response[binary_offset + 4],
        rshare_core::IpcEnvelopeKind::Binary as u8
    );
    let binary_len = u32::from_be_bytes(
        response[binary_offset..binary_offset + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    assert_eq!(binary_len, 20);
    assert_eq!(
        &response[binary_offset + 5..binary_offset + 21],
        Uuid::from_u128(0x00112233445566778899aabbccddeeff).as_bytes()
    );

    let mut invalid_error = result;
    invalid_error.status = DisplayOperationStatus::ApplyFailed;
    assert!(rshare_core::encode_display_capture_response(&invalid_error).is_err());
}

#[tokio::test]
async fn ui_state_envelopes_round_trip_over_typed_frames() {
    let (mut writer, mut reader) = duplex(4096);
    let envelope = rshare_core::UiEnvelope::Heartbeat {
        boot_id: Uuid::from_u128(1),
        revision: 7,
        sent_at_ms: 99,
    };

    rshare_core::write_ui_state_frame(&mut writer, &envelope)
        .await
        .unwrap();
    let decoded = rshare_core::read_ui_state_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, envelope);
}

#[test]
fn ui_state_subscription_request_round_trips_with_cursor() {
    let request = DaemonRequest::SubscribeUiState {
        cursor: Some(rshare_core::UiCursor::new(Uuid::from_u128(9), 41)),
    };

    let encoded = serde_json::to_vec(&request).unwrap();
    let decoded: DaemonRequest = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn ui_state_reader_rejects_binary_header_before_waiting_for_body() {
    let (mut writer, mut reader) = duplex(64);
    let declared_length = 16 * 1024 * 1024_u32;
    let mut header = [0_u8; rshare_core::IPC_FRAME_HEADER_LEN];
    header[..4].copy_from_slice(&declared_length.to_be_bytes());
    header[4] = rshare_core::IpcEnvelopeKind::Binary as u8;
    writer.write_all(&header).await.unwrap();

    let error = timeout(
        Duration::from_millis(100),
        rshare_core::read_ui_state_frame(&mut reader),
    )
    .await
    .expect("reader waited for a disallowed Binary body")
    .unwrap_err();

    let io_error = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .expect("error chain should retain the InvalidData framing error");
    assert_eq!(io_error.kind(), std::io::ErrorKind::InvalidData);
}

#[tokio::test]
async fn ui_state_header_rejects_heartbeat_body() {
    let (mut writer, mut reader) = duplex(4096);
    let envelope = rshare_core::UiEnvelope::Heartbeat {
        boot_id: Uuid::from_u128(1),
        revision: 7,
        sent_at_ms: 99,
    };
    let payload = serde_json::to_vec(&envelope).unwrap();
    rshare_core::IpcFrameCodec::default()
        .write_frame(&mut writer, rshare_core::IpcEnvelopeKind::UiState, &payload)
        .await
        .unwrap();

    let error = rshare_core::read_ui_state_frame(&mut reader)
        .await
        .unwrap_err();

    assert!(
        format!("{error:#}").contains("does not match its envelope"),
        "unexpected error: {error:#}"
    );
}

#[tokio::test]
async fn heartbeat_header_rejects_delta_body() {
    let (mut writer, mut reader) = duplex(4096);
    let envelope = rshare_core::UiEnvelope::Delta(rshare_core::UiDelta {
        boot_id: Uuid::from_u128(1),
        revision: 1,
        change: rshare_core::UiChange::Pointer(rshare_core::UiPointerState {
            x: 1,
            y: 2,
            display_id: None,
            observed_at_ms: 3,
        }),
    });
    let payload = serde_json::to_vec(&envelope).unwrap();
    rshare_core::IpcFrameCodec::default()
        .write_frame(
            &mut writer,
            rshare_core::IpcEnvelopeKind::Heartbeat,
            &payload,
        )
        .await
        .unwrap();

    let error = rshare_core::read_ui_state_frame(&mut reader)
        .await
        .unwrap_err();

    assert!(
        format!("{error:#}").contains("does not match its envelope"),
        "unexpected error: {error:#}"
    );
}

#[tokio::test]
async fn daemon_requests_round_trip_over_json_frames() {
    let (mut writer, mut reader) = duplex(1024);
    let request = DaemonRequest::Status;

    write_json_frame(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn daemon_connect_request_round_trips_target_device() {
    let (mut writer, mut reader) = duplex(1024);
    let request = DaemonRequest::Connect {
        device_id: Uuid::nil(),
    };

    write_json_frame(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn local_control_requests_round_trip_over_json_frames() {
    let (mut writer, mut reader) = duplex(1024);
    let request = DaemonRequest::RunLocalInputTest {
        test: LocalInputTestRequest {
            kind: LocalInputTestKind::KeyboardShift,
        },
    };

    write_json_frame(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn audio_control_requests_round_trip_over_json_frames() {
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
        write_json_frame(&mut writer, &request).await.unwrap();
        let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();
        assert_eq!(decoded, request);
    }
}

#[tokio::test]
async fn ipc_contract_display_operation_requests_round_trip_over_json_frames() {
    let requests = [
        DaemonRequest::CaptureDisplay(DisplayCaptureRequest {
            display_id: "primary".to_string(),
            max_width: Some(480),
            format: rshare_core::DisplayCaptureFormat::Png,
        }),
        DaemonRequest::IdentifyDisplays(DisplayIdentifyRequest {
            duration_ms: Some(2500),
        }),
        DaemonRequest::UpdateDisplaySettings(DisplaySettingsUpdateRequest {
            display_id: "display-1".to_string(),
            width: Some(2560),
            height: Some(1440),
            refresh_rate_millihz: Some(144_000),
            orientation: Some(DisplayOrientation::Landscape),
            primary: Some(true),
            x: Some(0),
            y: Some(0),
            scale_percent: Some(150),
        }),
        DaemonRequest::OpenDisplaySettings,
    ];

    for request in requests {
        let (mut writer, mut reader) = duplex(4096);
        write_json_frame(&mut writer, &request).await.unwrap();
        let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

        assert_eq!(decoded, request);
    }
}

#[tokio::test]
async fn ipc_contract_display_operation_responses_round_trip_over_json_frames() {
    let responses = [
        DaemonResponse::DisplayCapture(DisplayCaptureResult {
            request_id: Uuid::from_u128(1),
            status: DisplayOperationStatus::Success,
            message: None,
            payload: Some(rshare_core::DisplayCaptureDescriptor {
                capture_id: Uuid::from_u128(2),
                display_id: "primary".to_string(),
                mime_type: "image/png".to_string(),
                width: 480,
                height: 270,
                byte_length: 4,
            }),
            blob: None,
        }),
        DaemonResponse::DisplayIdentify(DisplayIdentifyResult {
            status: DisplayOperationStatus::Success,
            message: Some("identify overlay shown".to_string()),
        }),
        DaemonResponse::DisplaySettingsUpdated(DisplaySettingsUpdateResult {
            status: DisplayOperationStatus::RequiresSystemSettings,
            message: Some("Open system settings to adjust display scale.".to_string()),
        }),
    ];

    for response in responses {
        let (mut writer, mut reader) = duplex(4096);
        write_json_frame(&mut writer, &response).await.unwrap();
        let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

        assert_eq!(decoded, response);
    }
}

#[test]
fn ipc_contract_daemon_client_display_helpers_have_expected_signatures() {
    fn assert_capture_helper<F, Fut>(_helper: F)
    where
        F: Fn(DisplayCaptureRequest) -> Fut,
        Fut: Future<Output = anyhow::Result<DisplayCaptureResult>>,
    {
    }

    fn assert_identify_helper<F, Fut>(_helper: F)
    where
        F: Fn(DisplayIdentifyRequest) -> Fut,
        Fut: Future<Output = anyhow::Result<DisplayIdentifyResult>>,
    {
    }

    fn assert_update_helper<F, Fut>(_helper: F)
    where
        F: Fn(DisplaySettingsUpdateRequest) -> Fut,
        Fut: Future<Output = anyhow::Result<DisplaySettingsUpdateResult>>,
    {
    }

    fn assert_open_helper<F, Fut>(_helper: F)
    where
        F: Fn() -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
    {
    }

    assert_capture_helper(rshare_core::daemon_client::request_display_capture);
    assert_identify_helper(rshare_core::daemon_client::request_identify_displays);
    assert_update_helper(rshare_core::daemon_client::request_update_display_settings);
    assert_open_helper(rshare_core::daemon_client::request_open_display_settings);
}

#[tokio::test]
async fn virtual_display_requests_round_trip_over_json_frames() {
    let requests = [
        DaemonRequest::ListVirtualDisplays,
        DaemonRequest::CreateVirtualDisplay(VirtualDisplayCreateRequest {
            id: Some("vd-1".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        }),
        DaemonRequest::RemoveVirtualDisplay(VirtualDisplayRemoveRequest {
            id: "vd-1".to_string(),
        }),
    ];

    for request in requests {
        let (mut writer, mut reader) = duplex(4096);
        write_json_frame(&mut writer, &request).await.unwrap();
        let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

        assert_eq!(decoded, request);
    }
}

#[tokio::test]
async fn virtual_display_responses_round_trip_over_json_frames() {
    let display = VirtualDisplaySnapshot {
        id: "vd-1".to_string(),
        width: 1920,
        height: 1080,
        refresh_rate_millihz: Some(60_000),
        name: Some("R-ShareMouse Virtual Display".to_string()),
        status: VirtualDisplayStatus::DriverUnavailable,
        display_id: None,
        message: Some("Windows virtual display driver is not installed".to_string()),
    };
    let responses = [
        DaemonResponse::VirtualDisplays(vec![display.clone()]),
        DaemonResponse::VirtualDisplayOperation(VirtualDisplayOperationResult {
            status: VirtualDisplayOperationStatus::DriverUnavailable,
            display: Some(display),
            message: Some("Windows virtual display driver is not installed".to_string()),
        }),
    ];

    for response in responses {
        let (mut writer, mut reader) = duplex(4096);
        write_json_frame(&mut writer, &response).await.unwrap();
        let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

        assert_eq!(decoded, response);
    }
}

#[test]
fn ipc_contract_daemon_client_virtual_display_helpers_have_expected_signatures() {
    fn assert_list_helper<F, Fut>(_helper: F)
    where
        F: Fn() -> Fut,
        Fut: Future<Output = anyhow::Result<Vec<VirtualDisplaySnapshot>>>,
    {
    }

    fn assert_create_helper<F, Fut>(_helper: F)
    where
        F: Fn(VirtualDisplayCreateRequest) -> Fut,
        Fut: Future<Output = anyhow::Result<VirtualDisplayOperationResult>>,
    {
    }

    fn assert_remove_helper<F, Fut>(_helper: F)
    where
        F: Fn(VirtualDisplayRemoveRequest) -> Fut,
        Fut: Future<Output = anyhow::Result<VirtualDisplayOperationResult>>,
    {
    }

    assert_list_helper(rshare_core::daemon_client::request_virtual_displays);
    assert_create_helper(rshare_core::daemon_client::request_create_virtual_display);
    assert_remove_helper(rshare_core::daemon_client::request_remove_virtual_display);
}

#[tokio::test]
async fn usb_device_requests_round_trip_over_json_frames() {
    let requests = [
        DaemonRequest::ListUsbDevices,
        DaemonRequest::RunRemoteUsbDescriptorProbe {
            device_id: Uuid::nil(),
            bus_id: "usb:1-2".to_string(),
        },
    ];

    for request in requests {
        let (mut writer, mut reader) = duplex(4096);
        write_json_frame(&mut writer, &request).await.unwrap();
        let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

        assert_eq!(decoded, request);
    }
}

#[tokio::test]
async fn endpoint_event_requests_round_trip_over_json_frames() {
    let requests = [
        DaemonRequest::EndpointEvents {
            filter: EndpointEventFilter {
                endpoint_id: Some(Uuid::nil()),
                kinds: vec![EndpointEventKind::Keyboard],
                sources: vec![EndpointEventSource::Hardware],
                include_loopback: true,
                ..EndpointEventFilter::default()
            },
            after_sequence: Some(7),
            limit: Some(32),
        },
        DaemonRequest::SubscribeEndpointEvents {
            filter: EndpointEventFilter {
                kinds: vec![EndpointEventKind::Mouse],
                ..EndpointEventFilter::default()
            },
        },
    ];

    for request in requests {
        let (mut writer, mut reader) = duplex(4096);
        write_json_frame(&mut writer, &request).await.unwrap();
        let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

        assert_eq!(decoded, request);
    }
}

#[tokio::test]
async fn endpoint_inject_request_round_trips_over_json_frames() {
    let request = DaemonRequest::InjectEndpointEvent {
        target: EndpointInjectTarget::Local,
        request: EndpointInjectRequest {
            correlation_id: "test-shift-1".to_string(),
            device_kind: EndpointEventKind::Keyboard,
            payload: EndpointEventPayload::Keyboard {
                key: "ShiftLeft".to_string(),
                state: "Pressed".to_string(),
            },
            mode: EndpointInjectMode::RequireHealthyBackend,
            timeout_ms: 750,
        },
    };

    let (mut writer, mut reader) = duplex(4096);
    write_json_frame(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn endpoint_text_commit_request_round_trips_over_json_frames() {
    let request = DaemonRequest::InjectEndpointEvent {
        target: EndpointInjectTarget::Local,
        request: EndpointInjectRequest {
            correlation_id: "mobile-text-1".to_string(),
            device_kind: EndpointEventKind::Keyboard,
            payload: EndpointEventPayload::TextCommit {
                text: "你好🙂".to_string(),
            },
            mode: EndpointInjectMode::RequireHealthyBackend,
            timeout_ms: 750,
        },
    };

    let (mut writer, mut reader) = duplex(4096);
    write_json_frame(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, request);
}

#[tokio::test]
async fn capability_requests_round_trip_over_json_frames() {
    let requests = [
        DaemonRequest::Capabilities { device_id: None },
        DaemonRequest::Capabilities {
            device_id: Some(Uuid::nil()),
        },
    ];

    for request in requests {
        let (mut writer, mut reader) = duplex(4096);
        write_json_frame(&mut writer, &request).await.unwrap();
        let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

        assert_eq!(decoded, request);
    }
}

#[tokio::test]
async fn capability_response_round_trips_over_json_frames() {
    let registry = CapabilityRegistrySnapshot {
        local_device_id: Uuid::nil(),
        generated_at_ms: 42,
        devices: vec![DeviceCapabilitySnapshot {
            device_id: Uuid::nil(),
            device_name: "desktop".to_string(),
            hostname: "desktop-host".to_string(),
            connected: true,
            capabilities: vec![EndpointCapabilitySnapshot::new(
                EndpointCapabilityKind::Input,
                rshare_core::CapabilityState::Available,
            )],
        }],
    };
    let response = DaemonResponse::Capabilities(registry.clone());

    let (mut writer, mut reader) = duplex(4096);
    write_json_frame(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, DaemonResponse::Capabilities(registry));
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

    write_json_frame(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

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

    write_json_frame(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

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

    write_json_frame(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, response);
}

#[tokio::test]
async fn daemon_responses_round_trip_endpoint_event_payloads() {
    let event = EndpointEvent {
        event_id: 42,
        sequence: 42,
        timestamp_ms: 1_714_000_000_000,
        endpoint_id: Uuid::nil(),
        origin_endpoint_id: Uuid::nil(),
        device: EndpointDeviceRef {
            device_id: "keyboard-default".to_string(),
            instance_id: None,
            display_name: "Aggregate Keyboard".to_string(),
            kind: EndpointEventKind::Keyboard,
            attribution: DeviceAttribution::Aggregate,
        },
        direction: EndpointEventDirection::Observed,
        source: EndpointEventSource::Hardware,
        kind: EndpointEventKind::Keyboard,
        payload: EndpointEventPayload::Keyboard {
            key: "ShiftLeft".to_string(),
            state: "Pressed".to_string(),
        },
        correlation_id: None,
    };

    for response in [
        DaemonResponse::EndpointEvents(vec![event.clone()]),
        DaemonResponse::EndpointEvent(event),
    ] {
        let (mut writer, mut reader) = duplex(4096);
        write_json_frame(&mut writer, &response).await.unwrap();
        let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

        assert_eq!(decoded, response);
    }
}

#[tokio::test]
async fn daemon_responses_round_trip_endpoint_inject_result() {
    let response = DaemonResponse::EndpointInjectResult(EndpointInjectResult {
        correlation_id: "test-shift-1".to_string(),
        target: EndpointInjectTarget::Local,
        accepted: true,
        backend_kind: Some(rshare_core::BackendKind::Portable),
        health: rshare_core::BackendHealth::Healthy,
        elapsed_ms: 3,
        loopback_event_id: Some(42),
        error: None,
    });

    let (mut writer, mut reader) = duplex(4096);
    write_json_frame(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

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
        form_factor: LocalAudioEndpointFormFactor::Speakers,
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
        form_factor: LocalAudioEndpointFormFactor::Speakers,
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

    write_json_frame(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

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
fn latency_feedback_defaults_to_safe_unavailable_state() {
    let snapshot: rshare_core::ServiceStatusSnapshot = serde_json::from_str(
        r#"{
            "device_id":"00000000-0000-0000-0000-000000000000",
            "device_name":"desktop",
            "hostname":"desktop-host",
            "bind_address":"0.0.0.0:27431",
            "discovery_port":27432,
            "pid":42,
            "discovered_devices":0,
            "connected_devices":0,
            "healthy":true
        }"#,
    )
    .unwrap();

    assert_eq!(
        snapshot.latency_feedback.transport.status,
        rshare_core::LatencyFeedbackStatus::Unavailable
    );
    assert!(snapshot.latency_feedback.remote_latency.devices.is_empty());
}

#[tokio::test]
async fn latency_feedback_status_response_round_trips_populated_payload() {
    let mut snapshot = ServiceStatusSnapshot::new(
        Uuid::nil(),
        "desktop".to_string(),
        "desktop-host".to_string(),
        "0.0.0.0:27431".to_string(),
        27432,
        42,
    );

    snapshot.latency_feedback.generated_at_ms = 123;
    snapshot.latency_feedback.local_input.status = rshare_core::LatencyFeedbackStatus::Healthy;
    snapshot.latency_feedback.local_input.event_count = 2;
    snapshot.latency_feedback.local_input.latest_sequence = Some(7);
    snapshot.latency_feedback.local_input.latest_event_ms = Some(111);
    snapshot
        .latency_feedback
        .local_input
        .latest_keyboard_event_ms = Some(109);
    snapshot
        .latency_feedback
        .local_input
        .latest_gamepad_event_ms = Some(115);
    snapshot.latency_feedback.local_input.latest_gamepad_id = Some(0);
    snapshot
        .latency_feedback
        .local_input
        .latest_gamepad_event_kind = Some("state".to_string());
    snapshot.latency_feedback.local_input.latest_gamepad_button = Some("South pressed".to_string());
    snapshot.latency_feedback.local_input.latest_gamepad_axis = Some("left_stick".to_string());
    snapshot.latency_feedback.local_input.capture_path = Some("portable".to_string());
    snapshot.latency_feedback.remote_latency.devices.push(
        rshare_core::RemoteDeviceLatencyFeedback {
            device_id: Uuid::nil(),
            status: rshare_core::LatencyFeedbackStatus::Healthy,
            device_name: Some("laptop".to_string()),
            latest_sequence: Some(9),
            last_probe_sent_ms: Some(100),
            last_ack_ms: Some(112),
            pending_duration_ms: None,
            network_round_trip_ms: Some(12),
            raw_round_trip_ms: None,
            estimated_one_way_ms: Some(6),
            remote_processing_ms: None,
            direction: Some("right".to_string()),
            summary: Some("12 ms round trip".to_string()),
        },
    );
    snapshot.latency_feedback.transport.status = rshare_core::LatencyFeedbackStatus::Healthy;
    snapshot.latency_feedback.transport.datagram_available = true;
    snapshot.latency_feedback.transport.realtime_degraded = false;
    snapshot.latency_feedback.transport.rtt_ms = Some(12);

    let response = DaemonResponse::Status(snapshot.clone());
    let (mut writer, mut reader) = duplex(4096);

    write_json_frame(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, response);

    let DaemonResponse::Status(decoded_snapshot) = &decoded else {
        panic!("expected status response");
    };
    assert_eq!(
        decoded_snapshot
            .latency_feedback
            .local_input
            .latest_gamepad_event_ms,
        Some(115)
    );
    assert_eq!(
        decoded_snapshot
            .latency_feedback
            .local_input
            .latest_gamepad_id,
        Some(0)
    );
    assert_eq!(
        decoded_snapshot
            .latency_feedback
            .local_input
            .latest_gamepad_event_kind
            .as_deref(),
        Some("state")
    );
    assert_eq!(
        decoded_snapshot
            .latency_feedback
            .local_input
            .latest_gamepad_button
            .as_deref(),
        Some("South pressed")
    );
    assert_eq!(
        decoded_snapshot
            .latency_feedback
            .local_input
            .latest_gamepad_axis
            .as_deref(),
        Some("left_stick")
    );
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

#[tokio::test]
async fn mobile_access_request_round_trips_over_json_frames() {
    let (mut writer, mut reader) = duplex(2048);
    let request = DaemonRequest::MobileAccess;

    write_json_frame(&mut writer, &request).await.unwrap();
    let decoded: DaemonRequest = read_json_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, request);

    let snapshot = MobileAccessSnapshot {
        enabled: true,
        bind_address: "0.0.0.0:27437".to_string(),
        page_url: "http://192.168.1.50:27437/mobile?t=abc123".to_string(),
        token: "abc123".to_string(),
        last_client_addr: Some("192.168.1.80:53120".to_string()),
        last_client_seen_at_ms: Some(1_800_000),
        client_count: 2,
    };
    let response = DaemonResponse::MobileAccess(snapshot.clone());
    let (mut writer, mut reader) = duplex(2048);

    write_json_frame(&mut writer, &response).await.unwrap();
    let decoded: DaemonResponse = read_json_frame(&mut reader).await.unwrap();

    assert_eq!(decoded, response);
    assert!(snapshot.page_url.contains(&snapshot.token));
    assert_eq!(
        snapshot.last_client_addr.as_deref(),
        Some("192.168.1.80:53120")
    );
    assert_eq!(snapshot.last_client_seen_at_ms, Some(1_800_000));
    assert_eq!(snapshot.client_count, 2);
}

#[test]
fn default_mobile_gateway_addr_uses_separate_port() {
    let ipc_addr = default_ipc_addr();
    let mobile_addr = default_mobile_gateway_addr();

    assert_ne!(mobile_addr.port(), ipc_addr.port());
    assert_eq!(mobile_addr.port(), 27437);
}
