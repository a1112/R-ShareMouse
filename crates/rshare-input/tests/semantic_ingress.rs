use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use rshare_core::{
    ClockDomainId, GamepadButton, GamepadButtonState, GamepadDeviceInfo, GamepadState,
    MonotonicStamp,
};
use rshare_input::{
    adapt_gamepad_snapshots, ButtonState, CaptureOrigin, CaptureSource, CapturedInput,
    CapturedInputPayload, ContinuousInput, GamepadAxes, IngressClock, IngressEvent, IngressFault,
    InputEvent, InputEventChannel, KeyCode, MouseButton, PointerPosition, PointerSample,
    PushOutcome, SemanticInputIngress,
};

#[derive(Debug)]
struct ManualClock {
    domain: ClockDomainId,
    next_us: AtomicU64,
}

impl ManualClock {
    fn new(domain: u64, first_us: u64) -> Self {
        Self {
            domain: ClockDomainId(domain),
            next_us: AtomicU64::new(first_us),
        }
    }
}

impl IngressClock for ManualClock {
    fn now(&self) -> MonotonicStamp {
        MonotonicStamp::new(self.domain, self.next_us.fetch_add(1, Ordering::Relaxed))
    }
}

fn origin(device_token: u64) -> CaptureOrigin {
    CaptureOrigin {
        source: CaptureSource::PortableHook,
        device_token,
        instance_token: 1,
    }
}

fn captured(payload: CapturedInputPayload) -> CapturedInput {
    CapturedInput {
        captured_at: MonotonicStamp::new(ClockDomainId(7), 10),
        ingress_enqueued_at: MonotonicStamp::new(ClockDomainId(99), 999),
        origin: origin(1),
        pointer: None,
        payload,
    }
}

fn absolute(x: i32, y: i32) -> CapturedInput {
    captured(CapturedInputPayload::Continuous(ContinuousInput::Pointer(
        PointerSample::Absolute { x, y },
    )))
}

fn relative(dx: i32, dy: i32) -> CapturedInput {
    captured(CapturedInputPayload::Continuous(ContinuousInput::Pointer(
        PointerSample::Relative {
            dx,
            dy,
            observed_x: None,
            observed_y: None,
        },
    )))
}

fn key(state: ButtonState) -> CapturedInput {
    captured(CapturedInputPayload::Discrete(InputEvent::key(
        KeyCode::Char(b'A'),
        state,
    )))
}

fn mouse_button(state: ButtonState) -> CapturedInput {
    captured(CapturedInputPayload::Discrete(InputEvent::mouse_button(
        MouseButton::Left,
        state,
    )))
}

fn axes(gamepad_id: u8, x: i16) -> CapturedInput {
    captured(CapturedInputPayload::Continuous(
        ContinuousInput::GamepadAxes(GamepadAxes {
            gamepad_id,
            sequence: 1,
            buttons: Vec::new(),
            left_stick_x: x,
            left_stick_y: 0,
            right_stick_x: 0,
            right_stick_y: 0,
            left_trigger: 0,
            right_trigger: 0,
            timestamp_ms: 1,
        }),
    ))
}

fn assert_absolute(item: &CapturedInput, expected_x: i32, expected_y: i32) {
    assert!(matches!(
        item.payload,
        CapturedInputPayload::Continuous(ContinuousInput::Pointer(
            PointerSample::Absolute { x, y }
        )) if x == expected_x && y == expected_y
    ));
}

#[tokio::test]
async fn one_hundred_thousand_moves_use_one_pending_slot() {
    let (producer, mut consumer) = SemanticInputIngress::new(128);
    for x in 0..100_000 {
        let _ = producer.try_push(absolute(x, x));
    }

    let stats = producer.stats();
    assert_eq!(stats.pending_items, 1);
    assert_eq!(stats.coalesced_motion, 99_999);
    assert_absolute(&consumer.recv().await.unwrap(), 99_999, 99_999);
}

#[tokio::test]
async fn coalescing_never_crosses_a_button_barrier() {
    let (producer, mut consumer) = SemanticInputIngress::new(8);
    assert_eq!(producer.try_push(absolute(1, 1)), PushOutcome::Enqueued);
    assert_eq!(
        producer.try_push(mouse_button(ButtonState::Pressed)),
        PushOutcome::Enqueued
    );
    assert_eq!(producer.try_push(absolute(2, 2)), PushOutcome::Enqueued);

    assert_absolute(&consumer.recv().await.unwrap(), 1, 1);
    assert!(matches!(
        consumer.recv().await.unwrap().payload,
        CapturedInputPayload::Discrete(InputEvent::MouseButton { .. })
    ));
    assert_absolute(&consumer.recv().await.unwrap(), 2, 2);
}

