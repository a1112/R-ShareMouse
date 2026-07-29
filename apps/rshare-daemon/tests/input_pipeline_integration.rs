use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rshare_core::{
    BackendHealth, BackendKind, ClockDomainId, ControlConnectionId, ControlSessionState, DeviceId,
    Direction, InputRouter, KeyState, LayoutGraph, LayoutLink, LayoutNode, MonotonicStamp,
    PixelRect, RealtimeInputFrame, ReleaseAllReason, ReliableInputEvent, ReliableInputFrame,
    RouterCommand, SessionEpoch, VirtualDesktopGeometry,
};
use rshare_daemon::input_runtime::{
    dispatch_system_safety_event, run_authenticated_input_peers, InputDispatch, InputRuntime,
    InputTransport,
};
use rshare_daemon::input_state::{input_state_channel, ControlMetrics, InputStateFeeds};
use rshare_input::{
    ButtonState as CaptureButtonState, CaptureOrigin, CaptureSource, CapturedInputPayload,
    ContinuousInput, InjectBackend, InjectionActorConfig, InputEvent, InputInjectionHandle,
    KeyCode, MouseButton as CaptureMouseButton, PointerSample, PushOutcome, SemanticInputIngress,
};
use rshare_net::{encryption::PeerCertificateFingerprint, handshake::PeerAuthContext, PeerInbound};
use rshare_platform::SystemSafetyEvent;
use tokio::sync::{broadcast, mpsc};

#[derive(Debug)]
struct NoopInjectBackend;

impl InjectBackend for NoopInjectBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Portable
    }

    fn health(&self) -> BackendHealth {
        BackendHealth::Healthy
    }

    fn inject(&mut self, _event: InputEvent) -> anyhow::Result<()> {
        Ok(())
    }

    fn inject_relative_pointer(&mut self, _dx: i32, _dy: i32) -> anyhow::Result<()> {
        Ok(())
    }

    fn is_active(&self) -> bool {
        true
    }
}

#[derive(Debug, Default)]
struct BlockingInjectState {
    blocked: bool,
    release: bool,
    events: Vec<InputEvent>,
}

#[derive(Debug, Default)]
struct BlockingInjectGate {
    state: Mutex<BlockingInjectState>,
    changed: Condvar,
}

impl BlockingInjectGate {
    fn wait_until_blocked(&self) {
        let mut state = self.state.lock().unwrap();
        while !state.blocked {
            state = self.changed.wait(state).unwrap();
        }
    }

    fn unblock(&self) {
        let mut state = self.state.lock().unwrap();
        state.release = true;
        self.changed.notify_all();
    }

    fn events(&self) -> Vec<InputEvent> {
        self.state.lock().unwrap().events.clone()
    }
}

#[derive(Debug)]
struct BlockingInjectBackend {
    gate: Arc<BlockingInjectGate>,
}

impl InjectBackend for BlockingInjectBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Portable
    }

    fn health(&self) -> BackendHealth {
        BackendHealth::Healthy
    }

    fn inject(&mut self, event: InputEvent) -> anyhow::Result<()> {
        let should_block = matches!(
            event,
            InputEvent::Key {
                keycode: KeyCode::Raw(0x41),
                state: CaptureButtonState::Pressed,
            }
        );
        let mut state = self.gate.state.lock().unwrap();
        state.events.push(event);
        if should_block {
            state.blocked = true;
            self.gate.changed.notify_all();
            while !state.release {
                state = self.gate.changed.wait(state).unwrap();
            }
        }
        Ok(())
    }

    fn inject_relative_pointer(&mut self, _dx: i32, _dy: i32) -> anyhow::Result<()> {
        Ok(())
    }

    fn is_active(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct RecordingTransport {
    events: Mutex<Vec<InputDispatch>>,
}

#[derive(Default)]
struct RotatingTransport {
    generation: AtomicU64,
    events: Mutex<Vec<(u64, InputDispatch)>>,
}

#[derive(Default)]
struct BlockedTransportState {
    now_ms: u64,
    blocked_until_ms: u64,
    latest_realtime: Option<RealtimeInputFrame>,
    reliable: Vec<ReliableInputFrame>,
    sent_realtime: Vec<RealtimeInputFrame>,
    sent_reliable: Vec<ReliableInputFrame>,
    max_pending: usize,
}

struct BoundedBlockedTransport {
    capacity: usize,
    state: Mutex<BlockedTransportState>,
}

impl BoundedBlockedTransport {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(BlockedTransportState::default()),
        }
    }

    fn block_for(&self, duration: Duration) {
        let mut state = self.state.lock().unwrap();
        state.blocked_until_ms = state
            .now_ms
            .checked_add(duration.as_millis() as u64)
            .unwrap();
        state.latest_realtime = None;
        state.reliable.clear();
        state.sent_realtime.clear();
        state.sent_reliable.clear();
        state.max_pending = 0;
    }

    fn advance(&self, duration: Duration) {
        let mut state = self.state.lock().unwrap();
        state.now_ms = state
            .now_ms
            .checked_add(duration.as_millis() as u64)
            .unwrap();
    }

    fn flush_if_unblocked(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.now_ms < state.blocked_until_ms {
            return false;
        }
        if let Some(frame) = state.latest_realtime.take() {
            state.sent_realtime.push(frame);
        }
        let reliable = std::mem::take(&mut state.reliable);
        state.sent_reliable.extend(reliable);
        true
    }
}

