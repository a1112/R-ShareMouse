//! Screen edge switching state machine
//!
//! This module manages the state transitions when the cursor moves
//! between screens on different devices.

use anyhow::Result;
use std::time::{Duration, Instant};

use crate::{DeviceId, Direction};

/// Mode of input capture
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// Capturing input locally (normal operation)
    Local,
    /// Forwarding input to remote device
    Remote { target: DeviceId },
}

impl CaptureMode {
    pub fn is_local(&self) -> bool {
        matches!(self, CaptureMode::Local)
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, CaptureMode::Remote { .. })
    }

    pub fn remote_target(&self) -> Option<DeviceId> {
        match self {
            CaptureMode::Remote { target } => Some(*target),
            _ => None,
        }
    }
}

/// State machine for managing screen transitions
pub struct EdgeStateMachine {
    /// Current capture mode
    mode: CaptureMode,

    /// Device we're currently controlling (if remote)
    remote_device: Option<DeviceId>,

    /// Direction we exited the local screen
    exit_direction: Option<Direction>,

    /// Last transition time (for debouncing)
    last_transition: Option<Instant>,

    /// Cooldown period between transitions
    cooldown: Duration,

    /// Whether to enable automatic return
    auto_return: bool,

    /// Screen entry position when entering remote mode
    entry_position: Option<(i32, i32)>,
}

impl EdgeStateMachine {
    /// Create a new state machine
    pub fn new() -> Self {
        Self {
            mode: CaptureMode::Local,
            remote_device: None,
            exit_direction: None,
            last_transition: None,
            cooldown: Duration::from_millis(500),
            auto_return: true,
            entry_position: None,
        }
    }

    /// Set the cooldown period
    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = cooldown;
        self
    }

    /// Set whether to enable automatic return
    pub fn with_auto_return(mut self, auto_return: bool) -> Self {
        self.auto_return = auto_return;
        self
    }

    /// Get the current capture mode
    pub fn mode(&self) -> CaptureMode {
        self.mode
    }

    /// Get the remote device (if in remote mode)
    pub fn remote_device(&self) -> Option<DeviceId> {
        self.remote_device
    }

    /// Check if in local mode
    pub fn is_local(&self) -> bool {
        self.mode.is_local()
    }

    /// Check if in remote mode
    pub fn is_remote(&self) -> bool {
        self.mode.is_remote()
    }

    /// Check if we're in cooldown period
    pub fn is_in_cooldown(&self) -> bool {
        self.last_transition
            .map(|t| t.elapsed() < self.cooldown)
            .unwrap_or(false)
    }

    /// Transition to remote mode
    pub fn enter_remote(
        &mut self,
        target_device: DeviceId,
        direction: Direction,
        entry_pos: (i32, i32),
    ) -> Result<()> {
        if self.is_in_cooldown() {
            anyhow::bail!("Cannot transition: in cooldown period");
        }

        tracing::info!(
            "Entering remote mode: target={:?}, direction={:?}, pos={:?}",
            target_device,
            direction,
            entry_pos
        );

        self.mode = CaptureMode::Remote {
            target: target_device,
        };
        self.remote_device = Some(target_device);
        self.exit_direction = Some(direction);
        self.entry_position = Some(entry_pos);
        self.last_transition = Some(Instant::now());

        Ok(())
    }

    /// Transition back to local mode
    pub fn enter_local(&mut self) -> Result<()> {
        if !self.is_remote() {
            return Ok(());
        }

        tracing::info!("Entering local mode (returning from remote)");

        self.mode = CaptureMode::Local;
        self.remote_device = None;
        self.exit_direction = None;
        self.entry_position = None;
        self.last_transition = Some(Instant::now());

        Ok(())
    }

    /// Handle a screen edge crossing event
    pub fn handle_edge_cross(
        &mut self,
        direction: Direction,
        target_device: Option<DeviceId>,
        cursor_pos: (i32, i32),
    ) -> Result<Transition> {
        // Check cooldown
        if self.is_in_cooldown() {
            return Ok(Transition::None);
        }

        match self.mode {
            CaptureMode::Local => {
                // Exiting local screen to remote
                if let Some(target) = target_device {
                    self.enter_remote(target, direction, cursor_pos)?;
                    Ok(Transition::ToRemote {
                        device: target,
                        direction,
                    })
                } else {
                    tracing::debug!("No target device for direction {:?}", direction);
                    Ok(Transition::None)
                }
            }
            CaptureMode::Remote { target } => {
                // Returning from remote to local
                let expected_return = self.exit_direction.map(|d| d.opposite());

                if auto_return_enabled(self, direction, expected_return) {
                    self.enter_local()?;
                    Ok(Transition::ToLocal { direction })
                } else {
                    // Switching to another remote device
                    if let Some(new_target) = target_device {
                        if new_target != target {
                            self.enter_remote(new_target, direction, cursor_pos)?;
                            Ok(Transition::SwitchRemote {
                                from: target,
                                to: new_target,
                                direction,
                            })
                        } else {
                            Ok(Transition::None)
                        }
                    } else {
                        Ok(Transition::None)
                    }
                }
            }
        }
    }

    /// Reset the state machine
    pub fn reset(&mut self) {
        self.mode = CaptureMode::Local;
        self.remote_device = None;
        self.exit_direction = None;
        self.entry_position = None;
        self.last_transition = None;
    }

    /// Force set the mode (useful for initialization)
    pub fn set_mode(&mut self, mode: CaptureMode) {
        self.mode = mode;
        match mode {
            CaptureMode::Local => {
                self.remote_device = None;
                self.exit_direction = None;
                self.entry_position = None;
            }
            CaptureMode::Remote { target } => {
                self.remote_device = Some(target);
            }
        }
    }
}

