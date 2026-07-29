use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use rshare_core::{
    ButtonState, ControlSessionState, GamepadButton, LocalGamepadState, MouseButton, SessionEpoch,
    UiDiscreteInputState, UiPressedGamepadButton,
};
use tokio::sync::{mpsc, watch, Notify};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputUiMutation {
    KeyButton {
        generation: u64,
        projection: InputDiscreteProjection,
    },
    GamepadButtons {
        generation: u64,
        transitions: Vec<UiDiscreteInputState>,
    },
    Session {
        generation: u64,
        session: ControlSessionState,
    },
}

#[derive(Clone)]
pub struct InputStatePublisher {
    authoritative: watch::Sender<Arc<InputReliableUiProjection>>,
    reliable_tx: mpsc::Sender<InputUiMutation>,
    pointer_tx: watch::Sender<Option<InputPointerProjection>>,
    gamepads_tx: watch::Sender<Vec<LocalGamepadState>>,
    dirty: Arc<DirtyProjectionNotifier>,
    generation: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPointerProjection {
    pub session_epoch: SessionEpoch,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDiscreteProjection {
    pub session_epoch: SessionEpoch,
    pub pressed_keys: Vec<u32>,
    pub pressed_buttons: Vec<MouseButton>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputReliableUiProjection {
    pub generation: u64,
    pub discrete: InputDiscreteProjection,
    pub pressed_gamepad_buttons: Vec<UiPressedGamepadButton>,
    /// Canonical gamepad state shares this Arc with pressed-button truth so a
    /// rebuild cannot observe values from different watch-channel generations.
    pub gamepads: Vec<LocalGamepadState>,
    pub session: ControlSessionState,
}

#[derive(Default)]
pub struct DirtyProjectionNotifier {
    dirty: AtomicBool,
    wake: Notify,
}

impl DirtyProjectionNotifier {
    pub fn mark(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            self.wake.notify_waiters();
        }
    }

    pub fn take(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    pub async fn notified(&self) {
        if self.is_dirty() {
            return;
        }
        let notified = self.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_dirty() {
            return;
        }
        notified.await;
    }
}

pub struct InputStateFeeds {
    pub reliable_rx: mpsc::Receiver<InputUiMutation>,
    pub authoritative_rx: watch::Receiver<Arc<InputReliableUiProjection>>,
    pub pointer_rx: watch::Receiver<Option<InputPointerProjection>>,
    pub gamepads_rx: watch::Receiver<Vec<LocalGamepadState>>,
    pub dirty: Arc<DirtyProjectionNotifier>,
}

pub fn input_state_channel(capacity: usize) -> (InputStatePublisher, InputStateFeeds) {
    assert!(capacity > 0, "input state delta capacity must be non-zero");
    let initial = Arc::new(InputReliableUiProjection {
        generation: 0,
        discrete: InputDiscreteProjection {
            session_epoch: SessionEpoch(0),
            pressed_keys: Vec::new(),
            pressed_buttons: Vec::new(),
        },
        pressed_gamepad_buttons: Vec::new(),
        gamepads: Vec::new(),
        session: ControlSessionState::LocalReady,
    });
    let (authoritative, authoritative_rx) = watch::channel(initial);
    let (reliable_tx, reliable_rx) = mpsc::channel(capacity);
    let (pointer_tx, pointer_rx) = watch::channel(None);
    let (gamepads_tx, gamepads_rx) = watch::channel(Vec::new());
    let dirty = Arc::new(DirtyProjectionNotifier::default());
    let generation = Arc::new(AtomicU64::new(0));
    (
        InputStatePublisher {
            authoritative,
            reliable_tx,
            pointer_tx,
            gamepads_tx,
            dirty: dirty.clone(),
            generation,
        },
        InputStateFeeds {
            reliable_rx,
            authoritative_rx,
            pointer_rx,
            gamepads_rx,
            dirty,
        },
    )
}

impl InputStatePublisher {
    pub fn publish_pointer(&self, pointer: InputPointerProjection) {
        self.pointer_tx.send_replace(Some(pointer));
    }

    pub fn publish_gamepads(&self, gamepads: Vec<LocalGamepadState>) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let current = self.authoritative.borrow().clone();
        let pressed_gamepad_buttons = pressed_gamepad_buttons(&gamepads);
        self.authoritative
            .send_replace(Arc::new(InputReliableUiProjection {
                generation,
                discrete: current.discrete.clone(),
                pressed_gamepad_buttons: pressed_gamepad_buttons.clone(),
                gamepads: gamepads.clone(),
                session: current.session.clone(),
            }));
        self.gamepads_tx.send_replace(gamepads);

        let observed_at_ms = timestamp_from_gamepads(self.gamepads_tx.borrow().as_slice());
        let transitions = gamepad_button_transitions(
            &current.pressed_gamepad_buttons,
            &pressed_gamepad_buttons,
            observed_at_ms,
        );
        if !transitions.is_empty()
            && self
                .reliable_tx
                .try_send(InputUiMutation::GamepadButtons {
                    generation,
                    transitions,
                })
                .is_err()
        {
            self.dirty.mark();
        }
    }

    pub fn publish_discrete(&self, discrete: InputDiscreteProjection) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let current = self.authoritative.borrow().clone();
        self.authoritative
            .send_replace(Arc::new(InputReliableUiProjection {
                generation,
                discrete: discrete.clone(),
                pressed_gamepad_buttons: current.pressed_gamepad_buttons.clone(),
                gamepads: current.gamepads.clone(),
                session: current.session.clone(),
            }));
        if self
            .reliable_tx
            .try_send(InputUiMutation::KeyButton {
                generation,
                projection: discrete,
            })
            .is_err()
        {
            self.dirty.mark();
        }
    }

    pub fn publish_session(&self, session: ControlSessionState) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let current = self.authoritative.borrow().clone();
        self.authoritative
            .send_replace(Arc::new(InputReliableUiProjection {
                generation,
                discrete: current.discrete.clone(),
                pressed_gamepad_buttons: current.pressed_gamepad_buttons.clone(),
                gamepads: current.gamepads.clone(),
                session: session.clone(),
            }));
        if self
            .reliable_tx
            .try_send(InputUiMutation::Session {
                generation,
                session,
            })
            .is_err()
        {
            self.dirty.mark();
        }
    }
}

