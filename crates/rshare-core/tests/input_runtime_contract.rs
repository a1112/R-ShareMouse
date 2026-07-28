use rshare_core::{
    AcceptRealtime, AcceptReliable, AuthenticatedInputOwner, ButtonState, ClockDomainId,
    ControlConnectionId, DeviceId, InputOwnershipGate, KeyState, MonotonicStamp, MouseButton,
    PressedStateLedger, RealtimeInputFrame, RealtimeInputPayload, ReleaseAllReason,
    ReliableInputEvent, ReliableInputFrame, SessionEpoch, SessionEpochError, TransferError,
    INPUT_PROTOCOL_VERSION,
};

fn authenticated_owner(
    peer_id: DeviceId,
    control_connection_id: ControlConnectionId,
) -> AuthenticatedInputOwner {
    AuthenticatedInputOwner {
        peer_id,
        control_connection_id,
    }
}

#[test]
fn session_epochs_advance_strictly_and_fail_closed_at_exhaustion() {
    let mut epoch = SessionEpoch(7);

    assert_eq!(epoch.next(), Ok(SessionEpoch(8)));
    assert_eq!(epoch.advance(), Ok(SessionEpoch(8)));
    assert_eq!(epoch, SessionEpoch(8));
    assert_eq!(
        SessionEpoch(u64::MAX).next(),
        Err(SessionEpochError::Exhausted)
    );

    let mut exhausted = SessionEpoch(u64::MAX);
    assert_eq!(exhausted.advance(), Err(SessionEpochError::Exhausted));
    assert_eq!(exhausted, SessionEpoch(u64::MAX));
}

#[test]
fn old_owner_and_late_realtime_sequence_are_rejected() {
    let owner_a = authenticated_owner(DeviceId::new_v4(), ControlConnectionId::new());
    let owner_b = authenticated_owner(DeviceId::new_v4(), ControlConnectionId::new());
    let mut gate = InputOwnershipGate::new(owner_a, SessionEpoch(7));

    assert_eq!(
        gate.accept_realtime(owner_a, SessionEpoch(7), 10),
        AcceptRealtime::Accepted
    );
    assert_eq!(
        gate.accept_realtime(owner_a, SessionEpoch(7), 12),
        AcceptRealtime::AcceptedWithGap(1)
    );
    assert_eq!(
        gate.accept_realtime(owner_a, SessionEpoch(7), 11),
        AcceptRealtime::OutOfOrder
    );
    assert_eq!(gate.transfer(owner_b, SessionEpoch(8)), Ok(()));
    assert_eq!(
        gate.accept_realtime(owner_a, SessionEpoch(7), 13),
        AcceptRealtime::WrongOwnerOrEpoch
    );
}

#[test]
fn ownership_binds_peer_and_connection_generation_and_transfer_requires_newer_epoch() {
    let peer = DeviceId::new_v4();
    let owner = authenticated_owner(peer, ControlConnectionId::new());
    let reconnected_owner = authenticated_owner(peer, ControlConnectionId::new());
    let other_peer = authenticated_owner(DeviceId::new_v4(), ControlConnectionId::new());
    let mut gate = InputOwnershipGate::new(owner, SessionEpoch(20));

    assert_eq!(
        gate.accept_realtime(reconnected_owner, SessionEpoch(20), 1),
        AcceptRealtime::WrongOwnerOrEpoch
    );
    assert_eq!(
        gate.accept_reliable(other_peer, SessionEpoch(20), 1),
        AcceptReliable::WrongOwnerOrEpoch
    );
    assert_eq!(
        gate.transfer(reconnected_owner, SessionEpoch(20)),
        Err(TransferError::EpochNotIncreasing {
            current: SessionEpoch(20),
            proposed: SessionEpoch(20),
        })
    );
    assert_eq!(
        gate.transfer(reconnected_owner, SessionEpoch(19)),
        Err(TransferError::EpochNotIncreasing {
            current: SessionEpoch(20),
            proposed: SessionEpoch(19),
        })
    );

    assert_eq!(gate.transfer(reconnected_owner, SessionEpoch(21)), Ok(()));
    assert_eq!(
        gate.accept_realtime(reconnected_owner, SessionEpoch(21), 0),
        AcceptRealtime::Accepted
    );
    assert_eq!(
        gate.accept_reliable(reconnected_owner, SessionEpoch(21), 0),
        AcceptReliable::Accepted
    );
}

