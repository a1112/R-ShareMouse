use rshare_core::{
    DisplayCaptureRequest, DisplayIdentifyRequest, DisplayOperationStatus, DisplayOrientation,
    DisplaySettingsUpdateRequest, LocalDisplayInfo,
};

#[test]
fn display_settings_contract_local_display_info_deserializes_older_snapshots() {
    let json = r#"{
        "display_id":"primary",
        "x":0,
        "y":0,
        "width":1920,
        "height":1080,
        "primary":true
    }"#;

    let display: LocalDisplayInfo = serde_json::from_str(json).unwrap();

    assert_eq!(display.display_id, "primary");
    assert_eq!(display.width, 1920);
    assert_eq!(display.orientation, DisplayOrientation::Landscape);
    assert!(!display.write_capabilities.scale);
}

#[test]
fn display_settings_contract_update_request_round_trips() {
    let request = DisplaySettingsUpdateRequest {
        display_id: "display-1".to_string(),
        width: Some(2560),
        height: Some(1440),
        refresh_rate_millihz: Some(144_000),
        orientation: Some(DisplayOrientation::Landscape),
        primary: Some(true),
        x: Some(0),
        y: Some(0),
        scale_percent: Some(150),
    };

    let json = serde_json::to_string(&request).unwrap();
    let decoded: DisplaySettingsUpdateRequest = serde_json::from_str(&json).unwrap();

    assert_eq!(decoded.display_id, "display-1");
    assert_eq!(decoded.width, Some(2560));
    assert_eq!(decoded.height, Some(1440));
    assert_eq!(decoded.refresh_rate_millihz, Some(144_000));
    assert_eq!(decoded.orientation, Some(DisplayOrientation::Landscape));
    assert_eq!(decoded.primary, Some(true));
    assert_eq!(decoded.x, Some(0));
    assert_eq!(decoded.y, Some(0));
    assert_eq!(decoded.scale_percent, Some(150));
}

#[test]
fn display_settings_contract_operation_status_serializes_stable_names() {
    assert_eq!(
        serde_json::to_string(&DisplayOperationStatus::RequiresSystemSettings).unwrap(),
        r#""RequiresSystemSettings""#
    );
}

#[test]
fn display_settings_contract_capture_and_identify_requests_round_trip() {
    let capture = DisplayCaptureRequest {
        display_id: "primary".to_string(),
        max_width: Some(640),
        format: rshare_core::DisplayCaptureFormat::Png,
    };
    let identify = DisplayIdentifyRequest {
        duration_ms: Some(2500),
    };

    let decoded_capture: DisplayCaptureRequest =
        serde_json::from_str(&serde_json::to_string(&capture).unwrap()).unwrap();
    let decoded_identify: DisplayIdentifyRequest =
        serde_json::from_str(&serde_json::to_string(&identify).unwrap()).unwrap();

    assert_eq!(decoded_capture, capture);
    assert_eq!(decoded_identify, identify);
}
