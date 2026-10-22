#![cfg(target_os = "macos")]
use rshare_core::{
    VirtualDisplayCreateRequest, VirtualDisplayOperationStatus, VirtualDisplayRemoveRequest,
};

#[test]
#[ignore = "creates a real temporary macOS display; run explicitly on an unlocked desktop"]
fn creates_an_extended_system_display_and_removes_it() {
    let request = VirtualDisplayCreateRequest {
        id: Some("macos-usb-acceptance".into()),
        width: 1920,
        height: 1080,
        refresh_rate_millihz: Some(60000),
        name: Some("RShare USB Test".into()),
    };
    let result = rshare_platform::virtual_display::create_virtual_display(&request).unwrap();
    assert_eq!(
        result.status,
        VirtualDisplayOperationStatus::Created,
        "{result:?}"
    );
    let display = result.display.unwrap();
    let state = rshare_platform::display::query_display_state().unwrap();
    let visible = state
        .displays
        .iter()
        .any(|d| Some(&d.display_id) == display.display_id.as_ref());
    let removed =
        rshare_platform::virtual_display::remove_virtual_display(&VirtualDisplayRemoveRequest {
            id: display.id,
        })
        .unwrap();
    assert!(
        visible,
        "virtual screen must be a real enumerated OS display"
    );
    assert_eq!(removed.status, VirtualDisplayOperationStatus::Removed);
    assert!(!rshare_platform::display::query_display_state()
        .unwrap()
        .displays
        .iter()
        .any(|d| Some(&d.display_id) == display.display_id.as_ref()));
}

#[test]
#[ignore = "captures a real virtual screen; needs macOS Screen Recording permission"]
fn captures_real_virtual_screen_as_h264() {
    use std::time::Duration;
    let id = "macos-capture-acceptance";
    let created =
        rshare_platform::virtual_display::create_virtual_display(&VirtualDisplayCreateRequest {
            id: Some(id.into()),
            width: 1280,
            height: 720,
            refresh_rate_millihz: Some(60000),
            name: Some("RShare H264 Test".into()),
        })
        .unwrap();
    let capture = rshare_platform::macos_display::H264Capture::start(
        created.display.unwrap().display_id.as_deref().unwrap(),
        1280,
        720,
        30,
    );
    let result = capture.and_then(|capture| {
        let frame = capture.frames.recv_timeout(Duration::from_secs(10))?;
        capture.check()?;
        anyhow::ensure!(frame.len() > 8, "H.264 access unit must contain bytes");
        let sps = u16::from_be_bytes([frame[0], frame[1]]) as usize;
        let pps = u16::from_be_bytes([frame[2], frame[3]]) as usize;
        anyhow::ensure!(
            sps > 0 && pps > 0 && frame[4] & 31 == 7 && frame[4 + sps] & 31 == 8,
            "First frame needs H.264 SPS/PPS"
        );
        Ok(())
    });
    eprintln!("capture result: {result:?}");
    rshare_platform::virtual_display::remove_virtual_display(&VirtualDisplayRemoveRequest {
        id: id.into(),
    })
    .unwrap();
    result.unwrap();
}
