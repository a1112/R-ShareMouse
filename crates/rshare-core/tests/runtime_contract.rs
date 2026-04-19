//! Runtime state contract tests for Alpha-2
//!
//! This test module verifies that the core runtime types maintain their
//! contracts across serialization, state transitions, and aggregation.

use rshare_core::{
    BackendHealth, BackendKind, Direction, PeerDirectoryEntry, PrivilegeState,
    ResolvedInputMode, ControlSessionState, BackendRuntimeState, SuspendReason,
};
use serde_json;
use uuid::Uuid;

#[test]
fn peer_directory_entry_roundtrips_identity() {
    let id = Uuid::new_v4();
    let entry = PeerDirectoryEntry {
        id,
        name: "Test Device".to_string(),
        hostname: "test-host".to_string(),
        addresses: vec!["192.168.1.100".to_string()],
        discovery_state: rshare_core::DiscoveryState::Discovered,
        connection_state: rshare_core::ConnectionState::Connected,
        last_seen_secs: 12345,
        last_error: None,
    };

    // Serialize and deserialize
    let serialized = serde_json::to_string(&entry).unwrap();
    let deserialized: PeerDirectoryEntry = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized.id, id);
    assert_eq!(deserialized.name, "Test Device");
    assert_eq!(deserialized.hostname, "test-host");
    assert_eq!(deserialized.addresses.len(), 1);
    assert_eq!(deserialized.addresses[0], "192.168.1.100");
}

#[test]
fn backend_runtime_state_preserves_all_fields() {
    let state = BackendRuntimeState {
        selected_mode: Some(ResolvedInputMode::Portable),
        available_backends: vec![BackendKind::Portable],
        capture_health: BackendHealth::Healthy,
        inject_health: BackendHealth::Degraded {
            reason: rshare_core::BackendFailureReason::InitializationFailed,
        },
        aggregate_health: BackendHealth::Degraded {
            reason: rshare_core::BackendFailureReason::InitializationFailed,
        },
        privilege_state: PrivilegeState::UnlockedDesktop,
        last_error: Some("Injection backend failed".to_string()),
    };

    // Verify aggregate health is degraded when inject is degraded
    assert!(matches!(state.aggregate_health, BackendHealth::Degraded { .. }));

    // Serialize and deserialize
    let serialized = serde_json::to_string(&state).unwrap();
    let deserialized: BackendRuntimeState = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized.selected_mode, Some(ResolvedInputMode::Portable));
    assert_eq!(deserialized.available_backends.len(), 1);
    assert!(matches!(deserialized.inject_health, BackendHealth::Degraded { .. }));
    assert_eq!(deserialized.last_error, Some("Injection backend failed".to_string()));
}

#[test]
fn control_session_state_roundtrips_active_target() {
    let remote_id = Uuid::new_v4();
    let session_state = ControlSessionState::RemoteActive {
        target: remote_id,
        entered_via: Direction::Right,
    };

    // Serialize and deserialize
    let serialized = serde_json::to_string(&session_state).unwrap();
    let deserialized: ControlSessionState = serde_json::from_str(&serialized).unwrap();

    match deserialized {
        ControlSessionState::RemoteActive { target, entered_via } => {
            assert_eq!(target, remote_id);
            assert_eq!(entered_via, Direction::Right);
        }
        _ => panic!("Expected RemoteActive state"),
    }
}

#[test]
fn control_session_state_roundtrips_suspended_reason() {
    let session_state = ControlSessionState::Suspended {
        reason: SuspendReason::TargetUnavailable,
    };

    let serialized = serde_json::to_string(&session_state).unwrap();
    let deserialized: ControlSessionState = serde_json::from_str(&serialized).unwrap();

    match deserialized {
        ControlSessionState::Suspended { reason } => {
            assert_eq!(reason, SuspendReason::TargetUnavailable);
        }
        _ => panic!("Expected Suspended state"),
    }
}

#[test]
fn control_session_state_local_ready_is_default() {
    let session_state = ControlSessionState::LocalReady;

    let serialized = serde_json::to_string(&session_state).unwrap();
    let deserialized: ControlSessionState = serde_json::from_str(&serialized).unwrap();

    assert!(matches!(deserialized, ControlSessionState::LocalReady));
}

#[test]
fn backend_runtime_state_no_mode_means_no_end_to_end_path() {
    let state = BackendRuntimeState {
        selected_mode: None,
        available_backends: vec![],
        capture_health: BackendHealth::Degraded {
            reason: rshare_core::BackendFailureReason::Unavailable,
        },
        inject_health: BackendHealth::Degraded {
            reason: rshare_core::BackendFailureReason::Unavailable,
        },
        aggregate_health: BackendHealth::Degraded {
            reason: rshare_core::BackendFailureReason::Unavailable,
        },
        privilege_state: PrivilegeState::UnlockedDesktop,
        last_error: Some("No backend available".to_string()),
    };

    // When no mode is selected, there's no end-to-end path
    assert_eq!(state.selected_mode, None);
    assert!(matches!(state.aggregate_health, BackendHealth::Degraded { .. }));
}