impl Default for EdgeStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

/// Transition result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    /// No transition occurred
    None,
    /// Transitioning to remote device
    ToRemote {
        device: DeviceId,
        direction: Direction,
    },
    /// Transitioning back to local
    ToLocal { direction: Direction },
    /// Switching between remote devices
    SwitchRemote {
        from: DeviceId,
        to: DeviceId,
        direction: Direction,
    },
}

impl Transition {
    pub fn is_some(&self) -> bool {
        !matches!(self, Transition::None)
    }

    pub fn is_to_remote(&self) -> bool {
        matches!(self, Transition::ToRemote { .. })
    }

    pub fn is_to_local(&self) -> bool {
        matches!(self, Transition::ToLocal { .. })
    }
}

/// Check if auto-return should happen
fn auto_return_enabled(
    state: &EdgeStateMachine,
    direction: Direction,
    expected_return: Option<Direction>,
) -> bool {
    if !state.auto_return {
        return false;
    }

    match expected_return {
        Some(expected) => direction == expected,
        None => false,
    }
}

/// State machine for managing multiple simultaneous connections
pub struct MultiDeviceStateMachine {
    local_id: DeviceId,
    current_mode: CaptureMode,
    connections: Vec<DeviceId>,
    edge_mappings: std::collections::HashMap<Direction, DeviceId>,
}

impl MultiDeviceStateMachine {
    pub fn new(local_id: DeviceId) -> Self {
        Self {
            local_id,
            current_mode: CaptureMode::Local,
            connections: Vec::new(),
            edge_mappings: std::collections::HashMap::new(),
        }
    }

    /// Add a connected device
    pub fn add_connection(&mut self, device_id: DeviceId) {
        if !self.connections.contains(&device_id) {
            self.connections.push(device_id);
        }
    }

    /// Remove a connected device
    pub fn remove_connection(&mut self, device_id: &DeviceId) {
        self.connections.retain(|id| id != device_id);
        self.edge_mappings.retain(|_, id| id != device_id);

        // If we were controlling this device, return to local
        if self.current_mode.remote_target() == Some(*device_id) {
            self.current_mode = CaptureMode::Local;
        }
    }

