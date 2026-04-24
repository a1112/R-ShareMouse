#![cfg(windows)]

use rshare_platform::windows::{
    is_driver_event_queue_empty, WindowsDriverClient, WindowsDriverDeviceKind,
    WindowsDriverEventKind, WindowsDriverEventSource,
};

#[test]
#[ignore = "requires installed RShare Windows test drivers and test-signing mode"]
fn rshare_windows_filter_and_vhid_smoke() {
    let filter = WindowsDriverClient::open_filter().expect("open RShare filter control device");
    let version = filter.query_version().expect("query filter version");
    assert_eq!(version.abi, 1);

    let capabilities = filter
        .query_capabilities()
        .expect("query filter capabilities");
    assert!(capabilities.filter_events);

    match filter.read_event() {
        Ok(_) => {}
        Err(error) if is_driver_event_queue_empty(&error) => {}
        Err(error) => panic!("unexpected filter read error: {error}"),
    }

    filter
        .emit_test_packet(WindowsDriverDeviceKind::Keyboard)
        .expect("emit synthetic filter packet");
    let event = filter.read_event().expect("read synthetic filter packet");
    assert_eq!(event.source, WindowsDriverEventSource::DriverTest);
    assert_eq!(event.device_kind, WindowsDriverDeviceKind::Keyboard);
    assert_eq!(event.event_kind, WindowsDriverEventKind::Synthetic);

    let vhid = WindowsDriverClient::open_vhid().expect("open RShare virtual HID control device");
    let capabilities = vhid.query_capabilities().expect("query vhid capabilities");
    assert!(capabilities.virtual_keyboard);
    assert!(capabilities.virtual_mouse);

    vhid.inject_keyboard(0xA0, true)
        .expect("inject ShiftLeft down through vhid");
    vhid.inject_keyboard(0xA0, false)
        .expect("inject ShiftLeft up through vhid");
    vhid.inject_mouse_move(8, 8)
        .expect("inject mouse move through vhid");
    vhid.inject_mouse_move(-8, -8)
        .expect("restore mouse position through vhid");
}
