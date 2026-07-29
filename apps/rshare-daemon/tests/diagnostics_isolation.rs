use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rshare_core::{
    BackendHealth, BackendKind, ControlConnectionId, DeviceId, Direction, InputRouter, LayoutGraph,
    LayoutLink, LayoutNode, LocalInputDeviceKind, LocalInputDiagnosticEvent, LocalInputEventSource,
    PixelRect, RealtimeInputFrame, ReliableInputEvent, ReliableInputFrame, VirtualDesktopGeometry,
};
use rshare_daemon::diagnostics_runtime::{
    DiagnosticHistoryEntry, DiagnosticPayload, DiagnosticPublication, DiagnosticSubscriberId,
    DiagnosticsRuntime, DiagnosticsSubscription, DIAGNOSTICS_HISTORY_CAPACITY,
    DIAGNOSTICS_SAMPLE_PERIOD,
};
use rshare_daemon::input_runtime::{InputRuntime, InputTransport};
use rshare_daemon::input_state::{input_state_channel, ControlMetrics};
use rshare_input::{
    ButtonState, CaptureOrigin, CaptureSource, CapturedInputPayload, ContinuousInput,
    InjectBackend, InjectionActorConfig, InputEvent, InputInjectionHandle, KeyCode, PointerSample,
    PushOutcome, SemanticInputIngress,
};

fn subscriber(peer_id: DeviceId, _generation: u64) -> DiagnosticSubscriberId {
    DiagnosticSubscriberId {
        peer_id,
        control_connection_id: ControlConnectionId::new(),
    }
}

fn activate_and_subscribe(
    runtime: &DiagnosticsRuntime,
    id: DiagnosticSubscriberId,
) -> DiagnosticsSubscription {
    runtime.activate_generation(id);
    runtime
        .subscribe_current(id)
        .expect("test generation must be active")
}

fn discrete_event(sequence: u64) -> LocalInputDiagnosticEvent {
    LocalInputDiagnosticEvent {
        sequence,
        timestamp_ms: sequence,
        device_kind: LocalInputDeviceKind::Backend,
        event_kind: "latency_probe_ack".to_string(),
        summary: format!("Latency sample {sequence}"),
        device_id: None,
        device_instance_id: None,
        capture_path: Some("rshare-net".to_string()),
        source: LocalInputEventSource::System,
        payload: BTreeMap::from([("sequence".to_string(), sequence.to_string())]),
    }
}

fn publication_payloads(publication: &DiagnosticPublication) -> Vec<DiagnosticPayload> {
    publication
        .items
        .iter()
        .map(|item| item.payload.clone())
        .collect()
}

#[tokio::test]
async fn no_subscribers_skip_formatting_and_publication_for_one_hundred_thousand_updates() {
    let metrics = Arc::new(ControlMetrics::default());
    let mut runtime = DiagnosticsRuntime::new(metrics.clone(), DIAGNOSTICS_HISTORY_CAPACITY);

    for _ in 0..100_000 {
        metrics.record_captured();
    }
    assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD));

    let stats = runtime.stats();
    assert_eq!(stats.samples, 1);
    assert_eq!(stats.formatted, 0);
    assert_eq!(stats.publications, 0);
    assert_eq!(runtime.latest().captured, 100_000);
}

#[tokio::test]
async fn latest_metrics_watch_only_wakes_for_semantic_changes() {
    let metrics = Arc::new(ControlMetrics::default());
    let mut runtime = DiagnosticsRuntime::new(metrics.clone(), DIAGNOSTICS_HISTORY_CAPACITY);
    let mut latest = runtime.latest_receiver();

    assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), latest.changed())
            .await
            .is_err(),
        "an unchanged sample must not wake the UI projection"
    );

    metrics.record_captured();
    assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD * 2));
    tokio::time::timeout(Duration::from_millis(100), latest.changed())
        .await
        .expect("changed metrics must wake the UI projection")
        .expect("diagnostics metrics sender must remain open");
    assert_eq!(latest.borrow().captured, 1);
}

