//! Input backend mode and health types
//!
//! This module defines the shared types for describing input backends,
//! their health status, and the resolved input mode used by the daemon.

use serde::{Deserialize, Serialize};

/// The kind of input backend being used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BackendKind {
    /// Portable cross-platform backend using rdev/enigo
    Portable,
    /// Windows-native backend using low-level hooks and SendInput
    #[cfg(target_os = "windows")]
    WindowsNative,
    /// Virtual HID driver backend (Windows-only, optional)
    #[cfg(target_os = "windows")]
    VirtualHid,
}

/// Health status of a backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendHealth {
    /// Backend is healthy and operational.
    Healthy,
    /// Backend is degraded with a specific failure reason.
    Degraded { reason: BackendFailureReason },
}

/// Reason for a backend failure or degradation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendFailureReason {
    /// Backend failed to initialize.
    InitializationFailed,
    /// Backend encountered a runtime error.
    RuntimeError,
    /// Backend lacks required permissions.
    PermissionDenied,
    /// Backend version is incompatible.
    VersionMismatch,
    /// Backend is not available on this system.
    Unavailable,
}

/// The resolved input mode currently in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedInputMode {
    /// Using portable cross-platform backend.
    Portable,
    /// Using Windows-native backend.
    #[cfg(target_os = "windows")]
    WindowsNative,
    /// Using virtual HID driver backend.
    #[cfg(target_os = "windows")]
    VirtualHid,
}

/// Current privilege/state of the desktop session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrivilegeState {
    /// Normal unlocked desktop - input sharing works normally.
    UnlockedDesktop,
    /// Desktop is locked - input sharing may be restricted.
    LockedDesktop,
    /// Secure desktop (UAC, login screen) - input sharing restricted.
    SecureDesktop,
    /// Session unavailable (switched user, RDP disconnected).
    SessionUnavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kind_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BackendKind>();
    }

    #[test]
    fn backend_health_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BackendHealth>();
    }

    #[test]
    fn privilege_state_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PrivilegeState>();
    }
}
