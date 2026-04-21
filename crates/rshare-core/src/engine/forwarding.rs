//! Input event forwarding engine
//!
//! This module handles the forwarding of input events from local to remote devices.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::{ButtonState, DeviceId, KeyState, Message, MouseButton};

/// Forwarding mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardingMode {
    /// Forward all events
    All,
    /// Forward only mouse events
    MouseOnly,
    /// Forward only keyboard events
    KeyboardOnly,
}

impl Default for ForwardingMode {
    fn default() -> Self {
        Self::All
    }
}

/// Configuration for event forwarding
#[derive(Debug, Clone)]
pub struct ForwardingConfig {
    /// Mode of forwarding
    pub mode: ForwardingMode,
    /// Batch size for mouse moves (reduces network traffic)
    pub mouse_batch_size: usize,
    /// Maximum batch delay
    pub max_batch_delay: Duration,
    /// Whether to compress events
    pub compress_events: bool,
}

impl Default for ForwardingConfig {
    fn default() -> Self {
        Self {
            mode: ForwardingMode::All,
            mouse_batch_size: 10,
            max_batch_delay: Duration::from_millis(16), // ~60fps
            compress_events: true,
        }
    }
}

/// Statistics for forwarded events
#[derive(Debug, Clone, Default)]
pub struct ForwardingStats {
    pub events_sent: u64,
    pub events_dropped: u64,
    pub bytes_sent: u64,
    pub last_send: Option<Instant>,
}

/// Raw input event for forwarding
#[derive(Debug, Clone)]
pub enum RawInputEvent {
    MouseMove { x: i32, y: i32 },
    MouseButton { button: u8, pressed: bool },
    MouseWheel { delta_x: i32, delta_y: i32 },
    Key { keycode: u32, pressed: bool },
}

/// Event batch for efficient transmission
#[derive(Debug, Clone, Default)]
pub struct EventBatch {
    mouse_moves: Vec<(i32, i32)>,
    mouse_buttons: Vec<(u8, bool)>,
    keys: Vec<(u32, bool)>,
    wheel_delta: Option<(i32, i32)>,
}

impl EventBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.mouse_moves.is_empty()
            && self.mouse_buttons.is_empty()
            && self.keys.is_empty()
            && self.wheel_delta.is_none()
    }

    pub fn add_mouse_move(&mut self, x: i32, y: i32) {
        self.mouse_moves.push((x, y));
    }

    pub fn add_mouse_button(&mut self, button: u8, pressed: bool) {
        self.mouse_buttons.push((button, pressed));
    }

    pub fn add_key(&mut self, keycode: u32, pressed: bool) {
        self.keys.push((keycode, pressed));
    }

    pub fn set_wheel_delta(&mut self, delta_x: i32, delta_y: i32) {
        self.wheel_delta = Some((delta_x, delta_y));
    }

    /// Get the final position (last mouse move in batch)
    pub fn final_position(&self) -> Option<(i32, i32)> {
        self.mouse_moves.last().copied()
    }

    /// Clear the batch
    pub fn clear(&mut self) {
        self.mouse_moves.clear();
        self.mouse_buttons.clear();
        self.keys.clear();
        self.wheel_delta = None;
    }
}

/// Input event forwarding engine
pub struct ForwardingEngine {
    config: ForwardingConfig,
    current_batch: EventBatch,
    batch_start: Option<Instant>,
    stats: ForwardingStats,
    target_device: Option<DeviceId>,
}

impl ForwardingEngine {
    pub fn new() -> Self {
        Self {
            config: ForwardingConfig::default(),
            current_batch: EventBatch::new(),
            batch_start: None,
            stats: ForwardingStats::default(),
            target_device: None,
        }
    }