impl InputTransport for BoundedBlockedTransport {
    type Binding = DeviceId;

    fn bind(&self, target: DeviceId) -> Option<Self::Binding> {
        Some(target)
    }

    fn try_send_realtime(&self, _target: &Self::Binding, frame: RealtimeInputFrame) {
        let mut state = self.state.lock().unwrap();
        if state.now_ms >= state.blocked_until_ms {
            state.sent_realtime.push(frame);
            return;
        }
        state.latest_realtime = Some(frame);
        let pending = usize::from(state.latest_realtime.is_some()) + state.reliable.len();
        state.max_pending = state.max_pending.max(pending);
    }

    fn try_send_reliable(&self, _target: &Self::Binding, frame: ReliableInputFrame) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.now_ms >= state.blocked_until_ms {
            state.sent_reliable.push(frame);
            return true;
        }
        let pending = usize::from(state.latest_realtime.is_some()) + state.reliable.len();
        if pending >= self.capacity {
            return false;
        }
        state.reliable.push(frame);
        let pending = usize::from(state.latest_realtime.is_some()) + state.reliable.len();
        state.max_pending = state.max_pending.max(pending);
        true
    }
}

impl RotatingTransport {
    fn replace_generation(&self, generation: u64) {
        self.generation.store(generation, Ordering::Release);
    }
}

impl InputTransport for RotatingTransport {
    type Binding = (DeviceId, u64);

    fn bind(&self, target: DeviceId) -> Option<Self::Binding> {
        Some((target, self.generation.load(Ordering::Acquire)))
    }

    fn try_send_realtime(&self, binding: &Self::Binding, frame: RealtimeInputFrame) {
        self.events.lock().unwrap().push((
            binding.1,
            InputDispatch::Realtime {
                target: binding.0,
                frame,
            },
        ));
    }

    fn try_send_reliable(&self, binding: &Self::Binding, frame: ReliableInputFrame) -> bool {
        self.events.lock().unwrap().push((
            binding.1,
            InputDispatch::Reliable {
                target: binding.0,
                frame,
            },
        ));
        true
    }
}

impl InputTransport for RecordingTransport {
    type Binding = DeviceId;

    fn bind(&self, target: DeviceId) -> Option<Self::Binding> {
        Some(target)
    }

    fn try_send_realtime(&self, target: &Self::Binding, frame: RealtimeInputFrame) {
        self.events.lock().unwrap().push(InputDispatch::Realtime {
            target: *target,
            frame,
        });
    }

    fn try_send_reliable(&self, target: &Self::Binding, frame: ReliableInputFrame) -> bool {
        self.events.lock().unwrap().push(InputDispatch::Reliable {
            target: *target,
            frame,
        });
        true
    }
}

fn stamp(value: u64) -> MonotonicStamp {
    MonotonicStamp::new(ClockDomainId(1), value)
}

fn linked_router() -> (DeviceId, DeviceId, InputRouter) {
    let local = DeviceId::new_v4();
    let remote = DeviceId::new_v4();
    let mut layout = LayoutGraph::new(local);
    layout.add_node(LayoutNode::new(local, 0, 0, 100, 100));
    layout.add_node(LayoutNode::new(remote, 100, 0, 100, 100));
    layout.add_link(LayoutLink::new(
        local,
        Direction::Right,
        remote,
        Direction::Left,
    ));
    let router = InputRouter::new(
        local,
        layout,
        VirtualDesktopGeometry::new(PixelRect::new(0, 0, 100, 100)),
        [remote],
    );
    (local, remote, router)
}

fn runtime(
    capacity: usize,
) -> (
    rshare_input::SemanticInputProducer,
    InputRuntime<RecordingTransport>,
    Arc<RecordingTransport>,
) {
    let (producer, runtime, transport, _feeds) = runtime_with_feeds(capacity);
    (producer, runtime, transport)
}

