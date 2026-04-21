//! Privilege and session state tracking
//!
//! This module provides backends for tracking desktop session state,
//! such as locked/unlocked desktop, secure desktop (UAC), and session availability.

#[cfg(target_os = "windows")]
use crate::backend::PrivilegeBackend;
#[cfg(target_os = "windows")]
use anyhow::Result;
#[cfg(target_os = "windows")]
use std::fmt::Debug;

/// Windows privilege state tracker.
#[cfg(target_os = "windows")]
pub struct WindowsPrivilegeBackend {
    state: rshare_core::PrivilegeState,
}

#[cfg(target_os = "windows")]
impl WindowsPrivilegeBackend {
    /// Create a new Windows privilege backend.
    pub fn new() -> Self {
        Self {
            state: rshare_core::PrivilegeState::UnlockedDesktop,
        }
    }

    /// Create a new Windows privilege backend for testing.
    pub fn new_for_test() -> Result<Self> {
        Ok(Self::new())
    }

    /// Update the current state (for testing/simulation).
    pub fn set_state(&mut self, state: rshare_core::PrivilegeState) {
        self.state = state;
    }
}

#[cfg(target_os = "windows")]
impl Default for WindowsPrivilegeBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
impl Debug for WindowsPrivilegeBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsPrivilegeBackend")
            .field("state", &self.state)
            .finish()
    }
}

#[cfg(target_os = "windows")]
impl PrivilegeBackend for WindowsPrivilegeBackend {
    fn current_state(&self) -> rshare_core::PrivilegeState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use crate::backend::PrivilegeBackend;

    #[test]
    fn noop_privilege_backend_reports_unrestricted_state() {
        let backend = crate::backend::NoopPrivilegeBackend::default();
        assert_eq!(
            backend.current_state(),
            rshare_core::PrivilegeState::UnlockedDesktop
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_privilege_backend_reports_initial_state() {
        let backend = WindowsPrivilegeBackend::new();
        assert_eq!(
            backend.current_state(),
            rshare_core::PrivilegeState::UnlockedDesktop
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_privilege_backend_can_update_state() {
        let mut backend = WindowsPrivilegeBackend::new();
        backend.set_state(rshare_core::PrivilegeState::LockedDesktop);
        assert_eq!(
            backend.current_state(),
            rshare_core::PrivilegeState::LockedDesktop
        );
    }
}