#[test]
fn discrete_overflow_is_explicit() {
    let (producer, _consumer) = SemanticInputIngress::new(1);
    assert_eq!(
        producer.try_push(key(ButtonState::Pressed)),
        PushOutcome::Enqueued
    );
    assert_eq!(
        producer.try_push(key(ButtonState::Released)),
        PushOutcome::ReliableOverflow
    );
    assert_eq!(
        producer.try_pop_fault(),
        Some(IngressFault::ReliableOverflow)
    );
}

#[test]
fn replaceable_motion_cannot_turn_an_all_discrete_queue_into_a_reliable_fault() {
    let (producer, _consumer) = SemanticInputIngress::new(2);
    assert_eq!(
        producer.try_push(key(ButtonState::Pressed)),
        PushOutcome::Enqueued
    );
    assert_eq!(
        producer.try_push(mouse_button(ButtonState::Pressed)),
        PushOutcome::Enqueued
    );
    assert_eq!(
        producer.try_push(absolute(3, 4)),
        PushOutcome::RealtimeDropped
    );
    assert_eq!(producer.try_pop_fault(), None);
}

#[test]
fn discrete_input_evicts_the_oldest_replaceable_item() {
    let (producer, mut consumer) = SemanticInputIngress::new(3);
    assert_eq!(producer.try_push(absolute(1, 1)), PushOutcome::Enqueued);
    assert_eq!(
        producer.try_push(key(ButtonState::Pressed)),
        PushOutcome::Enqueued
    );
    assert_eq!(producer.try_push(axes(1, 10)), PushOutcome::Enqueued);

    assert_eq!(
        producer.try_push(mouse_button(ButtonState::Pressed)),
        PushOutcome::RealtimeReplaced
    );
    assert!(matches!(
        consumer.try_recv().unwrap().payload,
        CapturedInputPayload::Discrete(InputEvent::Key { .. })
    ));
    assert!(matches!(
        consumer.try_recv().unwrap().payload,
        CapturedInputPayload::Continuous(ContinuousInput::GamepadAxes(_))
    ));
    assert!(matches!(
        consumer.try_recv().unwrap().payload,
        CapturedInputPayload::Discrete(InputEvent::MouseButton { .. })
    ));
}

#[tokio::test]
async fn relative_motion_coalescing_accumulates_delta() {
    let (producer, mut consumer) = SemanticInputIngress::new(4);
    assert_eq!(producer.try_push(relative(4, -2)), PushOutcome::Enqueued);
    assert_eq!(producer.try_push(relative(3, 5)), PushOutcome::Coalesced);

    assert!(matches!(
        consumer.recv().await.unwrap().payload,
        CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Relative {
            dx: 7,
            dy: 3,
            ..
        }))
    ));
}

#[tokio::test]
async fn relative_motion_overflow_is_dropped_explicitly_without_wrapping() {
    let (producer, mut consumer) = SemanticInputIngress::new(4);
    assert_eq!(
        producer.try_push(relative(i32::MAX, 0)),
        PushOutcome::Enqueued
    );
    assert_eq!(
        producer.try_push(relative(1, 0)),
        PushOutcome::RealtimeDropped
    );
    assert!(matches!(
        consumer.recv().await.unwrap().payload,
        CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Relative {
            dx: i32::MAX,
            dy: 0,
            ..
        }))
    ));
    assert_eq!(producer.stats().dropped_realtime, 1);
}

#[tokio::test]
async fn mouse_button_snapshots_latest_pointer_coordinate() {
    let (producer, mut consumer) = SemanticInputIngress::new(4);
    let _ = producer.try_push(absolute(44, 55));
    let _ = producer.try_push(mouse_button(ButtonState::Pressed));
    let _ = consumer.recv().await.unwrap();
    let button = consumer.recv().await.unwrap();
    assert_eq!(button.pointer, Some(PointerPosition { x: 44, y: 55 }));
}

#[test]
fn reliable_overflow_uses_a_reserved_fault_slot() {
    let (producer, mut consumer) = SemanticInputIngress::new(1);
    let _ = producer.try_push(key(ButtonState::Pressed));
    let _ = producer.try_push(key(ButtonState::Released));

    assert_eq!(producer.stats().pending_items, 1);
    assert_eq!(
        consumer.try_pop_fault(),
        Some(IngressFault::ReliableOverflow)
    );
    assert!(matches!(
        consumer.try_recv().unwrap().payload,
        CapturedInputPayload::Discrete(InputEvent::Key {
            state: ButtonState::Pressed,
            ..
        })
    ));
}