#[tokio::test]
async fn one_subscriber_receives_twenty_hertz_samples_not_per_input_updates() {
    let metrics = Arc::new(ControlMetrics::default());
    let mut runtime = DiagnosticsRuntime::new(metrics.clone(), 8);
    let peer = subscriber(DeviceId::new_v4(), 7);
    let mut subscription = activate_and_subscribe(&runtime, peer);

    for _ in 0..100_000 {
        metrics.record_captured();
    }
    assert!(!runtime.sample_at(Duration::from_millis(49)));
    assert!(subscription.try_recv().is_none());
    assert!(runtime.sample_at(Duration::from_millis(50)));
    assert_eq!(
        publication_payloads(&subscription.try_recv().unwrap()),
        vec![DiagnosticPayload::Metrics(metrics.snapshot())]
    );

    for _ in 0..50_000 {
        metrics.record_captured();
    }
    assert!(!runtime.sample_at(Duration::from_millis(99)));
    assert!(subscription.try_recv().is_none());
    assert!(runtime.sample_at(Duration::from_millis(100)));
    assert_eq!(
        publication_payloads(&subscription.try_recv().unwrap()),
        vec![DiagnosticPayload::Metrics(metrics.snapshot())]
    );

    let stats = runtime.stats();
    assert_eq!(stats.samples, 2);
    assert_eq!(stats.formatted, 2);
    assert_eq!(stats.publications, 2);
}

#[tokio::test]
async fn recent_history_is_bounded_and_motion_is_sampled_while_discrete_events_survive() {
    const CAPACITY: usize = 4;
    let metrics = Arc::new(ControlMetrics::default());
    let mut runtime = DiagnosticsRuntime::new(metrics.clone(), CAPACITY);

    for sample in 1..=10_u64 {
        for _ in 0..100_000 {
            metrics.record_captured();
        }
        assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD * sample as u32));
    }
    assert_eq!(runtime.history().len(), CAPACITY);
    assert!(runtime
        .history()
        .iter()
        .all(|entry| matches!(entry, DiagnosticHistoryEntry::Metrics(_))));

    assert!(runtime.record_discrete(discrete_event(77)));
    assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD * 11));
    let history = runtime.history();
    assert_eq!(history.len(), CAPACITY);
    assert!(history.iter().any(|entry| matches!(
        entry,
        DiagnosticHistoryEntry::Discrete(event) if event.sequence == 77
    )));
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(entry, DiagnosticHistoryEntry::Metrics(_)))
            .count(),
        CAPACITY - 1,
        "100,000 continuous updates per interval must become one sampled history item"
    );
}

#[tokio::test]
async fn one_tick_preserves_metrics_and_all_discrete_events_for_a_subscriber() {
    let metrics = Arc::new(ControlMetrics::default());
    let mut runtime = DiagnosticsRuntime::new(metrics.clone(), 8);
    let id = subscriber(DeviceId::new_v4(), 11);
    let mut subscription = activate_and_subscribe(&runtime, id);

    assert!(runtime.record_discrete(discrete_event(101)));
    assert!(runtime.record_discrete(discrete_event(102)));
    assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD));

    let publication = subscription.try_recv().unwrap();
    assert_eq!(
        publication_payloads(&publication),
        vec![
            DiagnosticPayload::Metrics(metrics.snapshot()),
            DiagnosticPayload::Discrete(discrete_event(101)),
            DiagnosticPayload::Discrete(discrete_event(102)),
        ]
    );
    assert!(
        subscription.try_recv().is_none(),
        "one tick must replace one watch slot with one bounded batch"
    );
}