#[test]
fn reliable_and_realtime_sequences_are_independent() {
    let owner = authenticated_owner(DeviceId::new_v4(), ControlConnectionId::new());
    let mut gate = InputOwnershipGate::new(owner, SessionEpoch(3));

    assert_eq!(
        gate.accept_realtime(owner, SessionEpoch(3), 10),
        AcceptRealtime::Accepted
    );
    assert_eq!(
        gate.accept_reliable(owner, SessionEpoch(3), 10),
        AcceptReliable::Accepted
    );
    assert_eq!(
        gate.accept_realtime(owner, SessionEpoch(3), 10),
        AcceptRealtime::OutOfOrder
    );
    assert_eq!(
        gate.accept_reliable(owner, SessionEpoch(3), 12),
        AcceptReliable::AcceptedWithGap(1)
    );
    assert_eq!(
        gate.accept_reliable(owner, SessionEpoch(3), 11),
        AcceptReliable::OutOfOrder
    );
    assert_eq!(
        gate.accept_reliable(owner, SessionEpoch(4), 13),
        AcceptReliable::WrongOwnerOrEpoch
    );
}

#[test]
fn release_all_is_deterministic_idempotent_and_two_phase() {
    let mut ledger = PressedStateLedger::new();
    assert_eq!(ledger.record_key(90, KeyState::Pressed), Ok(true));
    assert_eq!(ledger.record_key(10, KeyState::Pressed), Ok(true));
    assert_eq!(ledger.record_key(10, KeyState::Pressed), Ok(false));
    assert_eq!(
        ledger.record_mouse_button(MouseButton::Right, ButtonState::Pressed, 800, 450, 19,),
        Ok(true)
    );
    assert_eq!(
        ledger.record_mouse_button(MouseButton::Left, ButtonState::Pressed, 25, 30, 4),
        Ok(true)
    );
    assert_eq!(
        ledger.record_mouse_button(MouseButton::Left, ButtonState::Pressed, 25, 30, 4),
        Ok(false)
    );

    let pending = ledger
        .release_all_events(ReleaseAllReason::SessionEnded)
        .unwrap();
    assert_eq!(pending.reason(), ReleaseAllReason::SessionEnded);
    assert_eq!(
        pending.events(),
        &[
            ReliableInputEvent::Key {
                keycode: 10,
                state: KeyState::Released,
            },
            ReliableInputEvent::Key {
                keycode: 90,
                state: KeyState::Released,
            },
            ReliableInputEvent::MouseButton {
                button: MouseButton::Left,
                state: ButtonState::Released,
                x: 25,
                y: 30,
                realtime_anchor_sequence: 4,
            },
            ReliableInputEvent::MouseButton {
                button: MouseButton::Right,
                state: ButtonState::Released,
                x: 800,
                y: 450,
                realtime_anchor_sequence: 19,
            },
        ]
    );

    // Preparing, or abandoning, a batch cannot clear held state.
    assert_eq!(ledger.held_key_count(), 2);
    assert_eq!(ledger.held_mouse_button_count(), 2);
    let retry = ledger
        .release_all_events(ReleaseAllReason::SessionEnded)
        .unwrap();
    assert_eq!(retry.events(), pending.events());

    assert_eq!(ledger.confirm_release_all(&pending), 4);
    assert!(ledger.is_empty());
    assert_eq!(ledger.confirm_release_all(&pending), 0);
}