#[tokio::test]
async fn recv_event_observes_reserved_fault_before_ordinary_queue_items() {
    let (producer, mut consumer) = SemanticInputIngress::new(1);
    let _ = producer.try_push(key(ButtonState::Pressed));
    let _ = producer.try_push(key(ButtonState::Released));

    assert!(matches!(
        consumer.recv_event().await,
        Some(IngressEvent::Fault(IngressFault::ReliableOverflow))
    ));
    assert!(matches!(
        consumer.recv_event().await,
        Some(IngressEvent::Input(CapturedInput {
            payload: CapturedInputPayload::Discrete(InputEvent::Key {
                state: ButtonState::Pressed,
                ..
            }),
            ..
        }))
    ));
}

#[test]
fn absolute_and_relative_pointer_classes_do_not_merge() {
    let (producer, mut consumer) = SemanticInputIngress::new(4);
    let _ = producer.try_push(absolute(7, 8));
    let _ = producer.try_push(relative(2, 3));

    assert_eq!(producer.stats().pending_items, 2);
    assert_absolute(&consumer.try_recv().unwrap(), 7, 8);
    assert!(matches!(
        consumer.try_recv().unwrap().payload,
        CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Relative {
            dx: 2,
            dy: 3,
            ..
        }))
    ));
}

#[test]
fn different_gamepad_ids_do_not_merge() {
    let (producer, _consumer) = SemanticInputIngress::new(4);
    let _ = producer.try_push(axes(1, 10));
    let _ = producer.try_push(axes(2, 20));
    assert_eq!(producer.stats().pending_items, 2);
    assert_eq!(producer.stats().coalesced_motion, 0);
}

#[test]
fn fault_latch_is_idempotent_while_overflow_counter_is_truthful() {
    let (producer, _consumer) = SemanticInputIngress::new(1);
    let _ = producer.try_push(key(ButtonState::Pressed));
    for _ in 0..3 {
        assert_eq!(
            producer.try_push(key(ButtonState::Released)),
            PushOutcome::ReliableOverflow
        );
    }

    let stats = producer.stats();
    assert_eq!(stats.reliable_overflow, 3);
    assert!(stats.reliable_overflow_latched);
    assert_eq!(
        producer.try_pop_fault(),
        Some(IngressFault::ReliableOverflow)
    );
    assert_eq!(producer.try_pop_fault(), None);
    assert!(!producer.stats().reliable_overflow_latched);
}

#[test]
fn dropping_consumer_closes_all_producers() {
    let (producer, consumer) = SemanticInputIngress::new(1);
    let clone = producer.clone();
    drop(consumer);

    assert_eq!(
        producer.try_push(key(ButtonState::Pressed)),
        PushOutcome::Closed
    );
    assert_eq!(
        clone.try_push(key(ButtonState::Released)),
        PushOutcome::Closed
    );
}

#[tokio::test]
async fn notify_has_no_lost_wakeup_and_last_producer_drop_closes_after_drain() {
    let (producer, mut consumer) = SemanticInputIngress::new(2);
    let clone = producer.clone();
    let waiter = tokio::spawn(async move { consumer.recv().await });
    tokio::task::yield_now().await;

    assert_eq!(
        clone.try_push(key(ButtonState::Pressed)),
        PushOutcome::Enqueued
    );
    drop(clone);
    drop(producer);

    let item = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("notified consumer stalled")
        .unwrap()
        .expect("queued item was lost");
    assert!(matches!(
        item.payload,
        CapturedInputPayload::Discrete(InputEvent::Key { .. })
    ));
}

#[tokio::test]
async fn explicit_close_rejects_pushes_and_wakes_consumer() {
    let (producer, mut consumer) = SemanticInputIngress::new(1);
    producer.close();
    assert_eq!(
        producer.try_push(key(ButtonState::Pressed)),
        PushOutcome::Closed
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(1), consumer.recv())
            .await
            .expect("closed consumer stalled")
            .is_none()
    );
}

#[test]
fn capacity_one_keeps_exactly_one_item_and_zero_capacity_is_rejected() {
    let (producer, _consumer) = SemanticInputIngress::new(1);
    assert_eq!(producer.try_push(absolute(1, 2)), PushOutcome::Enqueued);
    assert_eq!(producer.stats().capacity, 1);
    assert_eq!(producer.stats().pending_items, 1);

    let zero = std::panic::catch_unwind(|| SemanticInputIngress::new(0));
    assert!(zero.is_err());
}