fn runtime_with_feeds(
    capacity: usize,
) -> (
    rshare_input::SemanticInputProducer,
    InputRuntime<RecordingTransport>,
    Arc<RecordingTransport>,
    InputStateFeeds,
) {
    let (_, _, router) = linked_router();
    let (producer, consumer) = SemanticInputIngress::new(capacity);
    let transport = Arc::new(RecordingTransport::default());
    let (state, feeds) = input_state_channel(8);
    let metrics = Arc::new(ControlMetrics::default());
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    (
        producer,
        InputRuntime::new(
            consumer,
            router,
            transport.clone(),
            state,
            metrics,
            injection,
        ),
        transport,
        feeds,
    )
}

fn origin(source: CaptureSource) -> CaptureOrigin {
    CaptureOrigin {
        source,
        device_token: 7,
        instance_token: 9,
    }
}

async fn enter_remote<T: InputTransport>(
    producer: &rshare_input::SemanticInputProducer,
    runtime: &mut InputRuntime<T>,
) {
    assert_eq!(
        producer.try_push(producer.capture(
            origin(CaptureSource::PortableHook),
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Absolute {
                x: 99,
                y: 50
            },)),
        )),
        PushOutcome::Enqueued
    );
    assert!(runtime.process_next().await);
}

#[tokio::test]
async fn all_capture_sources_share_one_router_sequence_domain() {
    let (producer, mut runtime, transport) = runtime(16);
    enter_remote(&producer, &mut runtime).await;

    let sources = [
        CaptureSource::PortableHook,
        CaptureSource::WindowsHook,
        CaptureSource::WindowsFilter,
        CaptureSource::Evdev,
        CaptureSource::Gamepad,
    ];
    for (index, source) in sources.into_iter().enumerate() {
        assert_eq!(
            producer.try_push_event(
                origin(source),
                InputEvent::key(
                    KeyCode::Raw(0x41 + index as u32),
                    CaptureButtonState::Pressed,
                ),
            ),
            PushOutcome::Enqueued
        );
        assert!(runtime.process_next().await);
    }

    let reliable = transport
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            InputDispatch::Reliable { frame, .. }
                if matches!(frame.event, ReliableInputEvent::Key { .. }) =>
            {
                Some(frame.sequence)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(reliable.windows(2).all(|pair| pair[1] == pair[0] + 1), true);
    assert_eq!(reliable.len(), 5);
}

#[tokio::test]
async fn transport_control_precedes_saturated_ui_diagnostics() {
    let (producer, mut runtime, transport) = runtime(8);
    enter_remote(&producer, &mut runtime).await;
    assert_eq!(
        producer.try_push_event(
            origin(CaptureSource::WindowsHook),
            InputEvent::key(KeyCode::Raw(0x41), CaptureButtonState::Pressed),
        ),
        PushOutcome::Enqueued
    );
    assert!(runtime.process_next().await);
    assert!(transport
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|event| matches!(
            event,
            InputDispatch::Reliable { frame, .. }
                if matches!(frame.event, ReliableInputEvent::Key { keycode: 0x41, .. })
        )));
}

#[tokio::test]
async fn closed_noop_safety_lane_does_not_stop_input_runtime() {
    let (_, _, router) = linked_router();
    let (producer, consumer) = SemanticInputIngress::new(8);
    let transport = Arc::new(RecordingTransport::default());
    let (state, _feeds) = input_state_channel(1);
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    let runtime = InputRuntime::new(
        consumer,
        router,
        transport.clone(),
        state,
        Arc::new(ControlMetrics::default()),
        injection,
    );
    let (command_tx, command_rx) = mpsc::channel(1);
    let (safety_tx, safety_rx) = mpsc::unbounded_channel();
    drop(safety_tx);
    let worker = tokio::spawn(runtime.run_with_safety(command_rx, safety_rx));

    assert_eq!(
        producer.try_push(producer.capture(
            origin(CaptureSource::PortableHook),
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Absolute {
                x: 99,
                y: 50
            },)),
        )),
        PushOutcome::Enqueued
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while !transport.events.lock().unwrap().iter().any(|event| {
        matches!(
            event,
            InputDispatch::Reliable {
                frame: ReliableInputFrame {
                    event: ReliableInputEvent::Enter { .. },
                    ..
                },
                ..
            }
        )
    }) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "closed no-op safety lane stopped ordinary input"
        );
        tokio::task::yield_now().await;
    }
    drop(command_tx);
    worker.await.unwrap();
}

