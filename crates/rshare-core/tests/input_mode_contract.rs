//! Input mode IPC contract tests
//!
//! These tests verify that backend status types can be serialized
//! and deserialized through the IPC layer.

use rshare_core::{
    BackendFailureReason, BackendHealth, BackendKind, PrivilegeState, ResolvedInputMode,
};
use serde_json;

/// Helper to create a sample status for testing
fn sample_status() -> serde_json::Value {
    serde_json::json!({
        "device_id": "00000000-0000-0000-0000-000000000001",
        "device_name": "test-device",
        "hostname": "test-host",
        "bind_address": "0.0.0.0:27435",
        "discovery_port": 27432,
        "pid": 12345,
        "discovered_devices": 0,
        "connected_devices": 0,
        "healthy": true
    })
}

#[test]
fn backend_kind_portable_serializes_correctly() {
    let kind = BackendKind::Portable;
    let serialized = serde_json::to_string(&kind).unwrap();
    let deserialized: BackendKind = serde_json::from_str(&serialized).unwrap();
    assert_eq!(kind, deserialized);
}

#[cfg(target_os = "windows")]
#[test]
fn backend_kind_windows_serializes_correctly() {
    let kinds = vec![BackendKind::WindowsNative, BackendKind::VirtualHid];

    for kind in kinds {
        let serialized = serde_json::to_string(&kind).unwrap();
        let deserialized: BackendKind = serde_json::from_str(&serialized).unwrap();
        assert_eq!(kind, deserialized);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn backend_kind_linux_serializes_correctly() {
    let kinds = vec![BackendKind::Evdev, BackendKind::UInput];

    for kind in kinds {
        let serialized = serde_json::to_string(&kind).unwrap();
        let deserialized: BackendKind = serde_json::from_str(&serialized).unwrap();
        assert_eq!(kind, deserialized);
    }
}

#[test]
fn backend_health_serializes_correctly() {
    let health_states = vec![
        BackendHealth::Healthy,
        BackendHealth::Degraded {
            reason: BackendFailureReason::InitializationFailed,
        },
        BackendHealth::Degraded {
            reason: BackendFailureReason::RuntimeError,
        },
    ];

    for health in health_states {
        let serialized = serde_json::to_string(&health).unwrap();
        let deserialized: BackendHealth = serde_json::from_str(&serialized).unwrap();
        assert_eq!(health, deserialized);
    }
}

#[test]
fn privilege_state_serializes_correctly() {
    let states = vec![
        PrivilegeState::UnlockedDesktop,
        PrivilegeState::LockedDesktop,
        PrivilegeState::SecureDesktop,
        PrivilegeState::SessionUnavailable,
    ];

    for state in states {
        let serialized = serde_json::to_string(&state).unwrap();
        let deserialized: PrivilegeState = serde_json::from_str(&serialized).unwrap();
        assert_eq!(state, deserialized);
    }
}

#[test]
fn resolved_input_mode_portable_serializes_correctly() {
    let mode = ResolvedInputMode::Portable;
    let serialized = serde_json::to_string(&mode).unwrap();
    let deserialized: ResolvedInputMode = serde_json::from_str(&serialized).unwrap();
    assert_eq!(mode, deserialized);
}

#[cfg(target_os = "windows")]
#[test]
fn resolved_input_mode_windows_serializes_correctly() {
    let modes = vec![
        ResolvedInputMode::WindowsNative,
        ResolvedInputMode::VirtualHid,
    ];

    for mode in modes {
        let serialized = serde_json::to_string(&mode).unwrap();
        let deserialized: ResolvedInputMode = serde_json::from_str(&serialized).unwrap();
        assert_eq!(mode, deserialized);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn resolved_input_mode_linux_serializes_correctly() {
    let modes = vec![ResolvedInputMode::Evdev, ResolvedInputMode::UInput];

    for mode in modes {
        let serialized = serde_json::to_string(&mode).unwrap();
        let deserialized: ResolvedInputMode = serde_json::from_str(&serialized).unwrap();
        assert_eq!(mode, deserialized);
    }
}

#[test]
fn daemon_status_round_trips_input_backend_fields() {
    let mut status = sample_status();
    status["input_mode"] = serde_json::to_value(ResolvedInputMode::Portable).unwrap();
    status["available_backends"] = serde_json::to_value(vec![BackendKind::Portable]).unwrap();
    status["backend_health"] = serde_json::to_value(BackendHealth::Healthy).unwrap();
    status["privilege_state"] = serde_json::to_value(PrivilegeState::UnlockedDesktop).unwrap();

    let serialized = serde_json::to_string(&status).unwrap();
    let deserialized: serde_json::Value = serde_json::from_str(&serialized).unwrap();

    assert_eq!(
        deserialized["input_mode"],
        serde_json::to_value(ResolvedInputMode::Portable).unwrap()
    );
    assert_eq!(
        deserialized["available_backends"].as_array().unwrap().len(),
        1
    );
}

#[cfg(target_os = "windows")]
#[test]
fn daemon_status_round_trips_windows_backends() {
    let mut status = sample_status();
    status["input_mode"] = serde_json::to_value(ResolvedInputMode::WindowsNative).unwrap();
    status["available_backends"] =
        serde_json::to_value(vec![BackendKind::Portable, BackendKind::WindowsNative]).unwrap();
    status["backend_health"] = serde_json::to_value(BackendHealth::Healthy).unwrap();
    status["privilege_state"] = serde_json::to_value(PrivilegeState::UnlockedDesktop).unwrap();

    let serialized = serde_json::to_string(&status).unwrap();
    let deserialized: serde_json::Value = serde_json::from_str(&serialized).unwrap();

    assert_eq!(
        deserialized["input_mode"],
        serde_json::to_value(ResolvedInputMode::WindowsNative).unwrap()
    );
    assert_eq!(
        deserialized["available_backends"].as_array().unwrap().len(),
        2
    );
}

#[cfg(target_os = "linux")]
#[test]
fn daemon_status_round_trips_linux_backends() {
    let mut status = sample_status();
    status["input_mode"] = serde_json::to_value(ResolvedInputMode::Evdev).unwrap();
    status["available_backends"] =
        serde_json::to_value(vec![BackendKind::Portable, BackendKind::Evdev]).unwrap();
    status["backend_health"] = serde_json::to_value(BackendHealth::Healthy).unwrap();
    status["privilege_state"] = serde_json::to_value(PrivilegeState::UnlockedDesktop).unwrap();

    let serialized = serde_json::to_string(&status).unwrap();
    let deserialized: serde_json::Value = serde_json::from_str(&serialized).unwrap();

    assert_eq!(
        deserialized["input_mode"],
        serde_json::to_value(ResolvedInputMode::Evdev).unwrap()
    );
    assert_eq!(
        deserialized["available_backends"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn backend_failure_reason_serializes_correctly() {
    let reasons = vec![
        BackendFailureReason::InitializationFailed,
        BackendFailureReason::RuntimeError,
        BackendFailureReason::PermissionDenied,
        BackendFailureReason::VersionMismatch,
        BackendFailureReason::Unavailable,
    ];

    for reason in reasons {
        let serialized = serde_json::to_string(&reason).unwrap();
        let deserialized: BackendFailureReason = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reason, deserialized);
    }
}

#[test]
fn daemon_status_marks_degraded_after_backend_failure() {
    let mut status = sample_status();
    status["input_mode"] = serde_json::to_value(ResolvedInputMode::Portable).unwrap();
    status["backend_health"] = serde_json::to_value(BackendHealth::Degraded {
        reason: BackendFailureReason::Unavailable,
    })
    .unwrap();
    status["last_backend_error"] = serde_json::to_value("Preferred backend unavailable").unwrap();

    let serialized = serde_json::to_string(&status).unwrap();
    let deserialized: serde_json::Value = serde_json::from_str(&serialized).unwrap();

    assert_eq!(
        deserialized["input_mode"],
        serde_json::to_value(ResolvedInputMode::Portable).unwrap()
    );

    let health = &deserialized["backend_health"];
    assert!(health.is_object());
    assert_eq!(health["Degraded"]["reason"], "Unavailable");
    assert_eq!(
        deserialized["last_backend_error"],
        "Preferred backend unavailable"
    );
}

#[test]
fn daemon_status_preserves_healthy_when_backend_fine() {
    let mut status = sample_status();
    status["input_mode"] = serde_json::to_value(ResolvedInputMode::Portable).unwrap();
    status["backend_health"] = serde_json::to_value(BackendHealth::Healthy).unwrap();
    status["privilege_state"] = serde_json::to_value(PrivilegeState::UnlockedDesktop).unwrap();

    let serialized = serde_json::to_string(&status).unwrap();
    let deserialized: serde_json::Value = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized["backend_health"], "Healthy");
    assert_eq!(deserialized["privilege_state"], "UnlockedDesktop");
}
