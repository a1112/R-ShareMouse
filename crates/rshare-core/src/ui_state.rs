//! Versioned, revisioned state contracts for daemon-owned UI projections.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    ButtonState, CapabilityRegistrySnapshot, ControlSessionState, DaemonDeviceSnapshot, DeviceId,
    GamepadButton, KeyState, LatencyFeedbackSnapshot, LayoutGraph, LocalDisplayState,
    LocalGamepadState, MouseButton, ServiceStatusSnapshot,
};

pub const UI_STATE_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiCursor {
    pub boot_id: Uuid,
    pub revision: u64,
}

impl UiCursor {
    pub const fn new(boot_id: Uuid, revision: u64) -> Self {
        Self { boot_id, revision }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiSnapshot {
    pub protocol_version: u16,
    pub boot_id: Uuid,
    pub revision: u64,
    pub status: ServiceStatusSnapshot,
    pub devices: Vec<DaemonDeviceSnapshot>,
    pub layout: LayoutGraph,
    pub capabilities: CapabilityRegistrySnapshot,
    pub display_inventory: LocalDisplayState,
    pub dynamic_state: UiDynamicState,
    pub active_sessions: UiActiveSessions,
}

impl UiSnapshot {
    pub const fn cursor(&self) -> UiCursor {
        UiCursor::new(self.boot_id, self.revision)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiDelta {
    pub boot_id: Uuid,
    pub revision: u64,
    pub change: UiChange,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum UiEnvelope {
    Snapshot(UiSnapshot),
    Delta(UiDelta),
    ResyncRequired {
        boot_id: Uuid,
        current_revision: u64,
        reason: UiResyncReason,
    },
    Heartbeat {
        boot_id: Uuid,
        revision: u64,
        sent_at_ms: u64,
    },
}

impl UiEnvelope {
    pub const fn cursor(&self) -> UiCursor {
        match self {
            Self::Snapshot(snapshot) => snapshot.cursor(),
            Self::Delta(delta) => UiCursor::new(delta.boot_id, delta.revision),
            Self::ResyncRequired {
                boot_id,
                current_revision,
                ..
            } => UiCursor::new(*boot_id, *current_revision),
            Self::Heartbeat {
                boot_id, revision, ..
            } => UiCursor::new(*boot_id, *revision),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UiResyncReason {
    BootChanged,
    RevisionGap,
    HistoryExpired,
    ProjectionRebuilt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum UiChange {
    Status(ServiceStatusSnapshot),
    Capabilities(CapabilityRegistrySnapshot),
    DeviceUpsert(DaemonDeviceSnapshot),
    DeviceRemove(DeviceId),
    Topology(LayoutGraph),
    DisplayInventory(LocalDisplayState),
    Pointer(UiPointerState),
    Gamepads(Vec<LocalGamepadState>),
    KeyButton(UiDiscreteInputState),
    Session(UiActiveSessions),
    Diagnostics(LatencyFeedbackSnapshot),
    MediaSessionUpsert(UiMediaSession),
    MediaSessionRemove(Uuid),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiDynamicState {
    #[serde(default)]
    pub pointer: Option<UiPointerState>,
    #[serde(default)]
    pub gamepads: Vec<LocalGamepadState>,
    #[serde(default)]
    pub pressed_keys: Vec<u32>,
    #[serde(default)]
    pub pressed_mouse_buttons: Vec<MouseButton>,
    #[serde(default)]
    pub pressed_gamepad_buttons: Vec<UiPressedGamepadButton>,
    #[serde(default)]
    pub diagnostics: LatencyFeedbackSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiPointerState {
    pub x: i32,
    pub y: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_id: Option<String>,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiDiscreteInputState {
    Key {
        key_code: u32,
        state: KeyState,
        observed_at_ms: u64,
    },
    MouseButton {
        button: MouseButton,
        state: ButtonState,
        observed_at_ms: u64,
    },
    Wheel {
        delta_x: i32,
        delta_y: i32,
        observed_at_ms: u64,
    },
    GamepadButton {
        gamepad_id: u8,
        button: GamepadButton,
        state: ButtonState,
        observed_at_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiPressedGamepadButton {
    pub gamepad_id: u8,
    pub button: GamepadButton,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiActiveSessions {
    #[serde(default)]
    pub control: Option<ControlSessionState>,
    #[serde(default)]
    pub media_sessions: Vec<UiMediaSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiMediaSession {
    pub session_id: Uuid,
    pub peer_id: DeviceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_id: Option<String>,
    pub state: UiMediaSessionState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UiMediaSessionState {
    Starting,
    Streaming,
    Degraded,
    Stopped { reason: String },
}

/// Single-owner revision source. Revisions are consumed only by emitted deltas.
#[derive(Debug, Clone)]
pub struct UiRevisionSequencer {
    boot_id: Uuid,
    revision: u64,
}

impl UiRevisionSequencer {
    pub const fn new(boot_id: Uuid) -> Self {
        Self {
            boot_id,
            revision: 0,
        }
    }

    pub const fn cursor(&self) -> UiCursor {
        UiCursor::new(self.boot_id, self.revision)
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn emit_delta(&mut self, change: UiChange) -> Result<UiDelta, UiRevisionError> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(UiRevisionError::Exhausted)?;
        self.revision = revision;
        Ok(UiDelta {
            boot_id: self.boot_id,
            revision,
            change,
        })
    }

    pub const fn heartbeat(&self, sent_at_ms: u64) -> UiEnvelope {
        UiEnvelope::Heartbeat {
            boot_id: self.boot_id,
            revision: self.revision,
            sent_at_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum UiRevisionError {
    #[error("UI state revision counter exhausted")]
    Exhausted,
}

#[derive(Debug, Clone)]
pub struct UiView {
    snapshot: UiSnapshot,
    needs_resync: bool,
}

impl UiView {
    pub fn from_snapshot(snapshot: UiSnapshot) -> Result<Self, UiApplyError> {
        let snapshot = prepare_snapshot(snapshot)?;
        Ok(Self {
            snapshot,
            needs_resync: false,
        })
    }

    pub const fn snapshot(&self) -> &UiSnapshot {
        &self.snapshot
    }

    pub const fn cursor(&self) -> UiCursor {
        self.snapshot.cursor()
    }

    pub const fn needs_resync(&self) -> bool {
        self.needs_resync
    }

    pub fn apply(&mut self, envelope: UiEnvelope) -> Result<(), UiApplyError> {
        if self.needs_resync && !matches!(&envelope, UiEnvelope::Snapshot(_)) {
            return Err(UiApplyError::SnapshotRequired {
                boot_id: self.snapshot.boot_id,
                current_revision: self.snapshot.revision,
            });
        }
        match envelope {
            UiEnvelope::Snapshot(snapshot) => {
                let snapshot = prepare_snapshot(snapshot)?;
                self.snapshot = snapshot;
                self.needs_resync = false;
                Ok(())
            }
            UiEnvelope::Delta(delta) => self.apply_delta(delta),
            UiEnvelope::Heartbeat {
                boot_id, revision, ..
            } => {
                self.validate_boot(boot_id)?;
                if revision != self.snapshot.revision {
                    self.needs_resync = true;
                    return Err(UiApplyError::RevisionGap {
                        expected: self.snapshot.revision,
                        actual: revision,
                    });
                }
                Ok(())
            }
            UiEnvelope::ResyncRequired {
                boot_id,
                current_revision,
                reason,
            } => {
                self.needs_resync = true;
                Err(UiApplyError::ResyncRequired {
                    boot_id,
                    current_revision,
                    reason,
                })
            }
        }
    }

    fn apply_delta(&mut self, delta: UiDelta) -> Result<(), UiApplyError> {
        self.validate_boot(delta.boot_id)?;
        let Some(expected) = self.snapshot.revision.checked_add(1) else {
            self.needs_resync = true;
            return Err(UiApplyError::RevisionExhausted);
        };
        if delta.revision != expected {
            self.needs_resync = true;
            return Err(UiApplyError::RevisionGap {
                expected,
                actual: delta.revision,
            });
        }

        match delta.change {
            UiChange::Status(status) => self.snapshot.status = status,
            UiChange::Capabilities(capabilities) => self.snapshot.capabilities = capabilities,
            UiChange::DeviceUpsert(device) => {
                if let Some(existing) = self
                    .snapshot
                    .devices
                    .iter_mut()
                    .find(|existing| existing.id == device.id)
                {
                    *existing = device;
                } else {
                    self.snapshot.devices.push(device);
                }
            }
            UiChange::DeviceRemove(device_id) => {
                self.snapshot
                    .devices
                    .retain(|device| device.id != device_id);
            }
            UiChange::Topology(layout) => self.snapshot.layout = layout,
            UiChange::DisplayInventory(display_inventory) => {
                self.snapshot.display_inventory = display_inventory;
            }
            UiChange::Pointer(pointer) => self.snapshot.dynamic_state.pointer = Some(pointer),
            UiChange::Gamepads(gamepads) => self.snapshot.dynamic_state.gamepads = gamepads,
            UiChange::KeyButton(transition) => {
                apply_discrete_transition(&mut self.snapshot.dynamic_state, transition);
            }
            UiChange::Session(active_sessions) => {
                if let Err(error) = validate_unique_media_sessions(&active_sessions.media_sessions)
                {
                    self.needs_resync = true;
                    return Err(error);
                }
                self.snapshot.active_sessions = active_sessions;
            }
            UiChange::Diagnostics(diagnostics) => {
                self.snapshot.dynamic_state.diagnostics = diagnostics;
            }
            UiChange::MediaSessionUpsert(media_session) => {
                if let Some(existing) = self
                    .snapshot
                    .active_sessions
                    .media_sessions
                    .iter_mut()
                    .find(|existing| existing.session_id == media_session.session_id)
                {
                    *existing = media_session;
                } else {
                    self.snapshot
                        .active_sessions
                        .media_sessions
                        .push(media_session);
                }
            }
            UiChange::MediaSessionRemove(session_id) => {
                self.snapshot
                    .active_sessions
                    .media_sessions
                    .retain(|session| session.session_id != session_id);
            }
        }
        self.snapshot.revision = delta.revision;
        Ok(())
    }

    fn validate_boot(&mut self, actual: Uuid) -> Result<(), UiApplyError> {
        let expected = self.snapshot.boot_id;
        if actual != expected {
            self.needs_resync = true;
            return Err(UiApplyError::BootMismatch { expected, actual });
        }
        Ok(())
    }
}

fn prepare_snapshot(mut snapshot: UiSnapshot) -> Result<UiSnapshot, UiApplyError> {
    validate_protocol(snapshot.protocol_version)?;
    validate_unique_identities(&snapshot)?;
    canonicalize_pressed_truth(&mut snapshot.dynamic_state);
    Ok(snapshot)
}

fn validate_unique_identities(snapshot: &UiSnapshot) -> Result<(), UiApplyError> {
    let mut devices = HashSet::with_capacity(snapshot.devices.len());
    for device in &snapshot.devices {
        if !devices.insert(device.id) {
            return Err(UiApplyError::DuplicateDeviceId {
                device_id: device.id,
            });
        }
    }

    validate_unique_media_sessions(&snapshot.active_sessions.media_sessions)
}

fn validate_unique_media_sessions(sessions: &[UiMediaSession]) -> Result<(), UiApplyError> {
    let mut media_sessions = HashSet::with_capacity(sessions.len());
    for session in sessions {
        if !media_sessions.insert(session.session_id) {
            return Err(UiApplyError::DuplicateMediaSessionId {
                session_id: session.session_id,
            });
        }
    }
    Ok(())
}

fn canonicalize_pressed_truth(dynamic: &mut UiDynamicState) {
    dynamic.pressed_keys.sort_unstable();
    dynamic.pressed_keys.dedup();
    dynamic
        .pressed_mouse_buttons
        .sort_by_key(|button| mouse_button_sort_key(*button));
    dynamic.pressed_mouse_buttons.dedup();
    dynamic
        .pressed_gamepad_buttons
        .sort_by_key(gamepad_button_sort_key);
    dynamic.pressed_gamepad_buttons.dedup();
}

fn apply_discrete_transition(dynamic: &mut UiDynamicState, transition: UiDiscreteInputState) {
    match transition {
        UiDiscreteInputState::Key {
            key_code, state, ..
        } => {
            set_membership(
                &mut dynamic.pressed_keys,
                key_code,
                state == KeyState::Pressed,
            );
            dynamic.pressed_keys.sort_unstable();
        }
        UiDiscreteInputState::MouseButton { button, state, .. } => {
            set_membership(
                &mut dynamic.pressed_mouse_buttons,
                button,
                state == ButtonState::Pressed,
            );
            dynamic
                .pressed_mouse_buttons
                .sort_by_key(|button| mouse_button_sort_key(*button));
        }
        UiDiscreteInputState::GamepadButton {
            gamepad_id,
            button,
            state,
            ..
        } => {
            set_membership(
                &mut dynamic.pressed_gamepad_buttons,
                UiPressedGamepadButton { gamepad_id, button },
                state == ButtonState::Pressed,
            );
            dynamic
                .pressed_gamepad_buttons
                .sort_by_key(gamepad_button_sort_key);
        }
        UiDiscreteInputState::Wheel { .. } => {}
    }
}

fn set_membership<T: PartialEq>(values: &mut Vec<T>, value: T, present: bool) {
    if present {
        if !values.contains(&value) {
            values.push(value);
        }
    } else {
        values.retain(|existing| existing != &value);
    }
}

fn mouse_button_sort_key(button: MouseButton) -> (u8, u8) {
    match button {
        MouseButton::Left => (0, 0),
        MouseButton::Middle => (1, 0),
        MouseButton::Right => (2, 0),
        MouseButton::Back => (3, 0),
        MouseButton::Forward => (4, 0),
        MouseButton::Other(value) => (5, value),
    }
}

fn gamepad_button_sort_key(button: &UiPressedGamepadButton) -> (u8, u8, u16) {
    let (kind, value) = match button.button {
        GamepadButton::South => (0, 0),
        GamepadButton::East => (1, 0),
        GamepadButton::West => (2, 0),
        GamepadButton::North => (3, 0),
        GamepadButton::LeftBumper => (4, 0),
        GamepadButton::RightBumper => (5, 0),
        GamepadButton::LeftTrigger => (6, 0),
        GamepadButton::RightTrigger => (7, 0),
        GamepadButton::Select => (8, 0),
        GamepadButton::Start => (9, 0),
        GamepadButton::Guide => (10, 0),
        GamepadButton::LeftStick => (11, 0),
        GamepadButton::RightStick => (12, 0),
        GamepadButton::DPadUp => (13, 0),
        GamepadButton::DPadDown => (14, 0),
        GamepadButton::DPadLeft => (15, 0),
        GamepadButton::DPadRight => (16, 0),
        GamepadButton::Other(value) => (17, value),
    };
    (button.gamepad_id, kind, value)
}

fn validate_protocol(actual: u16) -> Result<(), UiApplyError> {
    if actual == UI_STATE_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(UiApplyError::ProtocolVersion {
            expected: UI_STATE_PROTOCOL_VERSION,
            actual,
        })
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum UiApplyError {
    #[error("UI state protocol mismatch: expected {expected}, received {actual}")]
    ProtocolVersion { expected: u16, actual: u16 },
    #[error("UI state daemon boot changed from {expected} to {actual}; resync required")]
    BootMismatch { expected: Uuid, actual: Uuid },
    #[error("UI state revision gap: expected {expected}, received {actual}; resync required")]
    RevisionGap { expected: u64, actual: u64 },
    #[error("UI state revision counter exhausted; resync required")]
    RevisionExhausted,
    #[error("UI snapshot contains duplicate device identity {device_id}")]
    DuplicateDeviceId { device_id: DeviceId },
    #[error("UI snapshot contains duplicate media session identity {session_id}")]
    DuplicateMediaSessionId { session_id: Uuid },
    #[error("a full UI snapshot is required for boot {boot_id} at revision {current_revision}")]
    SnapshotRequired {
        boot_id: Uuid,
        current_revision: u64,
    },
    #[error(
        "daemon requested UI state resync at boot {boot_id}, revision {current_revision}: {reason:?}"
    )]
    ResyncRequired {
        boot_id: Uuid,
        current_revision: u64,
        reason: UiResyncReason,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CapabilityRegistrySnapshot, ControlSessionState, DaemonDeviceSnapshot,
        LatencyFeedbackSnapshot, LayoutGraph, LocalDisplayState, ServiceStatusSnapshot,
    };
    use uuid::Uuid;

    const BOOT: Uuid = Uuid::from_u128(1);

    fn status() -> ServiceStatusSnapshot {
        ServiceStatusSnapshot::new(
            Uuid::from_u128(2),
            "local".into(),
            "host".into(),
            "127.0.0.1:27435".into(),
            27434,
            42,
        )
    }

    fn capabilities() -> CapabilityRegistrySnapshot {
        CapabilityRegistrySnapshot {
            local_device_id: Uuid::from_u128(2),
            generated_at_ms: 0,
            devices: Vec::new(),
        }
    }

    fn snapshot(boot_id: Uuid, revision: u64) -> UiSnapshot {
        UiSnapshot {
            protocol_version: UI_STATE_PROTOCOL_VERSION,
            boot_id,
            revision,
            status: status(),
            devices: Vec::new(),
            layout: LayoutGraph::new(Uuid::from_u128(2)),
            capabilities: capabilities(),
            display_inventory: LocalDisplayState::default(),
            dynamic_state: UiDynamicState::default(),
            active_sessions: UiActiveSessions::default(),
        }
    }

    fn pointer(x: i32) -> UiPointerState {
        UiPointerState {
            x,
            y: 20,
            display_id: Some("display-1".into()),
            observed_at_ms: 100,
        }
    }

    fn delta(boot_id: Uuid, revision: u64, change: UiChange) -> UiEnvelope {
        UiEnvelope::Delta(UiDelta {
            boot_id,
            revision,
            change,
        })
    }

    #[test]
    fn initial_snapshot_and_sequencer_start_at_revision_zero() {
        let sequencer = UiRevisionSequencer::new(BOOT);
        let snapshot = snapshot(BOOT, sequencer.revision());

        assert_eq!(UI_STATE_PROTOCOL_VERSION, 1);
        assert_eq!(snapshot.cursor(), UiCursor::new(BOOT, 0));
    }

    #[test]
    fn emitted_deltas_increment_revision_exactly_once() {
        let mut sequencer = UiRevisionSequencer::new(BOOT);

        let first = sequencer.emit_delta(UiChange::Pointer(pointer(1))).unwrap();
        let second = sequencer.emit_delta(UiChange::Pointer(pointer(2))).unwrap();

        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 2);
        assert_eq!(sequencer.revision(), 2);
    }

    #[test]
    fn heartbeat_and_overwritten_unemitted_latest_value_do_not_consume_revisions() {
        let mut sequencer = UiRevisionSequencer::new(BOOT);
        let latest = [pointer(1), pointer(2), pointer(3)]
            .into_iter()
            .last()
            .unwrap();

        let heartbeat = sequencer.heartbeat(500);
        let emitted = sequencer.emit_delta(UiChange::Pointer(latest)).unwrap();

        assert_eq!(
            heartbeat,
            UiEnvelope::Heartbeat {
                boot_id: BOOT,
                revision: 0,
                sent_at_ms: 500,
            }
        );
        assert_eq!(emitted.revision, 1);
    }

    #[test]
    fn revision_gap_requires_full_snapshot() {
        let mut view = UiView::from_snapshot(snapshot(BOOT, 7)).unwrap();

        assert_eq!(
            view.apply(delta(BOOT, 9, UiChange::Pointer(pointer(9)))),
            Err(UiApplyError::RevisionGap {
                expected: 8,
                actual: 9,
            })
        );
        assert!(view.needs_resync());
    }

    #[test]
    fn revision_exhaustion_freezes_view_without_mutating_snapshot() {
        let mut initial = snapshot(BOOT, u64::MAX);
        initial.dynamic_state.pointer = Some(pointer(1));
        let mut view = UiView::from_snapshot(initial).unwrap();
        let before = view.snapshot().clone();

        assert_eq!(
            view.apply(delta(BOOT, u64::MAX, UiChange::Pointer(pointer(2)))),
            Err(UiApplyError::RevisionExhausted)
        );
        assert!(view.needs_resync());
        assert_eq!(view.snapshot(), &before);
        assert_eq!(view.cursor(), before.cursor());

        assert_eq!(
            view.apply(UiEnvelope::Heartbeat {
                boot_id: BOOT,
                revision: u64::MAX,
                sent_at_ms: 10,
            }),
            Err(UiApplyError::SnapshotRequired {
                boot_id: BOOT,
                current_revision: u64::MAX,
            })
        );
        assert_eq!(
            view.apply(delta(BOOT, u64::MAX, UiChange::Pointer(pointer(3)))),
            Err(UiApplyError::SnapshotRequired {
                boot_id: BOOT,
                current_revision: u64::MAX,
            })
        );

        view.apply(UiEnvelope::Snapshot(snapshot(BOOT, 0))).unwrap();
        assert!(!view.needs_resync());
        view.apply(delta(BOOT, 1, UiChange::Pointer(pointer(4))))
            .unwrap();
    }

    #[test]
    fn deltas_remain_blocked_after_gap_until_snapshot_arrives() {
        let mut view = UiView::from_snapshot(snapshot(BOOT, 7)).unwrap();
        assert!(view
            .apply(delta(BOOT, 9, UiChange::Pointer(pointer(9))))
            .is_err());

        assert_eq!(
            view.apply(delta(BOOT, 8, UiChange::Pointer(pointer(8)))),
            Err(UiApplyError::SnapshotRequired {
                boot_id: BOOT,
                current_revision: 7,
            })
        );
        assert_eq!(view.snapshot().dynamic_state.pointer, None);

        view.apply(UiEnvelope::Snapshot(snapshot(BOOT, 9))).unwrap();
        view.apply(delta(BOOT, 10, UiChange::Pointer(pointer(10))))
            .unwrap();
        assert_eq!(view.snapshot().dynamic_state.pointer, Some(pointer(10)));
    }

    #[test]
    fn boot_mismatch_requires_full_snapshot() {
        let mut view = UiView::from_snapshot(snapshot(BOOT, 7)).unwrap();
        let actual = Uuid::from_u128(99);

        assert_eq!(
            view.apply(delta(actual, 8, UiChange::Pointer(pointer(9)))),
            Err(UiApplyError::BootMismatch {
                expected: BOOT,
                actual,
            })
        );
        assert!(view.needs_resync());
    }

    #[test]
    fn explicit_resync_envelope_is_a_typed_error() {
        let mut view = UiView::from_snapshot(snapshot(BOOT, 7)).unwrap();

        assert_eq!(
            view.apply(UiEnvelope::ResyncRequired {
                boot_id: BOOT,
                current_revision: 12,
                reason: UiResyncReason::HistoryExpired,
            }),
            Err(UiApplyError::ResyncRequired {
                boot_id: BOOT,
                current_revision: 12,
                reason: UiResyncReason::HistoryExpired,
            })
        );
    }

    #[test]
    fn pointer_only_delta_does_not_alter_topology() {
        let mut initial = snapshot(BOOT, 0);
        initial.layout.version = 17;
        let topology = initial.layout.clone();
        let mut view = UiView::from_snapshot(initial).unwrap();

        view.apply(delta(BOOT, 1, UiChange::Pointer(pointer(55))))
            .unwrap();

        assert_eq!(view.snapshot().layout, topology);
        assert_eq!(view.snapshot().dynamic_state.pointer, Some(pointer(55)));
    }

    #[test]
    fn ordinary_pointer_delta_serializes_below_one_kibibyte() {
        let envelope = delta(BOOT, 1, UiChange::Pointer(pointer(55)));

        let json = serde_json::to_vec(&envelope).unwrap();

        assert!(json.len() < 1024, "pointer delta was {} bytes", json.len());
    }

    #[test]
    fn media_sessions_default_when_decoding_older_snapshot() {
        let snapshot = snapshot(BOOT, 0);
        let mut value = serde_json::to_value(snapshot).unwrap();
        value["active_sessions"]
            .as_object_mut()
            .unwrap()
            .remove("media_sessions");

        let decoded: UiSnapshot = serde_json::from_value(value).unwrap();

        assert!(decoded.active_sessions.media_sessions.is_empty());
    }

    #[test]
    fn typed_change_contract_covers_all_ui_state_slices() {
        let device_id = Uuid::from_u128(3);
        let device = DaemonDeviceSnapshot {
            id: device_id,
            name: "peer".into(),
            hostname: "peer-host".into(),
            addresses: vec!["127.0.0.1".into()],
            connected: true,
            last_seen_secs: Some(0),
        };
        let changes = vec![
            UiChange::Status(status()),
            UiChange::Capabilities(capabilities()),
            UiChange::DeviceUpsert(device),
            UiChange::DeviceRemove(device_id),
            UiChange::Topology(LayoutGraph::new(Uuid::from_u128(2))),
            UiChange::DisplayInventory(LocalDisplayState::default()),
            UiChange::Pointer(pointer(1)),
            UiChange::Gamepads(Vec::new()),
            UiChange::KeyButton(UiDiscreteInputState::Key {
                key_code: 30,
                state: crate::KeyState::Pressed,
                observed_at_ms: 1,
            }),
            UiChange::Session(UiActiveSessions {
                control: Some(ControlSessionState::LocalReady),
                media_sessions: Vec::new(),
            }),
            UiChange::Diagnostics(LatencyFeedbackSnapshot::default()),
            UiChange::MediaSessionUpsert(UiMediaSession {
                session_id: Uuid::from_u128(10),
                peer_id: device_id,
                display_id: Some("display-1".into()),
                state: UiMediaSessionState::Streaming,
            }),
            UiChange::MediaSessionRemove(Uuid::from_u128(10)),
        ];

        for change in changes {
            serde_json::to_vec(&change).unwrap();
        }
    }

    #[test]
    fn key_and_button_deltas_keep_stable_idempotent_pressed_truth() {
        let mut view = UiView::from_snapshot(snapshot(BOOT, 0)).unwrap();
        let press_key = || {
            UiChange::KeyButton(UiDiscreteInputState::Key {
                key_code: 30,
                state: crate::KeyState::Pressed,
                observed_at_ms: 1,
            })
        };

        view.apply(delta(BOOT, 1, press_key())).unwrap();
        view.apply(delta(BOOT, 2, press_key())).unwrap();
        view.apply(delta(
            BOOT,
            3,
            UiChange::KeyButton(UiDiscreteInputState::Key {
                key_code: 10,
                state: crate::KeyState::Pressed,
                observed_at_ms: 2,
            }),
        ))
        .unwrap();
        view.apply(delta(
            BOOT,
            4,
            UiChange::KeyButton(UiDiscreteInputState::MouseButton {
                button: crate::MouseButton::Right,
                state: crate::ButtonState::Pressed,
                observed_at_ms: 3,
            }),
        ))
        .unwrap();

        assert_eq!(view.snapshot().dynamic_state.pressed_keys, vec![10, 30]);
        assert_eq!(
            view.snapshot().dynamic_state.pressed_mouse_buttons,
            vec![crate::MouseButton::Right]
        );

        view.apply(delta(
            BOOT,
            5,
            UiChange::KeyButton(UiDiscreteInputState::Key {
                key_code: 30,
                state: crate::KeyState::Released,
                observed_at_ms: 4,
            }),
        ))
        .unwrap();
        assert_eq!(view.snapshot().dynamic_state.pressed_keys, vec![10]);
    }

    #[test]
    fn pressed_truth_defaults_when_decoding_older_dynamic_state() {
        let snapshot = snapshot(BOOT, 0);
        let mut value = serde_json::to_value(snapshot).unwrap();
        let dynamic = value["dynamic_state"].as_object_mut().unwrap();
        dynamic.remove("pressed_keys");
        dynamic.remove("pressed_mouse_buttons");
        dynamic.remove("pressed_gamepad_buttons");

        let decoded: UiSnapshot = serde_json::from_value(value).unwrap();

        assert!(decoded.dynamic_state.pressed_keys.is_empty());
        assert!(decoded.dynamic_state.pressed_mouse_buttons.is_empty());
        assert!(decoded.dynamic_state.pressed_gamepad_buttons.is_empty());
    }

    #[test]
    fn accepted_snapshot_canonicalizes_pressed_truth() {
        let gamepad_a = UiPressedGamepadButton {
            gamepad_id: 2,
            button: crate::GamepadButton::East,
        };
        let gamepad_b = UiPressedGamepadButton {
            gamepad_id: 1,
            button: crate::GamepadButton::South,
        };
        let mut incoming = snapshot(BOOT, 4);
        incoming.dynamic_state.pressed_keys = vec![30, 10, 30];
        incoming.dynamic_state.pressed_mouse_buttons = vec![
            crate::MouseButton::Right,
            crate::MouseButton::Left,
            crate::MouseButton::Right,
        ];
        incoming.dynamic_state.pressed_gamepad_buttons =
            vec![gamepad_a.clone(), gamepad_b.clone(), gamepad_a.clone()];

        let view = UiView::from_snapshot(incoming).unwrap();

        assert_eq!(view.snapshot().dynamic_state.pressed_keys, vec![10, 30]);
        assert_eq!(
            view.snapshot().dynamic_state.pressed_mouse_buttons,
            vec![crate::MouseButton::Left, crate::MouseButton::Right]
        );
        assert_eq!(
            view.snapshot().dynamic_state.pressed_gamepad_buttons,
            vec![gamepad_b, gamepad_a]
        );
    }

    #[test]
    fn snapshots_with_duplicate_device_identity_are_rejected_without_replacement() {
        let duplicate_id = Uuid::from_u128(50);
        let device = DaemonDeviceSnapshot {
            id: duplicate_id,
            name: "peer".into(),
            hostname: "peer-host".into(),
            addresses: Vec::new(),
            connected: true,
            last_seen_secs: None,
        };
        let mut duplicate = snapshot(BOOT, 3);
        duplicate.devices = vec![device.clone(), device];

        assert_eq!(
            UiView::from_snapshot(duplicate.clone()).unwrap_err(),
            UiApplyError::DuplicateDeviceId {
                device_id: duplicate_id,
            }
        );

        let mut view = UiView::from_snapshot(snapshot(BOOT, 1)).unwrap();
        let before = view.snapshot().clone();
        assert_eq!(
            view.apply(UiEnvelope::Snapshot(duplicate)),
            Err(UiApplyError::DuplicateDeviceId {
                device_id: duplicate_id,
            })
        );
        assert_eq!(view.snapshot(), &before);
    }

    #[test]
    fn duplicate_media_identity_is_rejected_and_cannot_unfreeze_resync() {
        let session_id = Uuid::from_u128(60);
        let media = UiMediaSession {
            session_id,
            peer_id: Uuid::from_u128(61),
            display_id: Some("display-1".into()),
            state: UiMediaSessionState::Streaming,
        };
        let mut duplicate = snapshot(BOOT, 5);
        duplicate.active_sessions.media_sessions = vec![media.clone(), media];

        assert_eq!(
            UiView::from_snapshot(duplicate.clone()).unwrap_err(),
            UiApplyError::DuplicateMediaSessionId { session_id }
        );

        let mut view = UiView::from_snapshot(snapshot(BOOT, 1)).unwrap();
        assert!(view
            .apply(delta(BOOT, 3, UiChange::Pointer(pointer(3))))
            .is_err());
        let before = view.snapshot().clone();
        assert_eq!(
            view.apply(UiEnvelope::Snapshot(duplicate)),
            Err(UiApplyError::DuplicateMediaSessionId { session_id })
        );
        assert!(view.needs_resync());
        assert_eq!(view.snapshot(), &before);
        assert_eq!(
            view.apply(delta(BOOT, 2, UiChange::Pointer(pointer(2)))),
            Err(UiApplyError::SnapshotRequired {
                boot_id: BOOT,
                current_revision: 1,
            })
        );
    }

    #[test]
    fn session_delta_with_duplicate_media_identity_is_rejected_atomically() {
        let session_id = Uuid::from_u128(70);
        let media = UiMediaSession {
            session_id,
            peer_id: Uuid::from_u128(71),
            display_id: Some("display-1".into()),
            state: UiMediaSessionState::Streaming,
        };
        let mut view = UiView::from_snapshot(snapshot(BOOT, 0)).unwrap();
        let before = view.snapshot().clone();

        assert_eq!(
            view.apply(delta(
                BOOT,
                1,
                UiChange::Session(UiActiveSessions {
                    control: Some(ControlSessionState::LocalReady),
                    media_sessions: vec![media.clone(), media],
                }),
            )),
            Err(UiApplyError::DuplicateMediaSessionId { session_id })
        );
        assert!(view.needs_resync());
        assert_eq!(view.snapshot(), &before);
        assert_eq!(view.cursor(), UiCursor::new(BOOT, 0));
        assert_eq!(
            view.apply(delta(BOOT, 1, UiChange::Pointer(pointer(1)))),
            Err(UiApplyError::SnapshotRequired {
                boot_id: BOOT,
                current_revision: 0,
            })
        );
    }

    #[test]
    fn same_peer_media_sessions_are_distinguished_by_session_and_display() {
        let peer_id = Uuid::from_u128(20);
        let first_id = Uuid::from_u128(21);
        let second_id = Uuid::from_u128(22);
        let mut view = UiView::from_snapshot(snapshot(BOOT, 0)).unwrap();

        for (revision, session_id, display_id) in
            [(1, first_id, "display-1"), (2, second_id, "display-2")]
        {
            view.apply(delta(
                BOOT,
                revision,
                UiChange::MediaSessionUpsert(UiMediaSession {
                    session_id,
                    peer_id,
                    display_id: Some(display_id.into()),
                    state: UiMediaSessionState::Streaming,
                }),
            ))
            .unwrap();
        }

        assert_eq!(view.snapshot().active_sessions.media_sessions.len(), 2);
        assert_eq!(
            view.snapshot().active_sessions.media_sessions[0].session_id,
            first_id
        );
        assert_eq!(
            view.snapshot().active_sessions.media_sessions[1].display_id,
            Some("display-2".into())
        );

        view.apply(delta(BOOT, 3, UiChange::MediaSessionRemove(first_id)))
            .unwrap();
        assert_eq!(
            view.snapshot().active_sessions.media_sessions[0].session_id,
            second_id
        );
    }
}