fn pressed_gamepad_buttons(gamepads: &[LocalGamepadState]) -> Vec<UiPressedGamepadButton> {
    let mut pressed = gamepads
        .iter()
        .filter(|gamepad| gamepad.connected)
        .flat_map(|gamepad| {
            gamepad
                .buttons
                .iter()
                .filter(|button| button.pressed)
                .map(|button| UiPressedGamepadButton {
                    gamepad_id: gamepad.gamepad_id,
                    button: button.button,
                })
        })
        .collect::<Vec<_>>();
    pressed.sort_by_key(gamepad_button_sort_key);
    pressed.dedup();
    pressed
}

fn gamepad_button_transitions(
    previous: &[UiPressedGamepadButton],
    current: &[UiPressedGamepadButton],
    observed_at_ms: u64,
) -> Vec<UiDiscreteInputState> {
    let mut transitions = previous
        .iter()
        .filter(|button| !current.contains(button))
        .map(|button| UiDiscreteInputState::GamepadButton {
            gamepad_id: button.gamepad_id,
            button: button.button,
            state: ButtonState::Released,
            observed_at_ms,
        })
        .chain(
            current
                .iter()
                .filter(|button| !previous.contains(button))
                .map(|button| UiDiscreteInputState::GamepadButton {
                    gamepad_id: button.gamepad_id,
                    button: button.button,
                    state: ButtonState::Pressed,
                    observed_at_ms,
                }),
        )
        .collect::<Vec<_>>();
    transitions.sort_by_key(|transition| match transition {
        UiDiscreteInputState::GamepadButton {
            gamepad_id,
            button,
            state,
            ..
        } => (
            *gamepad_id,
            gamepad_button_rank(*button),
            u8::from(*state == ButtonState::Pressed),
        ),
        _ => unreachable!("only gamepad transitions are built here"),
    });
    transitions
}

fn gamepad_button_sort_key(button: &UiPressedGamepadButton) -> (u8, u8, u16) {
    let (rank, other) = gamepad_button_rank(button.button);
    (button.gamepad_id, rank, other)
}

fn gamepad_button_rank(button: GamepadButton) -> (u8, u16) {
    match button {
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
    }
}

fn timestamp_from_gamepads(gamepads: &[LocalGamepadState]) -> u64 {
    gamepads
        .iter()
        .map(|gamepad| gamepad.last_seen_ms)
        .max()
        .unwrap_or_default()
}

#[derive(Default)]
pub struct ControlMetrics {
    captured: AtomicU64,
    routed: AtomicU64,
    realtime_replaced: AtomicU64,
    realtime_dropped: AtomicU64,
    reliable_overflow: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlMetricSnapshot {
    pub captured: u64,
    pub routed: u64,
    pub realtime_replaced: u64,
    pub realtime_dropped: u64,
    pub reliable_overflow: u64,
}

impl ControlMetrics {
    pub fn record_captured(&self) {
        self.captured.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_routed(&self) {
        self.routed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_realtime_replaced(&self, count: u64) {
        self.realtime_replaced.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_realtime_dropped(&self, count: u64) {
        self.realtime_dropped.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_reliable_overflow(&self, count: u64) {
        self.reliable_overflow.fetch_add(count, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ControlMetricSnapshot {
        ControlMetricSnapshot {
            captured: self.captured.load(Ordering::Relaxed),
            routed: self.routed.load(Ordering::Relaxed),
            realtime_replaced: self.realtime_replaced.load(Ordering::Relaxed),
            realtime_dropped: self.realtime_dropped.load(Ordering::Relaxed),
            reliable_overflow: self.reliable_overflow.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rshare_core::{GamepadButtonState, GamepadState};

    #[test]
    fn authoritative_gamepad_projection_is_atomic_with_pressed_truth_and_matches_latest_watch() {
        let (publisher, feeds) = input_state_channel(4);
        let mut state = GamepadState::neutral(7, 1, 99);
        state.buttons.push(GamepadButtonState {
            button: GamepadButton::South,
            pressed: true,
        });
        let gamepads = vec![LocalGamepadState::from_state(
            &state,
            Some("Atomic Pad".into()),
            true,
        )];

        publisher.publish_gamepads(gamepads.clone());

        let authoritative = feeds.authoritative_rx.borrow().clone();
        assert_eq!(authoritative.gamepads, gamepads);
        assert_eq!(
            authoritative.gamepads,
            feeds.gamepads_rx.borrow().as_slice()
        );
        assert_eq!(
            authoritative.pressed_gamepad_buttons,
            vec![UiPressedGamepadButton {
                gamepad_id: 7,
                button: GamepadButton::South,
            }]
        );
    }
}