    /// Set the edge mapping
    pub fn set_edge_mapping(&mut self, direction: Direction, device_id: DeviceId) {
        self.edge_mappings.insert(direction, device_id);
    }

    /// Get target device for a direction
    pub fn get_target(&self, direction: Direction) -> Option<DeviceId> {
        self.edge_mappings.get(&direction).copied()
    }

    /// Get current mode
    pub fn mode(&self) -> CaptureMode {
        self.current_mode
    }

    /// Get the local device id this state machine represents.
    pub fn local_id(&self) -> DeviceId {
        self.local_id
    }

    /// Transition to remote mode
    pub fn transition_to_remote(&mut self, device_id: DeviceId) -> Result<()> {
        if !self.connections.contains(&device_id) {
            anyhow::bail!("Device {} is not connected", device_id);
        }

        self.current_mode = CaptureMode::Remote { target: device_id };
        tracing::info!("Transitioned to remote mode: controlling {:?}", device_id);

        Ok(())
    }

    /// Transition to local mode
    pub fn transition_to_local(&mut self) -> Result<()> {
        self.current_mode = CaptureMode::Local;
        tracing::info!("Transitioned to local mode");

        Ok(())
    }

    /// Get all connected devices
    pub fn connections(&self) -> &[DeviceId] {
        &self.connections
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture_mode() {
        let mode = CaptureMode::Local;
        assert!(mode.is_local());
        assert!(!mode.is_remote());

        let remote = CaptureMode::Remote {
            target: DeviceId::new_v4(),
        };
        assert!(remote.is_remote());
        assert!(remote.remote_target().is_some());
    }

    #[test]
    fn test_state_machine_new() {
        let sm = EdgeStateMachine::new();
        assert!(sm.is_local());
        assert!(!sm.is_remote());
    }

    #[test]
    fn test_enter_remote() {
        let mut sm = EdgeStateMachine::new();
        let target = DeviceId::new_v4();

        sm.enter_remote(target, Direction::Right, (0, 0)).unwrap();
        assert!(sm.is_remote());
        assert_eq!(sm.remote_device(), Some(target));
    }

    #[test]
    fn test_enter_local() {
        let mut sm = EdgeStateMachine::new();
        let target = DeviceId::new_v4();

        sm.enter_remote(target, Direction::Right, (0, 0)).unwrap();
        sm.enter_local().unwrap();
        assert!(sm.is_local());
    }

    #[test]
    fn test_cooldown() {
        let mut sm = EdgeStateMachine::new().with_cooldown(Duration::from_millis(100));

        let target = DeviceId::new_v4();
        sm.enter_remote(target, Direction::Right, (0, 0)).unwrap();

        // Immediate enter_remote again should fail due to cooldown
        let target2 = DeviceId::new_v4();
        sm.enter_remote(target2, Direction::Left, (0, 0))
            .unwrap_err();
    }

    #[test]
    fn test_multi_device_state() {
        let local_id = DeviceId::new_v4();
        let mut mdsm = MultiDeviceStateMachine::new(local_id);

        let device1 = DeviceId::new_v4();
        let device2 = DeviceId::new_v4();

        mdsm.add_connection(device1);
        mdsm.add_connection(device2);

        assert_eq!(mdsm.connections().len(), 2);

        mdsm.set_edge_mapping(Direction::Right, device1);
        assert_eq!(mdsm.get_target(Direction::Right), Some(device1));
    }

    #[test]
    fn test_transition() {
        let device = DeviceId::new_v4();
        let transition = Transition::ToRemote {
            device,
            direction: Direction::Right,
        };

        assert!(transition.is_some());
        assert!(transition.is_to_remote());
        assert!(!transition.is_to_local());
    }
}