#[tokio::test]
async fn blocked_runtime_keeps_latest_motion_and_reliable_key() {
    let (_, _, router) = linked_router();
    let (producer, consumer) = SemanticInputIngress::new(8);
    let transport = Arc::new(BoundedBlockedTransport::new(2));
    let (state, _feeds) = input_state_channel(1);
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    let mut runtime = InputRuntime::new(
        consumer,
        router,
        transport.clone(),
        state,
        Arc::new(ControlMetrics::default()),
        injection,
    );
    enter_remote(&producer, &mut runtime).await;
    transport.block_for(Duration::from_millis(100));

    for x in 0..100_000 {
        assert_eq!(
            producer.try_push(producer.capture(
                origin(CaptureSource::WindowsFilter),
                CapturedInputPayload::Continuous(ContinuousInput::Pointer(
                    PointerSample::Relative {
                        dx: x,
                        dy: 0,
                        observed_x: Some(x),
                        observed_y: Some(50),
                    },
                )),
            )),
            PushOutcome::Enqueued
        );
        assert!(runtime.process_ready());
    }
    assert_eq!(
        producer.try_push_event(
            origin(CaptureSource::WindowsFilter),
            InputEvent::key(KeyCode::Raw(0x41), CaptureButtonState::Pressed),
        ),
        PushOutcome::Enqueued
    );
    while runtime.process_ready() {}

    transport.advance(Duration::from_millis(99));
    assert!(!transport.flush_if_unblocked());
    {
        let state = transport.state.lock().unwrap();
        assert!(state.sent_realtime.is_empty());
        assert!(state.sent_reliable.is_empty());
    }
    transport.advance(Duration::from_millis(1));
    assert!(transport.flush_if_unblocked());
    let state = transport.state.lock().unwrap();
    assert!(matches!(
        state.sent_realtime.last().map(|frame| &frame.payload),
        Some(rshare_core::RealtimeInputPayload::RelativeMouse { dx: 99_999, dy: 0 })
    ));
    assert!(state
        .sent_reliable
        .iter()
        .any(|frame| matches!(frame.event, ReliableInputEvent::Key { keycode: 0x41, .. })));
    assert!(
        state.max_pending <= transport.capacity,
        "bounded transport exceeded capacity: {} > {}",
        state.max_pending,
        transport.capacity
    );
}

#[tokio::test]
async fn reliable_overflow_suspends_and_emits_emergency_release() {
    let (producer, mut runtime, transport) = runtime(2);
    enter_remote(&producer, &mut runtime).await;
    assert_eq!(
        producer.try_push_event(
            origin(CaptureSource::PortableHook),
            InputEvent::key(KeyCode::Raw(0x41), CaptureButtonState::Pressed),
        ),
        PushOutcome::Enqueued
    );
    assert!(runtime.process_next().await);

    for keycode in [0x42, 0x43] {
        assert_eq!(
            producer.try_push_event(
                origin(CaptureSource::PortableHook),
                InputEvent::key(KeyCode::Raw(keycode), CaptureButtonState::Pressed),
            ),
            PushOutcome::Enqueued
        );
    }
    assert_eq!(
        producer.try_push_event(
            origin(CaptureSource::PortableHook),
            InputEvent::key(KeyCode::Raw(0x44), CaptureButtonState::Pressed),
        ),
        PushOutcome::ReliableOverflow
    );
    assert!(runtime.process_next().await);
    assert!(transport.events.lock().unwrap().iter().any(|event| matches!(
        event,
        InputDispatch::Reliable { frame, .. }
            if matches!(frame.event, ReliableInputEvent::ReleaseAll { reason: ReleaseAllReason::BackendFailure })
    )));
}

