//! Capture session state machine for Alpha-2
//!
//! This module defines the state machine that controls input forwarding
//! based on edge hits, connection events, and backend health.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ControlSessionState, Direction, SuspendReason};

/// Error type for state machine transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    /// Cannot transition while in suspended state.
    Suspended,
    /// Cannot transition without a valid target.
    NoTarget,
    /// Cannot transition from current state.
    InvalidTransition,
}

/// Capture session state machine.
///
/// This machine tracks whether input should remain local or be forwarded
/// to a remote device, and manages transitions between these states.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureSessionStateMachine {
    /// Current session state.
    state: ControlSessionState,
}

impl CaptureSessionStateMachine {
    /// Create a new state machine in LocalReady state.
    pub fn new() -> Self {
        Self {
            state: ControlSessionState::LocalReady,
        }
    }

    /// Get the current state.
    pub fn state(&self) -> ControlSessionState {
        self.state.clone()
    }

    /// Handle an edge hit from local device.
    ///
    /// Transitions to RemoteActive if:
    /// - Not currently suspended
    /// - A valid target is provided
    /// - Currently in LocalReady state
    pub fn on_edge_hit(
        &mut self,
        edge: Direction,
        target: Option<Uuid>,
    ) -> Result<(), TransitionError> {
        // Check if suspended
        if matches!(self.state, ControlSessionState::Suspended { .. }) {
            return Err(TransitionError::Suspended);
        }

        // Need a target
        let remote_id = target.ok_or(TransitionError::NoTarget)?;

        // Can only enter remote mode from LocalReady
        match &self.state {
            ControlSessionState::LocalReady => {
                self.state = ControlSessionState::RemoteActive {
                    target: remote_id,
                    entered_via: edge,
                };
                Ok(())
            }
            _ => Err(TransitionError::InvalidTransition),
        }
    }

    /// Handle a return edge hit from remote device.
    ///
    /// Transitions back to LocalReady if currently in RemoteActive.
    pub fn on_return_edge_hit(&mut self, _edge: Direction) -> Result<(), TransitionError> {
        match &self.state {
            ControlSessionState::RemoteActive { .. } => {
                self.state = ControlSessionState::LocalReady;
                Ok(())
            }
            _ => Err(TransitionError::InvalidTransition),
        }
    }

    /// Handle target device disconnection.
    ///
    /// Transitions to Suspended if currently in RemoteActive with this target.
    pub fn on_target_disconnect(&mut self, target_id: Uuid) {
        match &self.state {
            ControlSessionState::RemoteActive { target, .. } if *target == target_id => {
                self.state = ControlSessionState::Suspended {
                    reason: SuspendReason::TargetUnavailable,
                };
            }
            _ => {}
        }
    }

    /// Handle backend degradation.
    ///
    /// Transitions to Suspended and prevents further forwarding until reset.
    pub fn on_backend_degraded(&mut self) {
        self.state = ControlSessionState::Suspended {
            reason: SuspendReason::BackendDegraded,
        };
    }

    /// Reset the state machine to LocalReady.
    ///
    /// This can be used to recover from a suspended state.
    pub fn reset(&mut self) {
        self.state = ControlSessionState::LocalReady;
    }

    /// Check if currently in a remote-active state.
    pub fn is_remote_active(&self) -> bool {
        matches!(self.state, ControlSessionState::RemoteActive { .. })
    }

    /// Check if currently in local-ready state.
    pub fn is_local_ready(&self) -> bool {
        matches!(self.state, ControlSessionState::LocalReady)
    }

    /// Check if currently suspended.
    pub fn is_suspended(&self) -> bool {
        matches!(self.state, ControlSessionState::Suspended { .. })
    }

    /// Get the active remote target if in RemoteActive state.
    pub fn active_target(&self) -> Option<Uuid> {
        match &self.state {
            ControlSessionState::RemoteActive { target, .. } => Some(*target),
            _ => None,
        }
    }
}

impl Default for CaptureSessionStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_machine_is_local_ready() {
        let machine = CaptureSessionStateMachine::new();
        assert!(machine.is_local_ready());
        assert!(!machine.is_remote_active());
        assert!(!machine.is_suspended());
    }

    #[test]
    fn test_edge_hit_requires_target() {
        let mut machine = CaptureSessionStateMachine::new();
        let result = machine.on_edge_hit(Direction::Right, None);
        assert_eq!(result, Err(TransitionError::NoTarget));
        assert!(machine.is_local_ready());
    }

    #[test]
    fn test_valid_edge_hit_transitions_to_remote() {
        let mut machine = CaptureSessionStateMachine::new();
        let remote_id = Uuid::new_v4();

        machine.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
        assert!(machine.is_remote_active());
        assert_eq!(machine.active_target(), Some(remote_id));
    }

    #[test]
    fn test_backend_degraded_prevents_forwarding() {
        let mut machine = CaptureSessionStateMachine::new();
        let remote_id = Uuid::new_v4();

        machine.on_backend_degraded();
        assert!(machine.is_suspended());

        let result = machine.on_edge_hit(Direction::Right, Some(remote_id));
        assert_eq!(result, Err(TransitionError::Suspended));
    }

    #[test]
    fn test_reset_clears_suspended_state() {
        let mut machine = CaptureSessionStateMachine::new();
        machine.on_backend_degraded();
        assert!(machine.is_suspended());

        machine.reset();
        assert!(machine.is_local_ready());
    }

    #[test]
    fn test_return_edge_from_remote() {
        let mut machine = CaptureSessionStateMachine::new();
        let remote_id = Uuid::new_v4();

        machine.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
        assert!(machine.is_remote_active());

        machine.on_return_edge_hit(Direction::Left).unwrap();
        assert!(machine.is_local_ready());
    }

    #[test]
    fn test_target_disconnect_from_remote() {
        let mut machine = CaptureSessionStateMachine::new();
        let remote_id = Uuid::new_v4();

        machine.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
        assert!(machine.is_remote_active());

        machine.on_target_disconnect(remote_id);
        assert!(machine.is_suspended());
    }
}