#[tokio::test]
async fn injected_clock_writes_ingress_stamp_without_mixing_capture_domain() {
    let clock = Arc::new(ManualClock::new(42, 500));
    let (producer, mut consumer) = SemanticInputIngress::with_clock(2, clock);
    let item = key(ButtonState::Pressed);
    assert_eq!(item.captured_at.domain, ClockDomainId(7));
    assert_eq!(producer.try_push(item), PushOutcome::Enqueued);

    let item = consumer.recv().await.unwrap();
    assert_eq!(item.captured_at, MonotonicStamp::new(ClockDomainId(7), 10));
    assert_eq!(
        item.ingress_enqueued_at,
        MonotonicStamp::new(ClockDomainId(42), 500)
    );
    assert!(item
        .ingress_enqueued_at
        .checked_duration_since(item.captured_at)
        .is_err());
}

#[test]
fn statistics_are_a_single_consistent_snapshot() {
    let (producer, mut consumer) = SemanticInputIngress::new(2);
    let _ = producer.try_push(absolute(1, 1));
    let _ = producer.try_push(absolute(2, 2));
    let _ = producer.try_push(key(ButtonState::Pressed));
    let _ = producer.try_push(key(ButtonState::Released));
    let _ = producer.try_push(key(ButtonState::Pressed));
    let _ = consumer.try_recv();

    let stats = producer.stats();
    assert_eq!(stats.capacity, 2);
    assert_eq!(stats.pending_items, 1);
    assert_eq!(stats.enqueued, 2);
    assert_eq!(stats.dequeued, 1);
    assert_eq!(stats.coalesced_motion, 1);
    assert_eq!(stats.replaced_realtime, 1);
    assert_eq!(stats.reliable_overflow, 1);
}

#[test]
fn compatibility_channel_exposes_nonblocking_outcomes_and_statistics() {
    let (channel, _consumer) = InputEventChannel::with_capacity(1);
    assert_eq!(
        channel.try_send(InputEvent::key(KeyCode::Char(b'Q'), ButtonState::Pressed)),
        PushOutcome::Enqueued
    );
    assert_eq!(
        channel.try_send(InputEvent::key(KeyCode::Char(b'Q'), ButtonState::Released)),
        PushOutcome::ReliableOverflow
    );
    assert_eq!(channel.stats().reliable_overflow, 1);
}

#[test]
fn capture_discontinuity_uses_the_reserved_fault_lane() {
    let (producer, consumer) = SemanticInputIngress::new(2);

    producer.report_fault(IngressFault::CaptureDiscontinuity);

    assert_eq!(
        consumer.try_pop_fault(),
        Some(IngressFault::CaptureDiscontinuity)
    );
    assert_eq!(consumer.try_pop_fault(), None);
}

#[test]
fn every_discrete_payload_has_an_explicit_legacy_conversion() {
    let events = vec![
        InputEvent::mouse_button(MouseButton::Left, ButtonState::Pressed),
        InputEvent::mouse_wheel(1, -2),
        InputEvent::key(KeyCode::Enter, ButtonState::Released),
        InputEvent::key_extended(
            KeyCode::Char(b'X'),
            ButtonState::Pressed,
            true,
            false,
            false,
            false,
        ),
        InputEvent::text_commit("hello".to_string()),
        InputEvent::gamepad_connected(GamepadDeviceInfo {
            gamepad_id: 2,
            name: "pad".to_string(),
            vendor_id: Some(1),
            product_id: Some(2),
        }),
        InputEvent::gamepad_disconnected(2),
        InputEvent::gamepad_button(
            2,
            GamepadButton::South,
            true,
            GamepadState::neutral(2, 1, 3),
        ),
        InputEvent::gamepad_state(GamepadState::neutral(2, 1, 3)),
    ];

    for event in events {
        let expected_kind = event.event_type();
        let converted = captured(CapturedInputPayload::Discrete(event))
            .into_input_event()
            .expect("discrete conversion must be total");
        assert_eq!(converted.event_type(), expected_kind);
    }
}

#[test]
fn gamepad_button_conversion_preserves_the_reliable_transition() {
    let converted = captured(CapturedInputPayload::Discrete(InputEvent::gamepad_button(
        7,
        GamepadButton::DPadLeft,
        false,
        GamepadState::neutral(7, 8, 9),
    )))
    .into_input_event()
    .unwrap();
    assert!(matches!(
        converted,
        InputEvent::GamepadButton {
            gamepad_id: 7,
            button: GamepadButton::DPadLeft,
            pressed: false,
            state_after: GamepadState {
                gamepad_id: 7,
                sequence: 8,
                timestamp_ms: 9,
                ..
            },
        }
    ));
}