#[tokio::test]
async fn subscribe_and_generation_safe_unsubscribe_gate_delivery_immediately() {
    let metrics = Arc::new(ControlMetrics::default());
    let mut runtime = DiagnosticsRuntime::new(metrics.clone(), 8);
    let peer_id = DeviceId::new_v4();
    let old_id = subscriber(peer_id, 1);
    let new_id = subscriber(peer_id, 2);
    let old = activate_and_subscribe(&runtime, old_id);
    let mut current = activate_and_subscribe(&runtime, new_id);
    assert!(old.is_closed(), "replacement must close the old generation");

    assert!(
        !runtime.unsubscribe(old_id),
        "old generation must not compare-remove the replacement"
    );
    metrics.record_captured();
    assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD));
    assert!(current.try_recv().is_some());

    assert!(runtime.unsubscribe(new_id));
    assert!(current.is_closed());
    metrics.record_captured();
    assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD * 2));
    assert!(current.try_recv().is_none());
}

#[tokio::test]
async fn stale_subscription_admission_cannot_overwrite_an_activated_replacement_generation() {
    let metrics = Arc::new(ControlMetrics::default());
    let mut runtime = DiagnosticsRuntime::new(metrics.clone(), 8);
    let handle = runtime.handle();
    let peer_id = DeviceId::new_v4();
    let old = subscriber(peer_id, 1);
    let replacement = subscriber(peer_id, 2);

    handle.activate_generation(old);
    handle.activate_generation(replacement);
    assert!(
        handle.subscribe_current(old).is_none(),
        "stale ControlReceived admission must be rejected atomically"
    );
    let mut current = handle
        .subscribe_current(replacement)
        .expect("activated replacement must subscribe");
    assert!(
        !handle.clear_generation(old),
        "stale disconnect must not clear replacement authority"
    );

    metrics.record_captured();
    assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD));
    assert!(current.try_recv().is_some());
}

#[tokio::test]
async fn repeated_same_generation_subscription_is_idempotent_and_preserves_existing_stream() {
    let metrics = Arc::new(ControlMetrics::default());
    let mut runtime = DiagnosticsRuntime::new(metrics.clone(), 8);
    let id = subscriber(DeviceId::new_v4(), 9);
    let mut existing = activate_and_subscribe(&runtime, id);
    assert!(
        runtime.subscribe_current(id).is_none(),
        "a repeated request must reuse the existing peer stream"
    );
    assert!(
        !existing.is_closed(),
        "the broad stream must remain active for older local filtered views"
    );
    metrics.record_captured();
    assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD));
    assert!(existing.try_recv().is_some());
}

#[tokio::test]
async fn repeated_subscription_does_not_discard_a_previously_queued_publication() {
    let metrics = Arc::new(ControlMetrics::default());
    let mut runtime = DiagnosticsRuntime::new(metrics.clone(), 8);
    let id = subscriber(DeviceId::new_v4(), 10);
    let mut existing = activate_and_subscribe(&runtime, id);

    metrics.record_captured();
    assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD));
    assert!(runtime.subscribe_current(id).is_none());

    assert!(
        existing.try_recv().is_some(),
        "a repeated request must not discard unread data from the broad peer stream"
    );
}