    pub fn with_config(mut self, config: ForwardingConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the target device for forwarding
    pub fn set_target(&mut self, device: DeviceId) {
        self.target_device = Some(device);
    }

    /// Clear the target device
    pub fn clear_target(&mut self) {
        self.target_device = None;
    }

    /// Get the current target device
    pub fn target(&self) -> Option<DeviceId> {
        self.target_device
    }

    /// Process a raw input event and return messages to send
    pub fn process_event(&mut self, event: RawInputEvent) -> Vec<Message> {
        if self.target_device.is_none() {
            return Vec::new();
        }

        let mut messages = Vec::new();

        match event {
            RawInputEvent::MouseMove { x, y } => {
                self.current_batch.add_mouse_move(x, y);

                if self.batch_start.is_none() {
                    self.batch_start = Some(Instant::now());
                }

                // Flush if batch is full
                if self.current_batch.mouse_moves.len() >= self.config.mouse_batch_size {
                    messages.extend(self.flush_batch());
                }
            }
            RawInputEvent::MouseButton { button, pressed } => {
                self.current_batch.add_mouse_button(button, pressed);
                messages.extend(self.flush_batch());
            }
            RawInputEvent::MouseWheel { delta_x, delta_y } => {
                self.current_batch.set_wheel_delta(delta_x, delta_y);
                messages.extend(self.flush_batch());
            }
            RawInputEvent::Key { keycode, pressed } => {
                self.current_batch.add_key(keycode, pressed);
                messages.extend(self.flush_batch());
            }
        }

        messages
    }

    /// Flush the current batch and return messages
    pub fn flush_batch(&mut self) -> Vec<Message> {
        let mut messages = Vec::new();

        if self.current_batch.is_empty() {
            return messages;
        }

        // Send final mouse position
        if let Some((x, y)) = self.current_batch.final_position() {
            messages.push(Message::MouseMove { x, y });
            self.stats.events_sent += 1;
        }

        // Send mouse button events
        for (button, pressed) in &self.current_batch.mouse_buttons {
            let button = convert_button_code(*button);
            messages.push(Message::MouseButton {
                button,
                state: if *pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
            });
            self.stats.events_sent += 1;
        }

        // Send wheel delta
        if let Some((delta_x, delta_y)) = self.current_batch.wheel_delta {
            messages.push(Message::MouseWheel { delta_x, delta_y });
            self.stats.events_sent += 1;
        }

        // Send key events
        for (keycode, pressed) in &self.current_batch.keys {
            messages.push(Message::Key {
                keycode: *keycode,
                state: if *pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
            });
            self.stats.events_sent += 1;
        }

        self.current_batch.clear();
        self.batch_start = None;
        self.stats.last_send = Some(Instant::now());

        messages
    }

    /// Check if the batch should be flushed based on time
    pub fn should_flush_batch(&self) -> bool {
        if let Some(start) = self.batch_start {
            start.elapsed() >= self.config.max_batch_delay
        } else {
            false
        }
    }

    /// Get the statistics
    pub fn stats(&self) -> &ForwardingStats {
        &self.stats
    }

    /// Reset the statistics
    pub fn reset_stats(&mut self) {
        self.stats = ForwardingStats::default();
    }
}

impl Default for ForwardingEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert button code to MouseButton
fn convert_button_code(code: u8) -> MouseButton {
    match code {
        1 => MouseButton::Left,
        2 => MouseButton::Middle,
        3 => MouseButton::Right,
        4 => MouseButton::Back,
        5 => MouseButton::Forward,
        n => MouseButton::Other(n),
    }
}

/// Shared forwarding engine for async access
pub type SharedForwardingEngine = Arc<RwLock<ForwardingEngine>>;

/// Create a shared forwarding engine
pub fn create_shared_forwarding_engine() -> SharedForwardingEngine {
    Arc::new(RwLock::new(ForwardingEngine::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_forwarding_config_default() {
        let config = ForwardingConfig::default();
        assert_eq!(config.mouse_batch_size, 10);
        assert!(config.compress_events);
    }

    #[test]
    fn test_event_batch_empty() {
        let batch = EventBatch::new();
        assert!(batch.is_empty());
    }

    #[test]
    fn test_event_batch_mouse_moves() {
        let mut batch = EventBatch::new();
        batch.add_mouse_move(100, 200);
        batch.add_mouse_move(150, 250);

        assert!(!batch.is_empty());
        assert_eq!(batch.final_position(), Some((150, 250)));
    }

    #[test]
    fn test_forwarding_engine() {
        let mut engine = ForwardingEngine::new();
        let device_id = DeviceId::new_v4();
        engine.set_target(device_id);

        // Process enough events to fill batch
        for i in 0..10 {
            let event = RawInputEvent::MouseMove { x: i, y: i };
            let _ = engine.process_event(event);
        }

        // Only final position is sent when batch fills
        assert_eq!(engine.stats().events_sent, 1);
    }

    #[test]
    fn test_flush_on_button() {
        let mut engine = ForwardingEngine::new();
        let device_id = DeviceId::new_v4();
        engine.set_target(device_id);

        // Add mouse move
        let event = RawInputEvent::MouseMove { x: 100, y: 200 };
        assert!(engine.process_event(event).is_empty());

        // Add button press should flush
        let event = RawInputEvent::MouseButton {
            button: 1,
            pressed: true,
        };
        let messages = engine.process_event(event);

        assert_eq!(messages.len(), 2); // MouseMove + MouseButton
    }

    #[test]
    fn test_convert_button_code() {
        assert_eq!(convert_button_code(1), MouseButton::Left);
        assert_eq!(convert_button_code(2), MouseButton::Middle);
        assert_eq!(convert_button_code(3), MouseButton::Right);
        assert_eq!(convert_button_code(99), MouseButton::Other(99));
    }
}