#[tokio::test]
async fn windows_filter_overflow_status_maps_to_suspension_epoch_and_targeted_release() {
    let (producer, mut runtime, transport, feeds) = runtime_with_feeds(8);
    enter_remote(&producer, &mut runtime).await;
    assert_eq!(
        producer.try_push_event(
            origin(CaptureSource::WindowsFilter),
            InputEvent::key(KeyCode::Raw(0x41), CaptureButtonState::Pressed),
        ),
        PushOutcome::Enqueued
    );
    assert!(runtime.process_next().await);
    assert_eq!(
        producer.try_push_event(
            origin(CaptureSource::WindowsFilter),
            InputEvent::mouse_button(CaptureMouseButton::Left, CaptureButtonState::Pressed,),
        ),
        PushOutcome::Enqueued
    );
    assert!(runtime.process_next().await);
    let pressed_projection = feeds.authoritative_rx.borrow().clone();
    assert!(pressed_projection.discrete.pressed_keys.contains(&0x41));
    assert!(pressed_projection
        .discrete
        .pressed_buttons
        .contains(&rshare_core::MouseButton::Left));
    let before = transport.events.lock().unwrap().clone();
    let (target, enter_epoch) = before
        .iter()
        .find_map(|event| match event {
            InputDispatch::Reliable { target, frame }
                if matches!(frame.event, ReliableInputEvent::Enter { .. }) =>
            {
                Some((*target, frame.session_epoch))
            }
            _ => None,
        })
        .expect("remote session must publish Enter");

    let rshare_input::backend::WindowsFilterCaptureOutput::Fault(fault) =
        rshare_input::backend::adapt_windows_filter_capture_event(
            rshare_platform::windows::WindowsDriverCaptureEvent::Status(
                rshare_platform::windows::RawCaptureStatus::ReliableOverflow,
            ),
        )
    else {
        panic!("overflow status must map to an ingress fault");
    };
    producer.report_fault(fault);
    assert!(runtime.process_next().await);
    let advanced_epoch = runtime.session_epoch();
    let recovered_projection = feeds.authoritative_rx.borrow().clone();
    assert!(matches!(
        recovered_projection.session,
        ControlSessionState::Suspended { .. }
    ));
    assert_eq!(recovered_projection.discrete.session_epoch, advanced_epoch);
    assert!(recovered_projection.discrete.pressed_keys.is_empty());
    assert!(recovered_projection.discrete.pressed_buttons.is_empty());

    let events = transport.events.lock().unwrap();
    let release = events
        .iter()
        .find_map(|event| match event {
            InputDispatch::Reliable {
                target: release_target,
                frame,
            } if matches!(
                frame.event,
                ReliableInputEvent::ReleaseAll {
                    reason: ReleaseAllReason::BackendFailure
                }
            ) =>
            {
                Some((*release_target, frame.session_epoch))
            }
            _ => None,
        })
        .expect("overflow must publish targeted emergency ReleaseAll");
    assert_eq!(release.0, target);
    assert_eq!(
        release.1, enter_epoch,
        "targeted release must use the remote-owned epoch"
    );
    assert!(
        advanced_epoch.0 > enter_epoch.0,
        "overflow must advance session epoch"
    );
}

#[tokio::test]
async fn disconnect_and_shutdown_release_held_state() {
    for command in [
        RouterCommand::ConnectivityChanged {
            peer: linked_router().1,
            connected: false,
        },
        RouterCommand::Shutdown,
    ] {
        let (producer, mut runtime, transport) = runtime(8);
        enter_remote(&producer, &mut runtime).await;
        assert_eq!(
            producer.try_push_event(
                origin(CaptureSource::PortableHook),
                InputEvent::key(KeyCode::Raw(0x41), CaptureButtonState::Pressed),
            ),
            PushOutcome::Enqueued
        );
        assert!(runtime.process_next().await);
        let target = transport
            .events
            .lock()
            .unwrap()
            .iter()
            .find_map(|event| match event {
                InputDispatch::Reliable { target, frame }
                    if matches!(frame.event, ReliableInputEvent::Enter { .. }) =>
                {
                    Some(*target)
                }
                _ => None,
            })
            .unwrap();
        let command = match command {
            RouterCommand::ConnectivityChanged { .. } => RouterCommand::ConnectivityChanged {
                peer: target,
                connected: false,
            },
            other => other,
        };
        runtime.handle_command(command);
        assert!(transport
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(
                event,
                InputDispatch::Reliable { frame, .. }
                    if matches!(frame.event, ReliableInputEvent::ReleaseAll { .. })
            )));
    }
}

#[tokio::test]
async fn release_uses_router_embedded_target_after_session_suspends() {
    let (producer, mut runtime, transport) = runtime(8);
    enter_remote(&producer, &mut runtime).await;
    let target = transport
        .events
        .lock()
        .unwrap()
        .iter()
        .find_map(|event| match event {
            InputDispatch::Reliable { target, frame }
                if matches!(frame.event, ReliableInputEvent::Enter { .. }) =>
            {
                Some(*target)
            }
            _ => None,
        })
        .unwrap();
    runtime.handle_command(RouterCommand::BackendDegraded);
    assert!(transport
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|event| matches!(
            event,
            InputDispatch::Reliable { target: sent_target, frame }
                if *sent_target == target
                    && matches!(frame.event, ReliableInputEvent::ReleaseAll { .. })
        )));
}