#[test]
fn pending_release_confirmation_does_not_clear_new_or_repressed_input() {
    let mut ledger = PressedStateLedger::new();
    ledger.record_key(1, KeyState::Pressed).unwrap();
    ledger
        .record_mouse_button(MouseButton::Left, ButtonState::Pressed, 10, 20, 5)
        .unwrap();
    let pending = ledger
        .release_all_events(ReleaseAllReason::OwnershipTransfer)
        .unwrap();

    // Input remains accepted while a release batch is pending. A release/re-press
    // creates a new generation that an older confirmation must not clear.
    ledger.record_key(1, KeyState::Released).unwrap();
    ledger.record_key(1, KeyState::Pressed).unwrap();
    ledger.record_key(2, KeyState::Pressed).unwrap();
    ledger
        .record_mouse_button(MouseButton::Left, ButtonState::Released, 10, 20, 5)
        .unwrap();
    ledger
        .record_mouse_button(MouseButton::Left, ButtonState::Pressed, 40, 50, 8)
        .unwrap();
    ledger
        .record_mouse_button(MouseButton::Right, ButtonState::Pressed, 60, 70, 9)
        .unwrap();

    assert_eq!(ledger.confirm_release_all(&pending), 0);
    assert_eq!(ledger.held_key_count(), 2);
    assert_eq!(ledger.held_mouse_button_count(), 2);

    let current = ledger
        .release_all_events(ReleaseAllReason::OwnershipTransfer)
        .unwrap();
    assert_eq!(
        current.events(),
        &[
            ReliableInputEvent::Key {
                keycode: 1,
                state: KeyState::Released,
            },
            ReliableInputEvent::Key {
                keycode: 2,
                state: KeyState::Released,
            },
            ReliableInputEvent::MouseButton {
                button: MouseButton::Left,
                state: ButtonState::Released,
                x: 40,
                y: 50,
                realtime_anchor_sequence: 8,
            },
            ReliableInputEvent::MouseButton {
                button: MouseButton::Right,
                state: ButtonState::Released,
                x: 60,
                y: 70,
                realtime_anchor_sequence: 9,
            },
        ]
    );
}

#[test]
fn control_frames_round_trip_without_a_wire_owner() {
    let realtime = RealtimeInputFrame {
        protocol_version: INPUT_PROTOCOL_VERSION,
        session_epoch: SessionEpoch(9),
        sequence: 22,
        captured_at: MonotonicStamp::new(ClockDomainId(4), 12_345),
        payload: RealtimeInputPayload::RelativeMouse { dx: -3, dy: 7 },
    };
    let reliable = ReliableInputFrame {
        protocol_version: INPUT_PROTOCOL_VERSION,
        session_epoch: SessionEpoch(9),
        sequence: 14,
        captured_at: MonotonicStamp::new(ClockDomainId(4), 12_346),
        event: ReliableInputEvent::MouseButton {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
            x: 800,
            y: 450,
            realtime_anchor_sequence: 22,
        },
    };

    let realtime_json = serde_json::to_value(&realtime).unwrap();
    let reliable_json = serde_json::to_value(&reliable).unwrap();
    assert_eq!(
        realtime_json["protocol_version"],
        serde_json::json!(INPUT_PROTOCOL_VERSION)
    );
    assert_eq!(
        reliable_json["protocol_version"],
        serde_json::json!(INPUT_PROTOCOL_VERSION)
    );
    assert!(realtime_json.get("owner").is_none());
    assert!(realtime_json.get("peer_id").is_none());
    assert!(realtime_json.get("control_connection_id").is_none());
    assert!(reliable_json.get("owner").is_none());
    assert!(reliable_json.get("peer_id").is_none());
    assert!(reliable_json.get("control_connection_id").is_none());

    assert_eq!(
        serde_json::from_value::<RealtimeInputFrame>(realtime_json).unwrap(),
        realtime
    );
    assert_eq!(
        serde_json::from_value::<ReliableInputFrame>(reliable_json).unwrap(),
        reliable
    );
}
