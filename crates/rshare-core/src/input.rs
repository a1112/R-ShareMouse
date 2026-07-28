use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    perf::MonotonicStamp, ButtonState, ControlConnectionId, DeviceId, GamepadButton,
    GamepadDeviceInfo, KeyState, MouseButton,
};

pub const INPUT_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SessionEpoch(pub u64);

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum SessionEpochError {
    #[error("the session epoch is exhausted")]
    Exhausted,
}

impl SessionEpoch {
    pub fn next(self) -> Result<Self, SessionEpochError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(SessionEpochError::Exhausted)
    }

    pub fn advance(&mut self) -> Result<Self, SessionEpochError> {
        let next = self.next()?;
        *self = next;
        Ok(next)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RealtimeInputFrame {
    pub protocol_version: u16,
    pub session_epoch: SessionEpoch,
    pub sequence: u64,
    pub captured_at: MonotonicStamp,
    pub payload: RealtimeInputPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RealtimeInputPayload {
    RelativeMouse {
        dx: i32,
        dy: i32,
    },
    AbsoluteAnchor {
        x: i32,
        y: i32,
    },
    GamepadAxes {
        gamepad_id: u8,
        left_stick_x: i16,
        left_stick_y: i16,
        right_stick_x: i16,
        right_stick_y: i16,
        left_trigger: u16,
        right_trigger: u16,
    },
    CursorVisual {
        x: i32,
        y: i32,
        visible: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReliableInputFrame {
    pub protocol_version: u16,
    pub session_epoch: SessionEpoch,
    pub sequence: u64,
    pub captured_at: MonotonicStamp,
    pub event: ReliableInputEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReliableInputEvent {
    Enter {
        target_display_id: String,
        x: i32,
        y: i32,
    },
    Leave,
    ReleaseAll {
        reason: ReleaseAllReason,
    },
    Key {
        keycode: u32,
        state: KeyState,
    },
    TextCommit {
        text: String,
    },
    MouseButton {
        button: MouseButton,
        state: ButtonState,
        x: i32,
        y: i32,
        realtime_anchor_sequence: u64,
    },
    Wheel {
        delta_x: i32,
        delta_y: i32,
    },
    GamepadConnected {
        info: GamepadDeviceInfo,
    },
    GamepadDisconnected {
        gamepad_id: u8,
    },
    GamepadButton {
        gamepad_id: u8,
        button: GamepadButton,
        pressed: bool,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReleaseAllReason {
    SessionEnded,
    OwnershipTransfer,
    Suspended,
    BackendFailure,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthenticatedInputOwner {
    pub peer_id: DeviceId,
    pub control_connection_id: ControlConnectionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptRealtime {
    Accepted,
    AcceptedWithGap(u64),
    OutOfOrder,
    WrongOwnerOrEpoch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptReliable {
    Accepted,
    AcceptedWithGap(u64),
    OutOfOrder,
    WrongOwnerOrEpoch,
}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum TransferError {
    #[error("session epoch {proposed:?} must be newer than {current:?}")]
    EpochNotIncreasing {
        current: SessionEpoch,
        proposed: SessionEpoch,
    },
}

/// The replay window has single ownership and cannot be cloned.
///
/// ```compile_fail
/// use rshare_core::{
///     AuthenticatedInputOwner, ControlConnectionId, DeviceId, InputOwnershipGate, SessionEpoch,
/// };
///
/// let owner = AuthenticatedInputOwner {
///     peer_id: DeviceId::nil(),
///     control_connection_id: ControlConnectionId::new(),
/// };
/// let gate = InputOwnershipGate::new(owner, SessionEpoch(1));
/// let forked_replay_window = gate.clone();
/// ```
#[derive(Debug)]
pub struct InputOwnershipGate {
    owner: AuthenticatedInputOwner,
    epoch: SessionEpoch,
    last_realtime_sequence: Option<u64>,
    last_reliable_sequence: Option<u64>,
}

impl InputOwnershipGate {
    pub fn new(owner: AuthenticatedInputOwner, epoch: SessionEpoch) -> Self {
        Self {
            owner,
            epoch,
            last_realtime_sequence: None,
            last_reliable_sequence: None,
        }
    }

    pub fn owner(&self) -> AuthenticatedInputOwner {
        self.owner
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn transfer(
        &mut self,
        owner: AuthenticatedInputOwner,
        epoch: SessionEpoch,
    ) -> Result<(), TransferError> {
        if epoch.0 <= self.epoch.0 {
            return Err(TransferError::EpochNotIncreasing {
                current: self.epoch,
                proposed: epoch,
            });
        }

        self.owner = owner;
        self.epoch = epoch;
        self.last_realtime_sequence = None;
        self.last_reliable_sequence = None;
        Ok(())
    }

    pub fn accept_realtime(
        &mut self,
        owner: AuthenticatedInputOwner,
        epoch: SessionEpoch,
        sequence: u64,
    ) -> AcceptRealtime {
        if !self.is_current(owner, epoch) {
            return AcceptRealtime::WrongOwnerOrEpoch;
        }

        match accept_sequence(&mut self.last_realtime_sequence, sequence) {
            SequenceAcceptance::Accepted => AcceptRealtime::Accepted,
            SequenceAcceptance::AcceptedWithGap(gap) => AcceptRealtime::AcceptedWithGap(gap),
            SequenceAcceptance::OutOfOrder => AcceptRealtime::OutOfOrder,
        }
    }

    pub fn accept_reliable(
        &mut self,
        owner: AuthenticatedInputOwner,
        epoch: SessionEpoch,
        sequence: u64,
    ) -> AcceptReliable {
        if !self.is_current(owner, epoch) {
            return AcceptReliable::WrongOwnerOrEpoch;
        }

        match accept_sequence(&mut self.last_reliable_sequence, sequence) {
            SequenceAcceptance::Accepted => AcceptReliable::Accepted,
            SequenceAcceptance::AcceptedWithGap(gap) => AcceptReliable::AcceptedWithGap(gap),
            SequenceAcceptance::OutOfOrder => AcceptReliable::OutOfOrder,
        }
    }

    fn is_current(&self, owner: AuthenticatedInputOwner, epoch: SessionEpoch) -> bool {
        self.owner == owner && self.epoch == epoch
    }
}

enum SequenceAcceptance {
    Accepted,
    AcceptedWithGap(u64),
    OutOfOrder,
}

fn accept_sequence(last: &mut Option<u64>, sequence: u64) -> SequenceAcceptance {
    let Some(previous) = *last else {
        *last = Some(sequence);
        return SequenceAcceptance::Accepted;
    };

    if sequence <= previous {
        return SequenceAcceptance::OutOfOrder;
    }

    *last = Some(sequence);
    let gap = sequence - previous - 1;
    if gap == 0 {
        SequenceAcceptance::Accepted
    } else {
        SequenceAcceptance::AcceptedWithGap(gap)
    }
}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum PressedStateLedgerError {
    #[error("the pressed-state generation is exhausted")]
    GenerationExhausted,
    #[error("the release-batch token is exhausted")]
    BatchTokenExhausted,
}

#[derive(Debug, Clone)]
pub struct PendingReleaseBatch {
    ledger_id: Uuid,
    token: u64,
    reason: ReleaseAllReason,
    events: Vec<ReliableInputEvent>,
    key_generations: Vec<(u32, u64)>,
    mouse_generations: Vec<(MouseButtonKey, u64)>,
}

impl PendingReleaseBatch {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn reason(&self) -> ReleaseAllReason {
        self.reason
    }

    pub fn events(&self) -> &[ReliableInputEvent] {
        &self.events
    }
}

/// A held-state ledger has one identity and cannot be cloned.
///
/// ```compile_fail
/// use rshare_core::PressedStateLedger;
///
/// let ledger = PressedStateLedger::new();
/// let forked_ledger_identity = ledger.clone();
/// ```
#[derive(Debug)]
pub struct PressedStateLedger {
    ledger_id: Uuid,
    next_generation: u64,
    next_batch_token: u64,
    keys: BTreeMap<u32, u64>,
    mouse_buttons: BTreeMap<MouseButtonKey, HeldMouseButton>,
}

impl Default for PressedStateLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl PressedStateLedger {
    pub fn new() -> Self {
        Self {
            ledger_id: Uuid::new_v4(),
            next_generation: 1,
            next_batch_token: 1,
            keys: BTreeMap::new(),
            mouse_buttons: BTreeMap::new(),
        }
    }

    pub fn record_key(
        &mut self,
        keycode: u32,
        state: KeyState,
    ) -> Result<bool, PressedStateLedgerError> {
        match state {
            KeyState::Pressed if self.keys.contains_key(&keycode) => Ok(false),
            KeyState::Pressed => {
                let generation = self.take_generation()?;
                self.keys.insert(keycode, generation);
                Ok(true)
            }
            KeyState::Released => Ok(self.keys.remove(&keycode).is_some()),
        }
    }

    pub fn record_mouse_button(
        &mut self,
        button: MouseButton,
        state: ButtonState,
        x: i32,
        y: i32,
        realtime_anchor_sequence: u64,
    ) -> Result<bool, PressedStateLedgerError> {
        let key = MouseButtonKey::from(button);
        match state {
            ButtonState::Pressed if self.mouse_buttons.contains_key(&key) => Ok(false),
            ButtonState::Pressed => {
                let generation = self.take_generation()?;
                self.mouse_buttons.insert(
                    key,
                    HeldMouseButton {
                        button,
                        x,
                        y,
                        realtime_anchor_sequence,
                        generation,
                    },
                );
                Ok(true)
            }
            ButtonState::Released => Ok(self.mouse_buttons.remove(&key).is_some()),
        }
    }

    /// Snapshots deterministic release events without changing held state.
    ///
    /// Input recording remains enabled while a batch is pending. If a held
    /// control is released and pressed again, its new generation is protected
    /// from confirmation of the older batch.
    pub fn release_all_events(
        &mut self,
        reason: ReleaseAllReason,
    ) -> Result<PendingReleaseBatch, PressedStateLedgerError> {
        let token = self.take_batch_token()?;
        let mut events = Vec::with_capacity(self.keys.len() + self.mouse_buttons.len());
        let mut key_generations = Vec::with_capacity(self.keys.len());
        let mut mouse_generations = Vec::with_capacity(self.mouse_buttons.len());

        for (&keycode, &generation) in &self.keys {
            events.push(ReliableInputEvent::Key {
                keycode,
                state: KeyState::Released,
            });
            key_generations.push((keycode, generation));
        }

        for (&key, held) in &self.mouse_buttons {
            events.push(ReliableInputEvent::MouseButton {
                button: held.button,
                state: ButtonState::Released,
                x: held.x,
                y: held.y,
                realtime_anchor_sequence: held.realtime_anchor_sequence,
            });
            mouse_generations.push((key, held.generation));
        }

        Ok(PendingReleaseBatch {
            ledger_id: self.ledger_id,
            token,
            reason,
            events,
            key_generations,
            mouse_generations,
        })
    }

    /// Clears only controls from `batch` whose releases were all injected.
    ///
    /// The caller must invoke this only after successfully injecting every
    /// event in the batch. Dropping a failed or partial batch preserves state
    /// for a later fail-safe retry.
    pub fn confirm_release_all(&mut self, batch: &PendingReleaseBatch) -> usize {
        if batch.ledger_id != self.ledger_id {
            return 0;
        }

        let mut cleared = 0;
        for &(keycode, generation) in &batch.key_generations {
            if self.keys.get(&keycode) == Some(&generation) {
                self.keys.remove(&keycode);
                cleared += 1;
            }
        }
        for &(key, generation) in &batch.mouse_generations {
            if self
                .mouse_buttons
                .get(&key)
                .is_some_and(|held| held.generation == generation)
            {
                self.mouse_buttons.remove(&key);
                cleared += 1;
            }
        }
        cleared
    }

    pub fn held_key_count(&self) -> usize {
        self.keys.len()
    }

    pub fn held_mouse_button_count(&self) -> usize {
        self.mouse_buttons.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.mouse_buttons.is_empty()
    }

    fn take_generation(&mut self) -> Result<u64, PressedStateLedgerError> {
        let generation = self.next_generation;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(PressedStateLedgerError::GenerationExhausted)?;
        Ok(generation)
    }

    fn take_batch_token(&mut self) -> Result<u64, PressedStateLedgerError> {
        let token = self.next_batch_token;
        self.next_batch_token = self
            .next_batch_token
            .checked_add(1)
            .ok_or(PressedStateLedgerError::BatchTokenExhausted)?;
        Ok(token)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MouseButtonKey(u8, u8);

impl From<MouseButton> for MouseButtonKey {
    fn from(button: MouseButton) -> Self {
        let variant = match button {
            MouseButton::Left
            | MouseButton::Middle
            | MouseButton::Right
            | MouseButton::Back
            | MouseButton::Forward => 0,
            MouseButton::Other(_) => 1,
        };
        Self(button.to_code(), variant)
    }
}

#[derive(Debug, Clone)]
struct HeldMouseButton {
    button: MouseButton,
    x: i32,
    y: i32,
    realtime_anchor_sequence: u64,
    generation: u64,
}
