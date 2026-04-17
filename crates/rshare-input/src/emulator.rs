//! Input emulator - simulates input events using enigo

use anyhow::Result;
use std::time::Duration;
use std::sync::Arc;
use std::sync::Mutex;
use enigo::{Enigo, Mouse, Keyboard, Key, Button, Direction, Coordinate};

use crate::events::{InputEvent, MouseButton, ButtonState, KeyCode};

/// Input emulator trait
pub trait InputEmulator {
    /// Emulate an input event
    fn emulate(&mut self, event: InputEvent) -> Result<()>;

    /// Move mouse to absolute position
    fn move_mouse(&mut self, x: i32, y: i32) -> Result<()>;

    /// Move mouse by relative offset
    fn move_mouse_relative(&mut self, dx: i32, dy: i32) -> Result<()>;

    /// Press mouse button
    fn press_button(&mut self, button: MouseButton) -> Result<()>;

    /// Release mouse button
    fn release_button(&mut self, button: MouseButton) -> Result<()>;

    /// Click mouse button
    fn click_button(&mut self, button: MouseButton) -> Result<()>;

    /// Scroll mouse wheel
    fn scroll_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()>;

    /// Press key
    fn press_key(&mut self, keycode: KeyCode) -> Result<()>;

    /// Release key
    fn release_key(&mut self, keycode: KeyCode) -> Result<()>;

    /// Type a key (press and release)
    fn type_key(&mut self, keycode: KeyCode) -> Result<()>;

    /// Check if emulator is active
    fn is_active(&self) -> bool;
}

/// Configuration for input emulator
#[derive(Debug, Clone)]
pub struct EmulatorConfig {
    /// Delay between input events (prevents flooding)
    pub event_delay: Duration,
    /// Scale factor for mouse coordinates
    pub mouse_scale: f64,
    /// Whether to emulate input immediately or queue
    pub immediate: bool,
}

impl Default for EmulatorConfig {
    fn default() -> Self {
        Self {
            event_delay: Duration::from_millis(1),
            mouse_scale: 1.0,
            immediate: true,
        }
    }
}

/// Enigo-based input emulator (cross-platform)
pub struct EnigoInputEmulator {
    config: EmulatorConfig,
    enigo: Arc<Mutex<Enigo>>,
    active: bool,
}

impl EnigoInputEmulator {
    /// Create a new enigo-based input emulator
    pub fn new() -> Result<Self> {
        let enigo = Enigo::new(&enigo::Settings::default())
            .map_err(|e| anyhow::anyhow!("Failed to create enigo: {:?}", e))?;

        Ok(Self {
            config: EmulatorConfig::default(),
            enigo: Arc::new(Mutex::new(enigo)),
            active: false,
        })
    }

    /// Set the configuration
    pub fn with_config(mut self, config: EmulatorConfig) -> Self {
        self.config = config;
        self
    }

    /// Activate the emulator
    pub fn activate(&mut self) -> Result<()> {
        tracing::info!("Enigo input emulator activating");
        self.active = true;
        Ok(())
    }

    /// Deactivate the emulator
    pub fn deactivate(&mut self) -> Result<()> {
        tracing::info!("Enigo input emulator deactivating");
        self.active = false;
        Ok(())
    }

    /// Convert MouseButton to enigo's Button
    fn convert_mouse_button(button: MouseButton) -> Button {
        match button {
            MouseButton::Left => Button::Left,
            MouseButton::Middle => Button::Middle,
            MouseButton::Right => Button::Right,
            MouseButton::Back => Button::Back,
            MouseButton::Forward => Button::Forward,
            MouseButton::Other(_) => Button::Left, // Default to left
        }
    }