#[tokio::test]
async fn active_session_keeps_exact_transport_generation_after_reconnect() {
    let (_, _, router) = linked_router();
    let (producer, consumer) = SemanticInputIngress::new(8);
    let transport = Arc::new(RotatingTransport::default());
    transport.replace_generation(1);
    let (state, _feeds) = input_state_channel(1);
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    let mut runtime = InputRuntime::new(
        consumer,
        router,
        transport.clone(),
        state,
        Arc::new(ControlMetrics::default()),
        injection,
    );
    assert_eq!(
        producer.try_push(producer.capture(
            origin(CaptureSource::PortableHook),
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Absolute {
                x: 99,
                y: 50
            },)),
        )),
        PushOutcome::Enqueued
    );
    assert!(runtime.process_next().await);

    transport.replace_generation(2);
    assert_eq!(
        producer.try_push_event(
            origin(CaptureSource::PortableHook),
            InputEvent::key(KeyCode::Raw(0x41), CaptureButtonState::Pressed),
        ),
        PushOutcome::Enqueued
    );
    assert!(runtime.process_next().await);
    let generations = transport
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(generation, dispatch)| match dispatch {
            InputDispatch::Reliable { frame, .. }
                if matches!(
                    frame.event,
                    ReliableInputEvent::Enter { .. } | ReliableInputEvent::Key { .. }
                ) =>
            {
                Some(*generation)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(generations, vec![1, 1]);
}

fn peer(
    peer_id: DeviceId,
    connection_id: ControlConnectionId,
) -> (
    PeerInbound,
    mpsc::Sender<RealtimeInputFrame>,
    mpsc::Sender<ReliableInputFrame>,
) {
    let auth = Arc::new(PeerAuthContext {
        peer_id,
        certificate_fingerprint: PeerCertificateFingerprint::from_der(b"test"),
        control_connection_id: connection_id,
    });
    let (realtime_tx, realtime_rx) = mpsc::channel(4);
    let (reliable_tx, reliable_input_rx) = mpsc::channel(4);
    let (_control_tx, control_rx) = mpsc::channel(1);
    let (_telemetry_tx, telemetry_rx) = mpsc::channel(1);
    let (_bulk_tx, bulk_rx) = mpsc::channel(1);
    (
        PeerInbound {
            auth,
            realtime_rx,
            reliable_input_rx,
            control_rx,
            telemetry_rx,
            bulk_rx,
        },
        realtime_tx,
        reliable_tx,
    )
}

#[tokio::test]
async fn authenticated_generation_rejects_old_connection_after_reconnect() {
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    let peer_id = DeviceId::new_v4();
    let (peer_tx, peer_rx) = mpsc::channel(4);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let worker = tokio::spawn(run_authenticated_input_peers(
        peer_rx,
        injection.clone(),
        Duration::from_secs(1),
        shutdown_rx,
    ));
    let (old, _old_realtime, old_reliable) = peer(peer_id, ControlConnectionId::new());
    let (new, _new_realtime, new_reliable) = peer(peer_id, ControlConnectionId::new());
    peer_tx.send(old).await.unwrap();
    peer_tx.send(new).await.unwrap();
    tokio::task::yield_now().await;

    let enter = |epoch| ReliableInputFrame {
        protocol_version: rshare_core::INPUT_PROTOCOL_VERSION,
        session_epoch: SessionEpoch(epoch),
        sequence: 0,
        captured_at: stamp(1),
        event: ReliableInputEvent::Enter {
            target_display_id: "primary".to_owned(),
            x: 0,
            y: 0,
        },
    };
    old_reliable.send(enter(1)).await.unwrap();
    new_reliable.send(enter(2)).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while injection
        .latest_timing()
        .is_none_or(|sample| sample.epoch != SessionEpoch(2))
    {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    shutdown_tx.send(()).unwrap();
    worker.await.unwrap();
    assert_eq!(
        injection.latest_timing().map(|sample| sample.epoch),
        Some(SessionEpoch(2))
    );
}

#[tokio::test]
async fn reliable_lane_drains_after_realtime_lane_closes() {
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    let (peer_tx, peer_rx) = mpsc::channel(1);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let worker = tokio::spawn(run_authenticated_input_peers(
        peer_rx,
        injection.clone(),
        Duration::from_secs(1),
        shutdown_rx,
    ));
    let (inbound, realtime, reliable) = peer(DeviceId::new_v4(), ControlConnectionId::new());
    peer_tx.send(inbound).await.unwrap();
    drop(realtime);
    reliable
        .send(ReliableInputFrame {
            protocol_version: rshare_core::INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(1),
            sequence: 0,
            captured_at: stamp(1),
            event: ReliableInputEvent::Enter {
                target_display_id: "primary".to_owned(),
                x: 0,
                y: 0,
            },
        })
        .await
        .unwrap();
    reliable
        .send(ReliableInputFrame {
            protocol_version: rshare_core::INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(1),
            sequence: 1,
            captured_at: stamp(2),
            event: ReliableInputEvent::Key {
                keycode: 0x41,
                state: KeyState::Pressed,
            },
        })
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while injection
        .latest_timing()
        .is_none_or(|sample| sample.epoch != SessionEpoch(1) || sample.sequence != 1)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "closed realtime lane starved reliable input"
        );
        tokio::task::yield_now().await;
    }
    shutdown_tx.send(()).unwrap();
    worker.await.unwrap();
}

