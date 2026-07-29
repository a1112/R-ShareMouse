#![cfg(windows)]

use rshare_platform::windows::{
    is_driver_event_queue_empty, DriverWaitError, WindowsDriverCaptureEvent, WindowsDriverClient,
    WindowsDriverDeviceKind, WindowsDriverEventKind, WindowsDriverEventSource,
    WindowsDriverEventStream,
};
use std::time::{Duration, Instant};

fn wait_until_filter_reports_pending_waiter(timeout: Duration) -> bool {
    let client = WindowsDriverClient::open_filter().expect("open filter wait-state observer");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if client
            .query_wait_state()
            .map(|state| state.pending_waiters > 0)
            .unwrap_or(false)
        {
            return true;
        }
        std::thread::yield_now();
    }
    false
}

fn drain_filter_events(client: &WindowsDriverClient) {
    loop {
        match client.read_event() {
            Ok(_) => {}
            Err(error) if is_driver_event_queue_empty(&error) => return,
            Err(error) => panic!("drain filter events failed: {error}"),
        }
    }
}

#[test]
#[ignore = "requires installed rshare-filter driver"]
fn wait_event_returns_immediately_for_an_already_queued_event() {
    let emitter = WindowsDriverClient::open_filter().expect("open filter event emitter");
    drain_filter_events(&emitter);
    emitter
        .emit_test_packet(WindowsDriverDeviceKind::Mouse)
        .expect("queue test mouse event");
    let (mut stream, _cancel) =
        WindowsDriverEventStream::open_filter().expect("open overlapped filter stream");

    let event = stream.wait_event().expect("wait for queued filter event");

    assert!(matches!(
        event,
        WindowsDriverCaptureEvent::Input(event)
            if event.event_kind == WindowsDriverEventKind::Synthetic
    ));
}

#[test]
#[ignore = "requires installed rshare-filter driver"]
fn wait_event_wakes_without_polling_delay() {
    let emitter = WindowsDriverClient::open_filter().expect("open filter event emitter");
    drain_filter_events(&emitter);
    let (mut stream, _cancel) =
        WindowsDriverEventStream::open_filter().expect("open overlapped filter stream");
    let waiter = std::thread::spawn(move || stream.wait_event());
    assert!(wait_until_filter_reports_pending_waiter(
        Duration::from_secs(1)
    ));
    emitter
        .emit_test_packet(WindowsDriverDeviceKind::Mouse)
        .expect("emit test mouse event");

    let event = waiter.join().expect("join waiter").expect("wait event");
    assert!(matches!(
        event,
        WindowsDriverCaptureEvent::Input(event)
            if event.event_kind == WindowsDriverEventKind::Synthetic
    ));
}

#[test]
#[ignore = "requires installed rshare-filter driver"]
fn pending_wait_can_be_cancelled_from_another_thread() {
    let observer = WindowsDriverClient::open_filter().expect("open filter event observer");
    drain_filter_events(&observer);
    let (mut stream, cancel) =
        WindowsDriverEventStream::open_filter().expect("open overlapped filter stream");
    let waiter = std::thread::spawn(move || stream.wait_event());
    assert!(wait_until_filter_reports_pending_waiter(
        Duration::from_secs(1)
    ));

    cancel.cancel().expect("cancel exact pending wait");

    assert!(matches!(
        waiter.join().expect("join waiter"),
        Err(DriverWaitError::Cancelled)
    ));
}

#[test]
#[ignore = "requires installed RShare Windows test drivers and test-signing mode"]
fn rshare_windows_filter_and_vhid_smoke() {
    let filter = WindowsDriverClient::open_filter().expect("open RShare filter control device");
    let version = filter.query_version().expect("query filter version");
    assert_eq!(version.abi, 1);
    assert!(version.major > 0 || version.minor >= 4);

    let capabilities = filter
        .query_capabilities()
        .expect("query filter capabilities");
    assert!(capabilities.filter_events);
    assert!(capabilities.filter_semantic_queue);
    assert!(capabilities.wait_event);
    let stats = filter.query_stats().expect("query semantic filter stats");
    assert!(stats.queue_depth <= stats.queue_capacity);

    match filter.read_event() {
        Ok(_) => {}
        Err(error) if is_driver_event_queue_empty(&error) => {}
        Err(error) => panic!("unexpected filter read error: {error}"),
    }

    filter
        .emit_test_packet(WindowsDriverDeviceKind::Keyboard)
        .expect("emit synthetic filter packet");
    let event = match filter.read_event().expect("read synthetic filter packet") {
        rshare_platform::windows::WindowsDriverCaptureEvent::Input(event) => event,
        rshare_platform::windows::WindowsDriverCaptureEvent::Status(status) => {
            panic!("unexpected filter capture status: {status:?}")
        }
    };
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