    /// Convert KeyCode to enigo's Key
    fn convert_keycode(keycode: KeyCode) -> Option<Key> {
        use enigo::Key as EKey;

        Some(match keycode {
            KeyCode::AltLeft => EKey::Alt,
            KeyCode::AltRight => EKey::Alt,
            KeyCode::Backspace => EKey::Backspace,
            KeyCode::CapsLock => EKey::CapsLock,
            KeyCode::ControlLeft => EKey::Control,
            KeyCode::ControlRight => EKey::Control,
            KeyCode::Down => EKey::DownArrow,
            KeyCode::Escape => EKey::Escape,
            KeyCode::F1 => EKey::F1,
            KeyCode::F2 => EKey::F2,
            KeyCode::F3 => EKey::F3,
            KeyCode::F4 => EKey::F4,
            KeyCode::F5 => EKey::F5,
            KeyCode::F6 => EKey::F6,
            KeyCode::F7 => EKey::F7,
            KeyCode::F8 => EKey::F8,
            KeyCode::F9 => EKey::F9,
            KeyCode::F10 => EKey::F10,
            KeyCode::F11 => EKey::F11,
            KeyCode::F12 => EKey::F12,
            KeyCode::Left => EKey::LeftArrow,
            KeyCode::Right => EKey::RightArrow,
            KeyCode::ShiftLeft => EKey::Shift,
            KeyCode::ShiftRight => EKey::Shift,
            KeyCode::Up => EKey::UpArrow,
            KeyCode::SuperLeft | KeyCode::SuperRight => EKey::Meta,
            KeyCode::Tab => EKey::Tab,
            KeyCode::Enter => EKey::Return,
            KeyCode::Space => EKey::Space,
            KeyCode::Home => EKey::Home,
            KeyCode::End => EKey::End,
            KeyCode::PageUp => EKey::PageUp,
            KeyCode::PageDown => EKey::PageDown,
            KeyCode::Insert => EKey::Insert,
            KeyCode::Delete => EKey::Delete,
            KeyCode::Char(c) => EKey::Unicode(c as char),
            _ => return None,
        })
    }
}

impl Default for EnigoInputEmulator {
    fn default() -> Self {
        Self::new().expect("Failed to create EnigoInputEmulator")
    }
}

impl InputEmulator for EnigoInputEmulator {
    fn emulate(&mut self, event: InputEvent) -> Result<()> {
        if !self.active {
            return Ok(());
        }

        match event {
            InputEvent::MouseMove { x, y } => self.move_mouse(x, y)?,
            InputEvent::MouseButton { button, state } => match state {
                ButtonState::Pressed => self.press_button(button)?,
                ButtonState::Released => self.release_button(button)?,
            },
            InputEvent::MouseWheel { delta_x, delta_y } => self.scroll_wheel(delta_x, delta_y)?,
            InputEvent::Key { keycode, state } => match state {
                ButtonState::Pressed => self.press_key(keycode)?,
                ButtonState::Released => self.release_key(keycode)?,
            },
            InputEvent::KeyExtended { keycode, state, .. } => match state {
                ButtonState::Pressed => self.press_key(keycode)?,
                ButtonState::Released => self.release_key(keycode)?,
            },
        }

        // Apply event delay if configured
        if self.config.event_delay.as_millis() > 0 {
            std::thread::sleep(self.config.event_delay);
        }

        Ok(())
    }

    fn move_mouse(&mut self, x: i32, y: i32) -> Result<()> {
        let mut enigo = self.enigo.lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        // Scale coordinates if needed
        let x = (x as f64 * self.config.mouse_scale) as i32;
        let y = (y as f64 * self.config.mouse_scale) as i32;

        tracing::trace!("Moving mouse to ({}, {})", x, y);

        enigo.move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| anyhow::anyhow!("Failed to move mouse: {:?}", e))?;