#[tokio::test]
async fn saturated_injection_actor_fails_closed_and_releases_active_epoch() {
    let gate = Arc::new(BlockingInjectGate::default());
    let injection = InputInjectionHandle::spawn(
        Box::new(BlockingInjectBackend { gate: gate.clone() }),
        InjectionActorConfig {
            reliable_capacity: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let (peer_tx, peer_rx) = mpsc::channel(1);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let worker = tokio::spawn(run_authenticated_input_peers(
        peer_rx,
        injection.clone(),
        Duration::from_secs(1),
        shutdown_rx,
    ));
    let (inbound, _realtime, reliable) = peer(DeviceId::new_v4(), ControlConnectionId::new());
    peer_tx.send(inbound).await.unwrap();

    let frame = |sequence, keycode| ReliableInputFrame {
        protocol_version: rshare_core::INPUT_PROTOCOL_VERSION,
        session_epoch: SessionEpoch(1),
        sequence,
        captured_at: stamp(sequence + 1),
        event: if sequence == 0 {
            ReliableInputEvent::Enter {
                target_display_id: "primary".to_owned(),
                x: 0,
                y: 0,
            }
        } else {
            ReliableInputEvent::Key {
                keycode,
                state: KeyState::Pressed,
            }
        },
    };
    reliable.send(frame(0, 0)).await.unwrap();
    let enter_deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while injection
        .latest_timing()
        .is_none_or(|sample| sample.epoch != SessionEpoch(1) || sample.sequence != 0)
    {
        assert!(
            tokio::time::Instant::now() < enter_deadline,
            "peer lane did not start the injection epoch"
        );
        tokio::task::yield_now().await;
    }
    reliable.send(frame(1, 0x41)).await.unwrap();
    let blocking_gate = gate.clone();
    tokio::task::spawn_blocking(move || blocking_gate.wait_until_blocked())
        .await
        .unwrap();
    reliable.send(frame(2, 0x42)).await.unwrap();
    reliable.send(frame(3, 0x43)).await.unwrap();

    tokio::time::timeout(Duration::from_secs(1), reliable.closed())
        .await
        .expect("queue overflow must terminate the generation lane");
    gate.unblock();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        if gate.events().iter().any(|event| {
            matches!(
                event,
                InputEvent::Key {
                    keycode: KeyCode::Raw(0x41),
                    state: CaptureButtonState::Released,
                }
            )
        }) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "active key was not released after reliable actor saturation"
        );
        tokio::task::yield_now().await;
    }
    let events = gate.events();
    assert!(!events.iter().any(|event| {
        matches!(
            event,
            InputEvent::Key {
                keycode: KeyCode::Raw(keycode),
                state: CaptureButtonState::Pressed,
            } if *keycode == 0x42 || *keycode == 0x43
        )
    }));

    let _ = shutdown_tx.send(());
    worker.await.unwrap();
}

#[tokio::test]
async fn lock_and_suspend_release_even_when_ordinary_command_queue_is_full() {
    for event in [
        SystemSafetyEvent::SessionLocked,
        SystemSafetyEvent::SystemSuspending,
    ] {
        let gate = Arc::new(BlockingInjectGate::default());
        gate.state.lock().unwrap().release = true;
        let injection = InputInjectionHandle::spawn(
            Box::new(BlockingInjectBackend { gate: gate.clone() }),
            InjectionActorConfig::default(),
        )
        .unwrap();
        injection
            .inject_trusted_local(InputEvent::key(
                KeyCode::Raw(0x41),
                CaptureButtonState::Pressed,
            ))
            .await
            .unwrap();
        assert!(matches!(
            gate.events().as_slice(),
            [InputEvent::Key {
                keycode: KeyCode::Raw(0x41),
                state: CaptureButtonState::Pressed,
            }]
        ));

        let (ordinary_tx, _ordinary_rx) = mpsc::channel(1);
        ordinary_tx.try_send(RouterCommand::Shutdown).unwrap();
        assert!(ordinary_tx.try_send(RouterCommand::Shutdown).is_err());
        let (safety_tx, mut safety_rx) = mpsc::unbounded_channel();
        assert!(dispatch_system_safety_event(&injection, &safety_tx, event));
        assert_eq!(safety_rx.recv().await, Some(event));

        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while !gate.events().iter().any(|captured| {
            matches!(
                captured,
                InputEvent::Key {
                    keycode: KeyCode::Raw(0x41),
                    state: CaptureButtonState::Released,
                }
            )
        }) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{event:?} did not release held input"
            );
            tokio::task::yield_now().await;
        }
    }
}

