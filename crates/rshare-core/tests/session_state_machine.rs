//! Session state machine contract tests for Alpha-2
//!
//! This test module verifies that the capture session state machine
//! correctly transitions between states based on edge hits and
//! connection events.

use rshare_core::{CaptureSessionStateMachine, ControlSessionState, Direction, SuspendReason};
use uuid::Uuid;

#[test]
fn session_starts_in_local_ready() {
    let machine = CaptureSessionStateMachine::new();
    assert_eq!(machine.state(), ControlSessionState::LocalReady);
}

#[test]
fn local_state_does_not_forward_without_valid_edge() {
    let mut machine = CaptureSessionStateMachine::new();

    // Edge hit with no target should not transition
    let result = machine.on_edge_hit(Direction::Right, None);
    assert!(result.is_err());
    assert_eq!(machine.state(), ControlSessionState::LocalReady);
}

#[test]
fn valid_edge_hit_transitions_to_remote_active() {
    let mut machine = CaptureSessionStateMachine::new();
    let remote_id = Uuid::new_v4();

    machine.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
    assert_eq!(machine.state(), ControlSessionState::RemoteActive {
        target: remote_id,
        entered_via: Direction::Right,
    });
}

#[test]
fn return_edge_transitions_back_to_local() {
    let mut machine = CaptureSessionStateMachine::new();
    let remote_id = Uuid::new_v4();

    // First enter remote mode
    machine.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
    assert!(matches!(machine.state(), ControlSessionState::RemoteActive { .. }));

    // Then return via left edge (opposite of right)
    machine.on_return_edge_hit(Direction::Left).unwrap();
    assert_eq!(machine.state(), ControlSessionState::LocalReady);
}

#[test]
fn target_disconnect_transitions_to_suspended() {
    let mut machine = CaptureSessionStateMachine::new();
    let remote_id = Uuid::new_v4();

    // Enter remote mode
    machine.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
    assert!(matches!(machine.state(), ControlSessionState::RemoteActive { .. }));

    // Target disconnects
    machine.on_target_disconnect(remote_id);
    assert_eq!(machine.state(), ControlSessionState::Suspended {
        reason: SuspendReason::TargetUnavailable,
    });
}

#[test]
fn backend_degradation_prevents_forwarding() {
    let mut machine = CaptureSessionStateMachine::new();
    let remote_id = Uuid::new_v4();

    // Backend degrades
    machine.on_backend_degraded();

    // Edge hit should not work
    let result = machine.on_edge_hit(Direction::Right, Some(remote_id));
    assert!(result.is_err());

    // State should be suspended
    assert_eq!(machine.state(), ControlSessionState::Suspended {
        reason: SuspendReason::BackendDegraded,
    });
}

#[test]
fn reset_returns_to_local_ready() {
    let mut machine = CaptureSessionStateMachine::new();
    let remote_id = Uuid::new_v4();

    // Enter remote mode
    machine.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
    assert!(matches!(machine.state(), ControlSessionState::RemoteActive { .. }));

    // Reset
    machine.reset();
    assert_eq!(machine.state(), ControlSessionState::LocalReady);
}

#[test]
fn reconnect_can_restore_target() {
    let mut machine = CaptureSessionStateMachine::new();
    let remote_id = Uuid::new_v4();

    // Enter and then disconnect
    machine.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
    machine.on_target_disconnect(remote_id);
    assert!(matches!(machine.state(), ControlSessionState::Suspended { .. }));

    // Reset to allow new transitions
    machine.reset();
    assert_eq!(machine.state(), ControlSessionState::LocalReady);

    // Can enter remote mode again
    machine.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
    assert!(matches!(machine.state(), ControlSessionState::RemoteActive { .. }));
}
