//! Gamepad capture through gilrs.

use crate::events::{
    GamepadButton, GamepadButtonState, GamepadDeviceInfo, GamepadState, InputEvent,
};
use crate::ingress::{
    CaptureOrigin, CaptureSource, CapturedInputPayload, ContinuousInput, GamepadAxes, PushOutcome,
    SemanticInputProducer,
};
use anyhow::Result;
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Configuration for gamepad capture.
#[derive(Debug, Clone)]
pub struct GamepadListenerConfig {
    /// Whether gamepad capture is enabled.
    pub enabled: bool,
    /// Axis deadzone in basis points. 800 = 8%.
    pub deadzone_basis_points: u16,
    /// Maximum state snapshot rate.
    pub max_update_hz: u16,
}

impl Default for GamepadListenerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            deadzone_basis_points: 800,
            max_update_hz: 120,
        }
    }
}

impl From<&rshare_core::GamepadConfig> for GamepadListenerConfig {
    fn from(config: &rshare_core::GamepadConfig) -> Self {
        Self {
            enabled: config.enabled,
            deadzone_basis_points: config.deadzone_basis_points,
            max_update_hz: config.max_update_hz,
        }
    }
}

/// Background gamepad listener that publishes directly into semantic ingress.
pub struct GilrsGamepadListener {
    config: GamepadListenerConfig,
    producer: SemanticInputProducer,
    origin: CaptureOrigin,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl GilrsGamepadListener {
    pub fn new(
        producer: SemanticInputProducer,
        origin: CaptureOrigin,
        config: GamepadListenerConfig,
    ) -> Self {
        Self {
            config,
            producer,
            origin: CaptureOrigin {
                source: CaptureSource::Gamepad,
                ..origin
            },
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    pub fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let running = self.running.clone();
        let producer = self.producer.clone();
        let origin = self.origin;
        let config = self.config.clone();

        self.thread = Some(
            std::thread::Builder::new()
                .name("rshare-gilrs-gamepad-listener".to_string())
                .spawn(move || run_gamepad_loop(running, producer, origin, config))
                .map_err(|error| anyhow::anyhow!("Failed to spawn gamepad listener: {error}"))?,
        );

        Ok(())
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn publish_for_test(&self, payload: CapturedInputPayload) -> PushOutcome {
        publish_payload(&self.producer, self.origin, payload)
    }
}

impl Drop for GilrsGamepadListener {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_gamepad_loop(
    running: Arc<AtomicBool>,
    producer: SemanticInputProducer,
    origin: CaptureOrigin,
    config: GamepadListenerConfig,
) {
    let mut gilrs = match Gilrs::new() {
        Ok(gilrs) => gilrs,
        Err(error) => {
            running.store(false, Ordering::SeqCst);
            tracing::warn!("Gamepad capture unavailable: {}", error);
            return;
        }
    };

    let mut states = HashMap::new();
    let mut last_sent_at = HashMap::new();
    let mut last_sent_states = HashMap::new();

    for (id, gamepad) in gilrs.gamepads() {
        let Some(gamepad_id) = stable_gamepad_id(id) else {
            continue;
        };

        let info = GamepadDeviceInfo {
            gamepad_id,
            name: gamepad.name().to_string(),
            vendor_id: gamepad.vendor_id(),
            product_id: gamepad.product_id(),
        };
        let _ = publish_event(&producer, origin, InputEvent::gamepad_connected(info));

        let state = GamepadState::neutral(gamepad_id, 0, timestamp_ms());
        states.insert(gamepad_id, state.clone());
        last_sent_states.insert(gamepad_id, state.clone());
        let _ = publish_payload(
            &producer,
            origin,
            CapturedInputPayload::Continuous(ContinuousInput::GamepadAxes(GamepadAxes::from(
                &state,
            ))),
        );
    }

    while running.load(Ordering::SeqCst) {
        let Some(event) = gilrs.next_event_blocking(Some(Duration::from_millis(100))) else {
            continue;
        };

        let Some(gamepad_id) = stable_gamepad_id(event.id) else {
            continue;
        };

        match event.event {
            EventType::Connected => {
                if let Some(info) = gamepad_info(&gilrs, event.id, gamepad_id) {
                    let _ = publish_event(&producer, origin, InputEvent::gamepad_connected(info));
                }

                let state = GamepadState::neutral(gamepad_id, 0, timestamp_ms());
                states.insert(gamepad_id, state.clone());
                last_sent_states.insert(gamepad_id, state.clone());
                let _ = publish_payload(
                    &producer,
                    origin,
                    CapturedInputPayload::Continuous(ContinuousInput::GamepadAxes(
                        GamepadAxes::from(&state),
                    )),
                );
            }
            EventType::Disconnected => {
                states.remove(&gamepad_id);
                last_sent_at.remove(&gamepad_id);
                last_sent_states.remove(&gamepad_id);
                let _ = publish_event(
                    &producer,
                    origin,
                    InputEvent::gamepad_disconnected(gamepad_id),
                );
            }
            EventType::ButtonPressed(button, _)
            | EventType::ButtonRepeated(button, _)
            | EventType::ButtonReleased(button, _) => {
                let pressed = !matches!(event.event, EventType::ButtonReleased(_, _));
                if let Some(state) = states
                    .entry(gamepad_id)
                    .or_insert_with(|| GamepadState::neutral(gamepad_id, 0, timestamp_ms()))
                    .update_button(button, pressed)
                {
                    send_state(
                        &producer,
                        origin,
                        state,
                        &mut last_sent_at,
                        &mut last_sent_states,
                        &config,
                        true,
                    );
                }
            }
            EventType::ButtonChanged(button, value, _) => {
                let pressed = value >= 0.5;
                if let Some(state) = states
                    .entry(gamepad_id)
                    .or_insert_with(|| GamepadState::neutral(gamepad_id, 0, timestamp_ms()))
                    .update_button(button, pressed)
                {
                    send_state(
                        &producer,
                        origin,
                        state,
                        &mut last_sent_at,
                        &mut last_sent_states,
                        &config,
                        true,
                    );
                }
            }
            EventType::AxisChanged(axis, value, _) => {
                if let Some(state) = states
                    .entry(gamepad_id)
                    .or_insert_with(|| GamepadState::neutral(gamepad_id, 0, timestamp_ms()))
                    .update_axis(axis, value, config.deadzone_basis_points)
                {
                    send_state(
                        &producer,
                        origin,
                        state,
                        &mut last_sent_at,
                        &mut last_sent_states,
                        &config,
                        false,
                    );
                }
            }
            EventType::Dropped | EventType::ForceFeedbackEffectCompleted => {}
            _ => {}
        }
    }
}

fn gamepad_info(gilrs: &Gilrs, id: GamepadId, gamepad_id: u8) -> Option<GamepadDeviceInfo> {
    let gamepad = gilrs.connected_gamepad(id)?;
    Some(GamepadDeviceInfo {
        gamepad_id,
        name: gamepad.name().to_string(),
        vendor_id: gamepad.vendor_id(),
        product_id: gamepad.product_id(),
    })
}

fn send_state(
    producer: &SemanticInputProducer,
    origin: CaptureOrigin,
    state: &mut GamepadState,
    last_sent_at: &mut HashMap<u8, Instant>,
    last_sent_states: &mut HashMap<u8, GamepadState>,
    config: &GamepadListenerConfig,
    immediate: bool,
) {
    let now = Instant::now();
    let min_interval = min_update_interval(config.max_update_hz);
    if !immediate {
        if let Some(previous) = last_sent_at.get(&state.gamepad_id) {
            if now.saturating_duration_since(*previous) < min_interval {
                return;
            }
        }
    }

    state.sequence = state.sequence.saturating_add(1);
    state.timestamp_ms = timestamp_ms();
    let previous = last_sent_states
        .get(&state.gamepad_id)
        .cloned()
        .unwrap_or_else(|| GamepadState::neutral(state.gamepad_id, 0, state.timestamp_ms));
    for payload in adapt_gamepad_snapshots(&previous, state) {
        let _ = publish_payload(producer, origin, payload);
    }
    last_sent_at.insert(state.gamepad_id, now);
    last_sent_states.insert(state.gamepad_id, state.clone());
}

fn publish_event(
    producer: &SemanticInputProducer,
    origin: CaptureOrigin,
    event: InputEvent,
) -> PushOutcome {
    publish_payload(
        producer,
        origin,
        CapturedInputPayload::from_input_event(event),
    )
}

fn publish_payload(
    producer: &SemanticInputProducer,
    origin: CaptureOrigin,
    payload: CapturedInputPayload,
) -> PushOutcome {
    producer.try_push(producer.capture(origin, payload))
}

/// Split full gamepad snapshots into ordered reliable button transitions and
/// one replaceable axes snapshot.
pub fn adapt_gamepad_snapshots(
    previous: &GamepadState,
    current: &GamepadState,
) -> Vec<CapturedInputPayload> {
    let mut outputs = Vec::new();
    for button in ordered_gamepad_buttons(previous, current) {
        let before = button_pressed(previous, button);
        let after = button_pressed(current, button);
        if before != after {
            outputs.push(CapturedInputPayload::Discrete(InputEvent::gamepad_button(
                current.gamepad_id,
                button,
                after,
                current.clone(),
            )));
        }
    }

    if axes_changed(previous, current) {
        outputs.push(CapturedInputPayload::Continuous(
            ContinuousInput::GamepadAxes(GamepadAxes::from(current)),
        ));
    }
    outputs
}

fn button_pressed(state: &GamepadState, button: GamepadButton) -> bool {
    state
        .buttons
        .iter()
        .find(|candidate| candidate.button == button)
        .is_some_and(|candidate| candidate.pressed)
}

fn axes_changed(previous: &GamepadState, current: &GamepadState) -> bool {
    previous.gamepad_id != current.gamepad_id
        || previous.left_stick_x != current.left_stick_x
        || previous.left_stick_y != current.left_stick_y
        || previous.right_stick_x != current.right_stick_x
        || previous.right_stick_y != current.right_stick_y
        || previous.left_trigger != current.left_trigger
        || previous.right_trigger != current.right_trigger
}

fn ordered_gamepad_buttons(previous: &GamepadState, current: &GamepadState) -> Vec<GamepadButton> {
    let mut buttons = vec![
        GamepadButton::South,
        GamepadButton::East,
        GamepadButton::West,
        GamepadButton::North,
        GamepadButton::LeftBumper,
        GamepadButton::RightBumper,
        GamepadButton::LeftTrigger,
        GamepadButton::RightTrigger,
        GamepadButton::Select,
        GamepadButton::Start,
        GamepadButton::Guide,
        GamepadButton::LeftStick,
        GamepadButton::RightStick,
        GamepadButton::DPadUp,
        GamepadButton::DPadDown,
        GamepadButton::DPadLeft,
        GamepadButton::DPadRight,
    ];
    let other_codes = previous
        .buttons
        .iter()
        .chain(&current.buttons)
        .filter_map(|state| match state.button {
            GamepadButton::Other(code) => Some(code),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    buttons.extend(other_codes.into_iter().map(GamepadButton::Other));
    buttons
}

fn min_update_interval(max_update_hz: u16) -> Duration {
    let hz = max_update_hz.clamp(1, 1000);
    Duration::from_secs_f64(1.0 / f64::from(hz))
}

trait GamepadStateExt {
    fn update_button(&mut self, button: Button, pressed: bool) -> Option<&mut Self>;
    fn update_axis(
        &mut self,
        axis: Axis,
        value: f32,
        deadzone_basis_points: u16,
    ) -> Option<&mut Self>;
}

impl GamepadStateExt for GamepadState {
    fn update_button(&mut self, button: Button, pressed: bool) -> Option<&mut Self> {
        let button = map_button(button)?;

        if let Some(existing) = self.buttons.iter_mut().find(|state| state.button == button) {
            existing.pressed = pressed;
        } else {
            self.buttons.push(GamepadButtonState { button, pressed });
        }

        Some(self)
    }

    fn update_axis(
        &mut self,
        axis: Axis,
        value: f32,
        deadzone_basis_points: u16,
    ) -> Option<&mut Self> {
        let value = apply_deadzone(value, deadzone_basis_points);

        match axis {
            Axis::LeftStickX => self.left_stick_x = normalize_stick(value),
            Axis::LeftStickY => self.left_stick_y = normalize_stick(value),
            Axis::RightStickX => self.right_stick_x = normalize_stick(value),
            Axis::RightStickY => self.right_stick_y = normalize_stick(value),
            Axis::LeftZ => self.left_trigger = normalize_trigger(value),
            Axis::RightZ => self.right_trigger = normalize_trigger(value),
            _ => return None,
        }

        Some(self)
    }
}

fn map_button(button: Button) -> Option<GamepadButton> {
    Some(match button {
        Button::South => GamepadButton::South,
        Button::East => GamepadButton::East,
        Button::West => GamepadButton::West,
        Button::North => GamepadButton::North,
        Button::LeftTrigger => GamepadButton::LeftBumper,
        Button::RightTrigger => GamepadButton::RightBumper,
        Button::LeftTrigger2 => GamepadButton::LeftTrigger,
        Button::RightTrigger2 => GamepadButton::RightTrigger,
        Button::Select => GamepadButton::Select,
        Button::Start => GamepadButton::Start,
        Button::Mode => GamepadButton::Guide,
        Button::LeftThumb => GamepadButton::LeftStick,
        Button::RightThumb => GamepadButton::RightStick,
        Button::DPadUp => GamepadButton::DPadUp,
        Button::DPadDown => GamepadButton::DPadDown,
        Button::DPadLeft => GamepadButton::DPadLeft,
        Button::DPadRight => GamepadButton::DPadRight,
        Button::Unknown => return None,
        other => GamepadButton::Other(other as u16),
    })
}

fn apply_deadzone(value: f32, deadzone_basis_points: u16) -> f32 {
    let deadzone = f32::from(deadzone_basis_points.min(10_000)) / 10_000.0;
    if value.abs() < deadzone {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    }
}

fn normalize_stick(value: f32) -> i16 {
    let value = value.clamp(-1.0, 1.0);
    if value >= 0.0 {
        (value * i16::MAX as f32).round() as i16
    } else {
        (value * -(i16::MIN as f32)).round() as i16
    }
}

fn normalize_trigger(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
}

fn stable_gamepad_id(id: GamepadId) -> Option<u8> {
    u8::try_from(usize::from(id)).ok()
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingress::{CaptureOrigin, CaptureSource, SemanticInputIngress};

    #[tokio::test]
    async fn production_listener_publishes_with_gamepad_origin() {
        let (producer, mut consumer) = SemanticInputIngress::new(4);
        let origin = CaptureOrigin {
            source: CaptureSource::Gamepad,
            device_token: 7,
            instance_token: 11,
        };
        let listener = GilrsGamepadListener::new(
            producer,
            origin,
            GamepadListenerConfig {
                enabled: false,
                ..Default::default()
            },
        );

        listener.publish_for_test(CapturedInputPayload::Discrete(
            InputEvent::gamepad_disconnected(3),
        ));
        let captured = consumer.recv().await.expect("gamepad event");
        assert_eq!(captured.origin, origin);
    }

    #[test]
    fn maps_standard_buttons_to_protocol_buttons() {
        assert_eq!(map_button(Button::South), Some(GamepadButton::South));
        assert_eq!(
            map_button(Button::LeftTrigger),
            Some(GamepadButton::LeftBumper)
        );
        assert_eq!(
            map_button(Button::LeftTrigger2),
            Some(GamepadButton::LeftTrigger)
        );
        assert_eq!(
            map_button(Button::RightTrigger2),
            Some(GamepadButton::RightTrigger)
        );
        assert_eq!(
            map_button(Button::DPadRight),
            Some(GamepadButton::DPadRight)
        );
        assert_eq!(map_button(Button::Unknown), None);
    }

    #[test]
    fn applies_deadzone_before_axis_normalization() {
        assert_eq!(apply_deadzone(0.05, 800), 0.0);
        assert_eq!(normalize_stick(apply_deadzone(1.0, 800)), i16::MAX);
        assert_eq!(normalize_stick(apply_deadzone(-1.0, 800)), i16::MIN);
    }

    #[test]
    fn button_updates_preserve_snapshot_shape() {
        let mut state = GamepadState::neutral(0, 0, 0);

        state.update_button(Button::South, true).unwrap();
        state.update_button(Button::South, false).unwrap();

        assert_eq!(state.buttons.len(), 1);
        assert_eq!(state.buttons[0].button, GamepadButton::South);
        assert!(!state.buttons[0].pressed);
    }

    #[test]
    fn axis_updates_write_protocol_fields() {
        let mut state = GamepadState::neutral(0, 0, 0);

        state.update_axis(Axis::LeftStickX, 0.5, 0).unwrap();
        state.update_axis(Axis::RightZ, 1.0, 0).unwrap();

        assert!(state.left_stick_x > 16_000);
        assert_eq!(state.right_trigger, u16::MAX);
    }
}