#[tokio::test]
async fn one_physical_input_produces_at_most_one_sampled_publication() {
    let (producer, consumer) = SemanticInputIngress::new(8);
    let transport = Arc::new(LatestTransport::default());
    let (state, _feeds) = input_state_channel(1);
    let metrics = Arc::new(ControlMetrics::default());
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    let mut input = InputRuntime::new(
        consumer,
        linked_router(),
        transport,
        state,
        metrics.clone(),
        injection,
    );
    let mut diagnostics = DiagnosticsRuntime::new(metrics.clone(), 8);
    let subscription_id = subscriber(DeviceId::new_v4(), 3);
    let mut subscription = activate_and_subscribe(&diagnostics, subscription_id);

    assert_eq!(
        producer.try_push(producer.capture(
            origin(),
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Absolute {
                x: 99,
                y: 50,
            })),
        )),
        PushOutcome::Enqueued
    );
    assert!(input.process_ready());
    assert!(diagnostics.sample_at(DIAGNOSTICS_SAMPLE_PERIOD));
    let _baseline = subscription
        .try_recv()
        .expect("remote-entry baseline sample");
    let baseline_metrics = metrics.snapshot();
    let baseline_publications = diagnostics.stats().publications;

    assert_eq!(
        producer.try_push(producer.capture(
            origin(),
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Relative {
                dx: 7,
                dy: -3,
                observed_x: Some(106),
                observed_y: Some(47),
            })),
        )),
        PushOutcome::Enqueued
    );
    assert!(input.process_ready());
    assert!(diagnostics.sample_at(DIAGNOSTICS_SAMPLE_PERIOD * 2));

    let after_input = metrics.snapshot();
    assert_eq!(
        after_input.captured,
        baseline_metrics.captured + 1,
        "one physical source input must increment captured exactly once"
    );
    assert_eq!(
        after_input.routed,
        baseline_metrics.routed + 1,
        "one physical source input must increment routed exactly once"
    );
    let publication = subscription
        .try_recv()
        .expect("one sampled telemetry message");
    assert_eq!(
        publication_payloads(&publication),
        vec![DiagnosticPayload::Metrics(after_input)]
    );
    assert!(subscription.try_recv().is_none());
    assert_eq!(
        diagnostics.stats().publications,
        baseline_publications + 1,
        "one physical source input must replace one telemetry publication slot"
    );
}

#[tokio::test]
async fn slow_subscriber_is_latest_only_and_cannot_create_a_hidden_backlog() {
    let metrics = Arc::new(ControlMetrics::default());
    let mut runtime = DiagnosticsRuntime::new(metrics.clone(), 8);
    let subscription_id = subscriber(DeviceId::new_v4(), 4);
    let mut subscription = activate_and_subscribe(&runtime, subscription_id);

    for tick in 1..=100_u64 {
        metrics.record_captured();
        assert!(runtime.sample_at(DIAGNOSTICS_SAMPLE_PERIOD * tick as u32));
    }

    let publication = subscription
        .try_recv()
        .expect("latest sample remains available");
    assert_eq!(
        publication_payloads(&publication),
        vec![DiagnosticPayload::Metrics(metrics.snapshot())]
    );
    assert!(
        subscription.try_recv().is_none(),
        "slow subscriber must not accumulate a per-tick backlog"
    );
}

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

#[derive(Default)]
struct LatestTransport {
    latest_realtime: Mutex<Option<RealtimeInputFrame>>,
    reliable: Mutex<Vec<ReliableInputFrame>>,
}

impl InputTransport for LatestTransport {
    type Binding = DeviceId;

    fn bind(&self, target: DeviceId) -> Option<Self::Binding> {
        Some(target)
    }

    fn try_send_realtime(&self, _binding: &Self::Binding, frame: RealtimeInputFrame) {
        *self.latest_realtime.lock().unwrap() = Some(frame);
    }

    fn try_send_reliable(&self, _binding: &Self::Binding, frame: ReliableInputFrame) -> bool {
        self.reliable.lock().unwrap().push(frame);
        true
    }
}

fn linked_router() -> InputRouter {
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
    InputRouter::new(
        local,
        layout,
        VirtualDesktopGeometry::new(PixelRect::new(0, 0, 100, 100)),
        [remote],
    )
}

fn origin() -> CaptureOrigin {
    CaptureOrigin {
        source: CaptureSource::WindowsFilter,
        device_token: 7,
        instance_token: 9,
    }
}