        Ok(())
    }

    fn move_mouse_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        let mut enigo = self.enigo.lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        tracing::trace!("Moving mouse by ({}, {})", dx, dy);

        enigo.move_mouse(dx, dy, Coordinate::Rel)
            .map_err(|e| anyhow::anyhow!("Failed to move mouse: {:?}", e))?;

        Ok(())
    }

    fn press_button(&mut self, button: MouseButton) -> Result<()> {
        let mut enigo = self.enigo.lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        let enigo_btn = Self::convert_mouse_button(button);
        tracing::trace!("Pressing mouse button: {:?}", button);

        enigo.button(enigo_btn, Direction::Press)
            .map_err(|e| anyhow::anyhow!("Failed to press button: {:?}", e))?;

        Ok(())
    }

    fn release_button(&mut self, button: MouseButton) -> Result<()> {
        let mut enigo = self.enigo.lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        let enigo_btn = Self::convert_mouse_button(button);
        tracing::trace!("Releasing mouse button: {:?}", button);

        enigo.button(enigo_btn, Direction::Release)
            .map_err(|e| anyhow::anyhow!("Failed to release button: {:?}", e))?;

        Ok(())
    }

    fn click_button(&mut self, button: MouseButton) -> Result<()> {
        self.press_button(button)?;
        self.release_button(button)
    }

    fn scroll_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
        let mut enigo = self.enigo.lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        tracing::trace!("Scrolling wheel: ({}, {})", delta_x, delta_y);

        if delta_y > 0 {
            enigo.button(Button::ScrollUp, Direction::Click)
                .map_err(|e| anyhow::anyhow!("Failed to scroll up: {:?}", e))?;
        } else if delta_y < 0 {
            enigo.button(Button::ScrollDown, Direction::Click)
                .map_err(|e| anyhow::anyhow!("Failed to scroll down: {:?}", e))?;
        }

        if delta_x > 0 {
            enigo.button(Button::ScrollRight, Direction::Click)
                .map_err(|e| anyhow::anyhow!("Failed to scroll right: {:?}", e))?;
        } else if delta_x < 0 {
            enigo.button(Button::ScrollLeft, Direction::Click)
                .map_err(|e| anyhow::anyhow!("Failed to scroll left: {:?}", e))?;
        }

        Ok(())
    }

    fn press_key(&mut self, keycode: KeyCode) -> Result<()> {
        let mut enigo = self.enigo.lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        let key = Self::convert_keycode(keycode)
            .ok_or_else(|| anyhow::anyhow!("Unsupported keycode: {:?}", keycode))?;

        tracing::trace!("Pressing key: {:?}", keycode);

        enigo.key(key, Direction::Press)
            .map_err(|e| anyhow::anyhow!("Failed to press key: {:?}", e))?;

        Ok(())
    }

    fn release_key(&mut self, keycode: KeyCode) -> Result<()> {
        let mut enigo = self.enigo.lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        let key = Self::convert_keycode(keycode)
            .ok_or_else(|| anyhow::anyhow!("Unsupported keycode: {:?}", keycode))?;

        tracing::trace!("Releasing key: {:?}", keycode);

        enigo.key(key, Direction::Release)
            .map_err(|e| anyhow::anyhow!("Failed to release key: {:?}", e))?;

        Ok(())
    }

    fn type_key(&mut self, keycode: KeyCode) -> Result<()> {
        self.press_key(keycode)?;
        self.release_key(keycode)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

/// Default input emulator (alias for EnigoInputEmulator)
pub type DefaultInputEmulator = EnigoInputEmulator;

/// Batch emulator for sending multiple events efficiently
pub struct BatchEmulator<E: InputEmulator> {
    inner: E,
    batch: Vec<InputEvent>,
    batch_size: usize,
}

impl<E: InputEmulator> BatchEmulator<E> {
    /// Create a new batch emulator
    pub fn new(inner: E, batch_size: usize) -> Self {
        Self {
            inner,
            batch: Vec::new(),
            batch_size,
        }
    }

    /// Queue an event to be emulated
    pub fn queue(&mut self, event: InputEvent) -> Result<()> {
        self.batch.push(event);

        if self.batch.len() >= self.batch_size {
            self.flush()?;
        }

        Ok(())
    }

    /// Flush all queued events
    pub fn flush(&mut self) -> Result<()> {
        for event in self.batch.drain(..) {
            self.inner.emulate(event)?;
        }
        Ok(())
    }

    /// Get the inner emulator
    pub fn inner(&self) -> &E {
        &self.inner
    }

    /// Get mutable reference to inner emulator
    pub fn inner_mut(&mut self) -> &mut E {
        &mut self.inner
    }
}

impl<E: InputEmulator> InputEmulator for BatchEmulator<E> {
    fn emulate(&mut self, event: InputEvent) -> Result<()> {
        self.queue(event)
    }

    fn move_mouse(&mut self, x: i32, y: i32) -> Result<()> {
        self.inner.move_mouse(x, y)
    }

    fn move_mouse_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.inner.move_mouse_relative(dx, dy)
    }

    fn press_button(&mut self, button: MouseButton) -> Result<()> {
        self.inner.press_button(button)
    }

    fn release_button(&mut self, button: MouseButton) -> Result<()> {
        self.inner.release_button(button)
    }

    fn click_button(&mut self, button: MouseButton) -> Result<()> {
        self.inner.click_button(button)
    }

    fn scroll_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
        self.inner.scroll_wheel(delta_x, delta_y)
    }

    fn press_key(&mut self, keycode: KeyCode) -> Result<()> {
        self.inner.press_key(keycode)
    }

    fn release_key(&mut self, keycode: KeyCode) -> Result<()> {
        self.inner.release_key(keycode)
    }

    fn type_key(&mut self, keycode: KeyCode) -> Result<()> {
        self.inner.type_key(keycode)
    }

    fn is_active(&self) -> bool {
        self.inner.is_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emulator_config_default() {
        let config = EmulatorConfig::default();
        assert_eq!(config.mouse_scale, 1.0);
        assert!(config.immediate);
    }

    #[test]
    fn test_enigo_emulator_creation() {
        let emulator = EnigoInputEmulator::new();
        assert!(emulator.is_ok());
        let mut emulator = emulator.unwrap();
        assert!(!emulator.is_active());

        let result = emulator.activate();
        assert!(result.is_ok());
        assert!(emulator.is_active());
    }

    #[test]
    fn test_enigo_emulator_with_config() {
        let config = EmulatorConfig {
            event_delay: Duration::from_millis(10),
            mouse_scale: 1.5,
            immediate: false,
        };

        let emulator = EnigoInputEmulator::new()
            .unwrap()
            .with_config(config);

        assert_eq!(emulator.config.mouse_scale, 1.5);
        assert!(!emulator.config.immediate);
    }

    #[test]
    fn test_batch_emulator() {
        let inner = EnigoInputEmulator::new().unwrap();
        let mut batch = BatchEmulator::new(inner, 10);

        for i in 0..5 {
            batch.queue(InputEvent::mouse_move(i, i)).unwrap();
        }

        assert_eq!(batch.batch.len(), 5);
        batch.flush().unwrap();
        assert_eq!(batch.batch.len(), 0);
    }

    #[test]
    fn test_mouse_button_conversion() {
        assert_eq!(EnigoInputEmulator::convert_mouse_button(MouseButton::Left), Button::Left);
        assert_eq!(EnigoInputEmulator::convert_mouse_button(MouseButton::Right), Button::Right);
        assert_eq!(EnigoInputEmulator::convert_mouse_button(MouseButton::Middle), Button::Middle);
    }

    #[test]
    fn test_keycode_conversion() {
        assert_eq!(EnigoInputEmulator::convert_keycode(KeyCode::Space), Some(Key::Space));
        assert_eq!(EnigoInputEmulator::convert_keycode(KeyCode::Escape), Some(Key::Escape));
        assert_eq!(EnigoInputEmulator::convert_keycode(KeyCode::Enter), Some(Key::Return));
    }
}