#[tokio::test]
async fn unrelated_peer_disconnect_does_not_close_active_owner() {
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    let (peer_tx, peer_rx) = mpsc::channel(2);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let worker = tokio::spawn(run_authenticated_input_peers(
        peer_rx,
        injection.clone(),
        Duration::from_secs(1),
        shutdown_rx,
    ));
    let (unrelated, unrelated_realtime, unrelated_reliable) =
        peer(DeviceId::new_v4(), ControlConnectionId::new());
    let (active, _active_realtime, active_reliable) =
        peer(DeviceId::new_v4(), ControlConnectionId::new());
    peer_tx.send(unrelated).await.unwrap();
    peer_tx.send(active).await.unwrap();
    active_reliable
        .send(ReliableInputFrame {
            protocol_version: rshare_core::INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(1),
            sequence: 0,
            captured_at: stamp(1),
            event: ReliableInputEvent::Enter {
                target_display_id: "primary".to_owned(),
                x: 0,
                y: 0,
            },
        })
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while injection
        .latest_timing()
        .is_none_or(|sample| sample.sequence != 0)
    {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }

    drop(unrelated_realtime);
    drop(unrelated_reliable);
    tokio::task::yield_now().await;
    active_reliable
        .send(ReliableInputFrame {
            protocol_version: rshare_core::INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(1),
            sequence: 1,
            captured_at: stamp(2),
            event: ReliableInputEvent::Key {
                keycode: 0x41,
                state: KeyState::Pressed,
            },
        })
        .await
        .unwrap();
    while injection
        .latest_timing()
        .is_none_or(|sample| sample.sequence != 1)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "unrelated disconnect closed the active owner"
        );
        tokio::task::yield_now().await;
    }

    shutdown_tx.send(()).unwrap();
    worker.await.unwrap();
}

#[test]
fn terminal_release_high_water_does_not_close_newer_epoch() {
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    let owner = rshare_core::AuthenticatedInputOwner {
        peer_id: DeviceId::new_v4(),
        control_connection_id: ControlConnectionId::new(),
    };
    injection
        .begin_session(owner, SessionEpoch(2), Duration::from_secs(1))
        .unwrap();
    injection.request_release_through(owner, SessionEpoch(1), ReleaseAllReason::BackendFailure);
    assert!(injection
        .try_submit_reliable(
            owner,
            ReliableInputFrame {
                protocol_version: rshare_core::INPUT_PROTOCOL_VERSION,
                session_epoch: SessionEpoch(2),
                sequence: 0,
                captured_at: stamp(1),
                event: ReliableInputEvent::Enter {
                    target_display_id: "primary".to_owned(),
                    x: 0,
                    y: 0,
                },
            },
        )
        .is_ok());
}

#[tokio::test]
async fn trusted_local_injection_is_acknowledged_by_the_actor_thread() {
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    injection
        .inject_trusted_local(InputEvent::key(
            KeyCode::Raw(0x41),
            CaptureButtonState::Pressed,
        ))
        .await
        .unwrap();
    injection
        .inject_trusted_local(InputEvent::key(
            KeyCode::Raw(0x41),
            CaptureButtonState::Released,
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn snapshot_commands_refresh_router_cache_without_per_event_state_lock() {
    let (producer, mut runtime, transport) = runtime(8);
    let before = runtime.route_cache_generation();
    let (_, target, mut replacement) = linked_router();
    replacement.handle(RouterCommand::ConnectivityChanged {
        peer: target,
        connected: true,
    });
    runtime.handle_command(RouterCommand::LayoutChanged(LayoutGraph::new(
        DeviceId::new_v4(),
    )));
    assert!(runtime.route_cache_generation() > before);

    assert_eq!(
        producer.try_push_event(
            origin(CaptureSource::PortableHook),
            InputEvent::key(KeyCode::Raw(0x41), CaptureButtonState::Pressed),
        ),
        PushOutcome::Enqueued
    );
    assert!(runtime.process_next().await);
    assert!(transport.events.lock().unwrap().is_empty());
}

#[test]
fn input_state_channel_preserves_authoritative_truth_when_delta_queue_is_full() {
    let (publisher, mut feeds) = input_state_channel(1);
    publisher.publish_session(ControlSessionState::LocalReady);
    publisher.publish_session(ControlSessionState::Suspended {
        reason: rshare_core::SuspendReason::BackendDegraded,
    });
    assert!(feeds.dirty.take());
    assert!(matches!(
        &*feeds.authoritative_rx.borrow(),
        reliable if matches!(
            reliable.session,
            ControlSessionState::Suspended { .. }
        )
    ));
    assert!(feeds.reliable_rx.try_recv().is_ok());
}