#[test]
fn relative_input_without_an_observed_position_is_an_explicit_conversion_error() {
    assert!(relative(1, 2).into_input_event().is_err());
}

#[test]
fn gamepad_adapter_emits_ordered_button_diffs_before_replaceable_axes() {
    let previous = gamepad_state(3, &[(GamepadButton::East, true)], 0);
    let current = gamepad_state(
        3,
        &[(GamepadButton::East, false), (GamepadButton::South, true)],
        12_000,
    );

    let outputs = adapt_gamepad_snapshots(&previous, &current);
    assert_eq!(outputs.len(), 3);
    assert!(matches!(
        outputs[0],
        CapturedInputPayload::Discrete(InputEvent::GamepadButton {
            gamepad_id: 3,
            button: GamepadButton::South,
            pressed: true,
            ..
        })
    ));
    assert!(matches!(
        outputs[1],
        CapturedInputPayload::Discrete(InputEvent::GamepadButton {
            gamepad_id: 3,
            button: GamepadButton::East,
            pressed: false,
            ..
        })
    ));
    assert!(matches!(
        outputs[2],
        CapturedInputPayload::Continuous(ContinuousInput::GamepadAxes(GamepadAxes {
            gamepad_id: 3,
            left_stick_x: 12_000,
            ..
        }))
    ));
}

#[test]
fn gamepad_adapter_orders_vendor_buttons_deterministically() {
    let previous = gamepad_state(4, &[(GamepadButton::Other(9), true)], 0);
    let current = gamepad_state(
        4,
        &[
            (GamepadButton::Other(7), true),
            (GamepadButton::Other(9), false),
        ],
        0,
    );

    let outputs = adapt_gamepad_snapshots(&previous, &current);
    assert!(matches!(
        outputs.as_slice(),
        [
            CapturedInputPayload::Discrete(InputEvent::GamepadButton {
                button: GamepadButton::Other(7),
                pressed: true,
                ..
            }),
            CapturedInputPayload::Discrete(InputEvent::GamepadButton {
                button: GamepadButton::Other(9),
                pressed: false,
                ..
            })
        ]
    ));
}

#[tokio::test]
async fn gamepad_button_barrier_is_not_erased_by_axis_coalescing() {
    let (producer, mut consumer) = SemanticInputIngress::new(8);
    let previous = gamepad_state(1, &[], 0);
    let current = gamepad_state(1, &[(GamepadButton::South, true)], 10);
    let next = gamepad_state(1, &[(GamepadButton::South, true)], 20);

    for payload in adapt_gamepad_snapshots(&previous, &current)
        .into_iter()
        .chain(adapt_gamepad_snapshots(&current, &next))
    {
        let _ = producer.try_push(captured(payload));
    }

    assert!(matches!(
        consumer.recv().await.unwrap().payload,
        CapturedInputPayload::Discrete(InputEvent::GamepadButton { .. })
    ));
    assert!(matches!(
        consumer.recv().await.unwrap().payload,
        CapturedInputPayload::Continuous(ContinuousInput::GamepadAxes(GamepadAxes {
            left_stick_x: 20,
            ..
        }))
    ));
}

#[test]
fn button_and_axis_change_preserves_buttons_in_the_final_legacy_state() {
    let previous = gamepad_state(5, &[], 0);
    let current = gamepad_state(5, &[(GamepadButton::South, true)], 9_000);
    let converted = adapt_gamepad_snapshots(&previous, &current)
        .into_iter()
        .map(|payload| captured(payload).into_input_event().unwrap())
        .collect::<Vec<_>>();

    assert!(matches!(
        converted.last(),
        Some(InputEvent::GamepadState {
            state: GamepadState {
                gamepad_id: 5,
                left_stick_x: 9_000,
                buttons,
                ..
            }
        }) if buttons == &vec![GamepadButtonState {
            button: GamepadButton::South,
            pressed: true,
        }]
    ));
}

fn gamepad_state(
    gamepad_id: u8,
    buttons: &[(GamepadButton, bool)],
    left_stick_x: i16,
) -> GamepadState {
    GamepadState {
        gamepad_id,
        sequence: 5,
        buttons: buttons
            .iter()
            .map(|(button, pressed)| GamepadButtonState {
                button: *button,
                pressed: *pressed,
            })
            .collect(),
        left_stick_x,
        left_stick_y: 0,
        right_stick_x: 0,
        right_stick_y: 0,
        left_trigger: 0,
        right_trigger: 0,
        timestamp_ms: 123,
    }
}