#[tokio::test(start_paused = true)]
async fn five_second_scheduler_stall_skips_missed_diagnostics_ticks() {
    let metrics = Arc::new(ControlMetrics::default());
    let diagnostics = DiagnosticsRuntime::new(metrics, 8);
    let diagnostics_handle = diagnostics.handle();
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel(1);
    let diagnostics_worker = tokio::spawn(diagnostics.run(shutdown_rx));

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;

    assert_eq!(
        diagnostics_handle.stats().samples,
        1,
        "a stalled low-priority actor must skip missed ticks instead of bursting"
    );

    shutdown_tx.send(()).await.unwrap();
    diagnostics_worker.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn blocked_diagnostics_sink_for_five_seconds_never_backpressures_input() {
    let (producer, consumer) = SemanticInputIngress::new(8);
    let transport = Arc::new(LatestTransport::default());
    let (state, feeds) = input_state_channel(1);
    let metrics = Arc::new(ControlMetrics::default());
    let injection =
        InputInjectionHandle::spawn(Box::new(NoopInjectBackend), InjectionActorConfig::default())
            .unwrap();
    let mut input = InputRuntime::new(
        consumer,
        linked_router(),
        transport.clone(),
        state,
        metrics.clone(),
        injection,
    );
    let diagnostics = DiagnosticsRuntime::new(metrics, 8);
    let diagnostics_handle = diagnostics.handle();
    let blocked_id = subscriber(DeviceId::new_v4(), 5);
    diagnostics_handle.activate_generation(blocked_id);
    let mut blocked_sink = diagnostics_handle
        .subscribe_current(blocked_id)
        .expect("blocked sink generation must be current");
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel(1);
    let diagnostics_worker = tokio::spawn(diagnostics.run(shutdown_rx));

    assert_eq!(
        producer.try_push(producer.capture(
            origin(),
            CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Absolute {
                x: 99,
                y: 50,
            })),
        )),
        PushOutcome::Enqueued
    );
    assert!(input.process_ready());

    let fake_start = tokio::time::Instant::now();
    for tick in 0_u64..100 {
        for offset in 0_u64..1_000 {
            let x = i32::try_from(tick * 1_000 + offset).unwrap();
            assert_eq!(
                producer.try_push(producer.capture(
                    origin(),
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
            assert!(
                producer.stats().pending_items <= producer.stats().capacity,
                "input ingress depth must stay bounded"
            );
            assert!(input.process_ready());
        }
        if tick == 99 {
            assert_eq!(
                producer.try_push_event(
                    origin(),
                    InputEvent::key(KeyCode::Raw(0x41), ButtonState::Pressed),
                ),
                PushOutcome::Enqueued
            );
            while input.process_ready() {}
        }
        tokio::time::advance(DIAGNOSTICS_SAMPLE_PERIOD).await;
        while diagnostics_handle.stats().samples < tick + 1 {
            tokio::task::yield_now().await;
        }
    }

    assert_eq!(fake_start.elapsed(), Duration::from_secs(5));
    assert_eq!(diagnostics_handle.stats().samples, 100);
    assert_eq!(producer.stats().pending_items, 0);
    assert!(matches!(
        transport
            .latest_realtime
            .lock()
            .unwrap()
            .as_ref()
            .map(|frame| &frame.payload),
        Some(rshare_core::RealtimeInputPayload::RelativeMouse { dx: 99_999, dy: 0 })
    ));
    assert!(transport
        .reliable
        .lock()
        .unwrap()
        .iter()
        .any(|frame| matches!(frame.event, ReliableInputEvent::Key { keycode: 0x41, .. })));
    assert_eq!(
        feeds.pointer_rx.borrow().as_ref().map(|pointer| pointer.x),
        Some(99_999)
    );
    let latest = blocked_sink
        .try_recv()
        .expect("latest sampled publication remains available");
    assert!(matches!(
        latest.items.first().map(|item| &item.payload),
        Some(DiagnosticPayload::Metrics(_))
    ));
    assert!(
        blocked_sink.try_recv().is_none(),
        "five seconds of blocked delivery must retain no hidden backlog"
    );

    shutdown_tx.send(()).await.unwrap();
    diagnostics_worker.await.unwrap();
}
