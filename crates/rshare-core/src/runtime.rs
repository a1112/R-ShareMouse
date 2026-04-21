//! Alpha-2 runtime state models
//!
//! This module defines the canonical runtime types that the daemon owns
//! and exposes through IPC to GUI and CLI clients.

use serde::{Deserialize, Serialize};
use std::time::Instant;
use uuid::Uuid;

use crate::{BackendHealth, BackendKind, Direction, PrivilegeState, ResolvedInputMode};

/// Discovery state of a peer device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscoveryState {
    /// Device has been discovered on the network.
    Discovered,
    /// Device discovery has timed out or entry is stale.
    Expired,
    /// Device was never discovered.
    NotFound,
}

/// Connection state of a peer device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    /// Active connection established.
    Connected,
    /// Connection in progress.
    Connecting,
    /// Disconnected.
    Disconnected,
    /// Connection failed.
    Failed,
}

/// Unified peer directory entry combining discovery and connection state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerDirectoryEntry {
    /// Unique device identifier.
    pub id: Uuid,
    /// Human-readable device name.
    pub name: String,
    /// Device hostname.
    pub hostname: String,
    /// Network addresses for this device.
    pub addresses: Vec<String>,
    /// Current discovery state.
    pub discovery_state: DiscoveryState,
    /// Current connection state.
    pub connection_state: ConnectionState,
    /// Last time this device was seen (as Unix timestamp seconds).
    #[serde(default = "PeerDirectoryEntry::default_last_seen")]
    pub last_seen_secs: u64,
    /// Last error encountered (if any).
    pub last_error: Option<String>,
}

impl PeerDirectoryEntry {
    fn default_last_seen() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Get the last seen time as an Instant (for local use).
    pub fn last_seen_instant(&self) -> Instant {
        // This is approximate; for accurate local timing use a monotonic clock
        let now = Instant::now();
        let duration = std::time::Duration::from_secs(self.last_seen_secs);
        // Note: This is an approximation and won't be exact for serialization purposes
        now.checked_sub(duration).unwrap_or(now)
    }
}

/// Reason for session suspension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuspendReason {
    /// Target device disconnected or became unavailable.
    TargetUnavailable,
    /// Input backend degraded and cannot forward.
    BackendDegraded,
    /// Manual suspension by operator.
    Manual,
    /// Service restart or recovery in progress.
    ServiceRestart,
}

/// Process that owns a product runtime surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackgroundProcessOwner {
    /// The standalone daemon owns the runtime surface.
    Daemon,
    /// The desktop shell owns the runtime surface.
    Desktop,
}

/// How the daemon is currently running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackgroundRunMode {
    /// Started as a foreground console process.
    ForegroundProcess,
    /// Running detached from the desktop control window.
    BackgroundProcess,
}

/// Current daemon-owned tray runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrayRuntimeState {
    /// Native tray runtime has not been wired on this platform/build.
    Unavailable,
    /// Tray runtime is being initialized.
    Starting,
    /// Tray runtime is active.
    Running,
    /// Tray runtime failed and the daemon continues without it.
    Failed,
}

/// Control session state owned by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlSessionState {
    /// Local input is captured but not forwarded.
    LocalReady,
    /// Transitioning to remote control after edge hit.
    TransitioningToRemote { target: Uuid, edge: Direction },
    /// Actively forwarding input to remote device.
    RemoteActive {
        target: Uuid,
        entered_via: Direction,
    },
    /// Returning to local control after return edge hit.
    ReturningLocal { from: Uuid },
    /// Forwarding suspended due to degradation.
    Suspended { reason: SuspendReason },
}

/// Runtime state of the input backend.
///
/// This separates capture and inject health to allow the daemon to report
/// "service up but input degraded" states.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendRuntimeState {
    /// The selected input mode, if any end-to-end path exists.
    pub selected_mode: Option<ResolvedInputMode>,
    /// Available backend kinds on this system.
    pub available_backends: Vec<BackendKind>,
    /// Health of the capture (local event ingestion) backend.
    pub capture_health: BackendHealth,
    /// Health of the injection (remote event playback) backend.
    pub inject_health: BackendHealth,
    /// Aggregate health (worst of capture and inject).
    pub aggregate_health: BackendHealth,
    /// Current desktop session privilege state.
    pub privilege_state: PrivilegeState,
    /// Last error message (if any).
    pub last_error: Option<String>,
}

impl BackendRuntimeState {
    /// Create a new backend runtime state with default values.
    pub fn new() -> Self {
        Self {
            selected_mode: None,
            available_backends: Vec::new(),
            capture_health: BackendHealth::Healthy,
            inject_health: BackendHealth::Healthy,
            aggregate_health: BackendHealth::Healthy,
            privilege_state: PrivilegeState::UnlockedDesktop,
            last_error: None,
        }
    }

    /// Update the aggregate health based on capture and inject states.
    pub fn update_aggregate_health(&mut self) {
        self.aggregate_health = match (&self.capture_health, &self.inject_health) {
            (BackendHealth::Healthy, BackendHealth::Healthy) => BackendHealth::Healthy,
            (BackendHealth::Degraded { reason: r }, _)
            | (_, BackendHealth::Degraded { reason: r }) => {
                BackendHealth::Degraded { reason: r.clone() }
            }
        };
    }

    /// Check if there is a working end-to-end input path.
    pub fn has_end_to_end_path(&self) -> bool {
        self.selected_mode.is_some() && matches!(self.aggregate_health, BackendHealth::Healthy)
    }
}

impl Default for BackendRuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BackendFailureReason;

    #[test]
    fn test_backend_runtime_state_new() {
        let state = BackendRuntimeState::new();
        assert_eq!(state.selected_mode, None);
        assert!(state.available_backends.is_empty());
        assert!(matches!(state.capture_health, BackendHealth::Healthy));
        assert!(matches!(state.inject_health, BackendHealth::Healthy));
        assert!(matches!(state.aggregate_health, BackendHealth::Healthy));
    }

    #[test]
    fn test_backend_runtime_state_degraded_capture() {
        let mut state = BackendRuntimeState::new();
        state.capture_health = BackendHealth::Degraded {
            reason: BackendFailureReason::InitializationFailed,
        };
        state.update_aggregate_health();

        assert!(matches!(
            state.aggregate_health,
            BackendHealth::Degraded { .. }
        ));
        assert!(!state.has_end_to_end_path());
    }

    #[test]
    fn test_backend_runtime_state_degraded_inject() {
        let mut state = BackendRuntimeState::new();
        state.inject_health = BackendHealth::Degraded {
            reason: BackendFailureReason::RuntimeError,
        };
        state.update_aggregate_health();

        assert!(matches!(
            state.aggregate_health,
            BackendHealth::Degraded { .. }
        ));
        assert!(!state.has_end_to_end_path());
    }

    #[test]
    fn test_backend_runtime_state_has_end_to_end_path() {
        let mut state = BackendRuntimeState::new();
        state.selected_mode = Some(ResolvedInputMode::Portable);
        state.update_aggregate_health();

        assert!(state.has_end_to_end_path());
    }
}
