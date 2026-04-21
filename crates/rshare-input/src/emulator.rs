//! Input emulator - simulates input events using enigo

use anyhow::Result;
use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use crate::events::{ButtonState, InputEvent, KeyCode, MouseButton};

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
            active: true, // Auto-activate on creation
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
            MouseButton::Back | MouseButton::Forward => Button::Left,
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
            KeyCode::Insert => return None,
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
        let mut enigo = self
            .enigo
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        // Scale coordinates if needed
        let x = (x as f64 * self.config.mouse_scale) as i32;
        let y = (y as f64 * self.config.mouse_scale) as i32;

        tracing::trace!("Moving mouse to ({}, {})", x, y);

        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| anyhow::anyhow!("Failed to move mouse: {:?}", e))?;

        Ok(())
    }

    fn move_mouse_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        let mut enigo = self
            .enigo
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        tracing::trace!("Moving mouse by ({}, {})", dx, dy);

        enigo
            .move_mouse(dx, dy, Coordinate::Rel)
            .map_err(|e| anyhow::anyhow!("Failed to move mouse: {:?}", e))?;

        Ok(())
    }

    fn press_button(&mut self, button: MouseButton) -> Result<()> {
        let mut enigo = self
            .enigo
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        let enigo_btn = Self::convert_mouse_button(button);
        tracing::trace!("Pressing mouse button: {:?}", button);

        enigo
            .button(enigo_btn, Direction::Press)
            .map_err(|e| anyhow::anyhow!("Failed to press button: {:?}", e))?;

        Ok(())
    }

    fn release_button(&mut self, button: MouseButton) -> Result<()> {
        let mut enigo = self
            .enigo
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        let enigo_btn = Self::convert_mouse_button(button);
        tracing::trace!("Releasing mouse button: {:?}", button);

        enigo
            .button(enigo_btn, Direction::Release)
            .map_err(|e| anyhow::anyhow!("Failed to release button: {:?}", e))?;

        Ok(())
    }

    fn click_button(&mut self, button: MouseButton) -> Result<()> {
        self.press_button(button)?;
        self.release_button(button)
    }

    fn scroll_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
        let mut enigo = self
            .enigo
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        tracing::trace!("Scrolling wheel: ({}, {})", delta_x, delta_y);

        if delta_y > 0 {
            enigo
                .button(Button::ScrollUp, Direction::Click)
                .map_err(|e| anyhow::anyhow!("Failed to scroll up: {:?}", e))?;
        } else if delta_y < 0 {
            enigo
                .button(Button::ScrollDown, Direction::Click)
                .map_err(|e| anyhow::anyhow!("Failed to scroll down: {:?}", e))?;
        }

        if delta_x > 0 {
            enigo
                .button(Button::ScrollRight, Direction::Click)
                .map_err(|e| anyhow::anyhow!("Failed to scroll right: {:?}", e))?;
        } else if delta_x < 0 {
            enigo
                .button(Button::ScrollLeft, Direction::Click)
                .map_err(|e| anyhow::anyhow!("Failed to scroll left: {:?}", e))?;
        }

        Ok(())
    }

    fn press_key(&mut self, keycode: KeyCode) -> Result<()> {
        let mut enigo = self
            .enigo
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        let key = Self::convert_keycode(keycode)
            .ok_or_else(|| anyhow::anyhow!("Unsupported keycode: {:?}", keycode))?;

        tracing::trace!("Pressing key: {:?}", keycode);

        enigo
            .key(key, Direction::Press)
            .map_err(|e| anyhow::anyhow!("Failed to press key: {:?}", e))?;

        Ok(())
    }

    fn release_key(&mut self, keycode: KeyCode) -> Result<()> {
        let mut enigo = self
            .enigo
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock enigo: {}", e))?;

        let key = Self::convert_keycode(keycode)
            .ok_or_else(|| anyhow::anyhow!("Unsupported keycode: {:?}", keycode))?;

        tracing::trace!("Releasing key: {:?}", keycode);

        enigo
            .key(key, Direction::Release)
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

/// Native macOS input emulator backed by CoreGraphics.
#[cfg(target_os = "macos")]
pub struct MacosNativeInputEmulator {
    inner: rshare_platform::MacosInputEmulator,
    config: EmulatorConfig,
    active: bool,
}

#[cfg(target_os = "macos")]
impl MacosNativeInputEmulator {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: rshare_platform::MacosInputEmulator::new(),
            config: EmulatorConfig::default(),
            active: false,
        })
    }

    pub fn new_for_test() -> Result<Self> {
        Ok(Self {
            inner: rshare_platform::MacosInputEmulator::new(),
            config: EmulatorConfig::default(),
            active: true,
        })
    }

    pub fn with_config(mut self, config: EmulatorConfig) -> Self {
        self.config = config;
        self
    }

    pub fn activate(&mut self) -> Result<()> {
        self.inner.activate()?;
        self.active = true;
        Ok(())
    }

    pub fn deactivate(&mut self) -> Result<()> {
        self.inner.deactivate()?;
        self.active = false;
        Ok(())
    }

    fn convert_mouse_button(button: MouseButton) -> Result<u8> {
        match button {
            MouseButton::Left | MouseButton::Middle | MouseButton::Right => Ok(button.to_code()),
            MouseButton::Back | MouseButton::Forward | MouseButton::Other(_) => {
                anyhow::bail!("Unsupported macOS mouse button: {:?}", button)
            }
        }
    }

    fn convert_keycode(keycode: KeyCode) -> Result<u32> {
        let raw = match keycode {
            KeyCode::Char(c) => macos_char_keycode(c)? as u32,
            KeyCode::Raw(raw) => raw,
            _ => keycode.to_raw(),
        };

        rshare_platform::mac_key_code(raw)?;
        Ok(raw)
    }
}

#[cfg(target_os = "macos")]
impl Default for MacosNativeInputEmulator {
    fn default() -> Self {
        Self::new().expect("Failed to create MacosNativeInputEmulator")
    }
}

#[cfg(target_os = "macos")]
impl InputEmulator for MacosNativeInputEmulator {
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

        if self.config.event_delay.as_millis() > 0 {
            std::thread::sleep(self.config.event_delay);
        }

        Ok(())
    }

    fn move_mouse(&mut self, x: i32, y: i32) -> Result<()> {
        let x = (x as f64 * self.config.mouse_scale) as i32;
        let y = (y as f64 * self.config.mouse_scale) as i32;
        self.inner.send_mouse_move(x, y)
    }

    fn move_mouse_relative(&mut self, _dx: i32, _dy: i32) -> Result<()> {
        anyhow::bail!("Relative mouse movement is not supported by the macOS native emulator")
    }

    fn press_button(&mut self, button: MouseButton) -> Result<()> {
        self.inner
            .send_button(Self::convert_mouse_button(button)?, true)
    }

    fn release_button(&mut self, button: MouseButton) -> Result<()> {
        self.inner
            .send_button(Self::convert_mouse_button(button)?, false)
    }

    fn click_button(&mut self, button: MouseButton) -> Result<()> {
        self.press_button(button)?;
        self.release_button(button)
    }

    fn scroll_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
        self.inner.send_wheel(delta_x, delta_y)
    }

    fn press_key(&mut self, keycode: KeyCode) -> Result<()> {
        self.inner.send_key(Self::convert_keycode(keycode)?, true)
    }

    fn release_key(&mut self, keycode: KeyCode) -> Result<()> {
        self.inner.send_key(Self::convert_keycode(keycode)?, false)
    }

    fn type_key(&mut self, keycode: KeyCode) -> Result<()> {
        self.press_key(keycode)?;
        self.release_key(keycode)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(target_os = "macos")]
fn macos_char_keycode(value: u8) -> Result<u16> {
    let key = match value.to_ascii_uppercase() {
        b'A' => 0x00,
        b'S' => 0x01,
        b'D' => 0x02,
        b'F' => 0x03,
        b'H' => 0x04,
        b'G' => 0x05,
        b'Z' => 0x06,
        b'X' => 0x07,
        b'C' => 0x08,
        b'V' => 0x09,
        b'B' => 0x0B,
        b'Q' => 0x0C,
        b'W' => 0x0D,
        b'E' => 0x0E,
        b'R' => 0x0F,
        b'Y' => 0x10,
        b'T' => 0x11,
        b'1' => 0x12,
        b'2' => 0x13,
        b'3' => 0x14,
        b'4' => 0x15,
        b'6' => 0x16,
        b'5' => 0x17,
        b'9' => 0x19,
        b'7' => 0x1A,
        b'8' => 0x1C,
        b'0' => 0x1D,
        b'O' => 0x1F,
        b'U' => 0x20,
        b'I' => 0x22,
        b'P' => 0x23,
        b'L' => 0x25,
        b'J' => 0x26,
        b'K' => 0x28,
        b'N' => 0x2D,
        b'M' => 0x2E,
        _ => anyhow::bail!("Unsupported macOS character key: {}", value),
    };
    Ok(key)
}

/// Native Windows input emulator backed by SendInput.
#[cfg(target_os = "windows")]
pub struct WindowsNativeInputEmulator {
    inner: rshare_platform::WindowsInputEmulator,
    config: EmulatorConfig,
    active: bool,
    health: crate::backend::BackendHealth,
}

#[cfg(target_os = "windows")]
impl std::fmt::Debug for WindowsNativeInputEmulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsNativeInputEmulator")
            .field("active", &self.active)
            .field("health", &self.health)
            .finish()
    }
}

// Implement InjectBackend for WindowsNativeInputEmulator
#[cfg(target_os = "windows")]
impl crate::backend::InjectBackend for WindowsNativeInputEmulator {
    fn kind(&self) -> rshare_core::BackendKind {
        rshare_core::BackendKind::WindowsNative
    }

    fn health(&self) -> crate::backend::BackendHealth {
        self.health.clone()
    }

    fn inject(&mut self, event: InputEvent) -> anyhow::Result<()> {
        self.emulate(event)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(target_os = "windows")]
impl WindowsNativeInputEmulator {
    pub fn new() -> Result<Self> {
        let mut inner = rshare_platform::WindowsInputEmulator::new();
        inner.activate()?;

        Ok(Self {
            inner,
            config: EmulatorConfig::default(),
            active: true,
            health: crate::backend::BackendHealth::Healthy,
        })
    }

    pub fn with_config(mut self, config: EmulatorConfig) -> Self {
        self.config = config;
        self
    }

    pub fn activate(&mut self) -> Result<()> {
        self.inner.activate()?;
        self.active = true;
        Ok(())
    }

    pub fn deactivate(&mut self) -> Result<()> {
        self.inner.deactivate()?;
        self.active = false;
        Ok(())
    }

    #[cfg(test)]
    fn platform_emulator_is_active_for_test(&self) -> bool {
        self.inner.is_active()
    }

    fn convert_mouse_button(button: MouseButton) -> Result<u8> {
        Ok(button.to_code())
    }

    fn convert_keycode(keycode: KeyCode) -> Result<u16> {
        use rshare_platform::vk;

        let vk = match keycode {
            KeyCode::Escape => vk::VK_ESCAPE,
            KeyCode::Enter => vk::VK_RETURN,
            KeyCode::Tab => vk::VK_TAB,
            KeyCode::Backspace => vk::VK_BACK,
            KeyCode::Delete => vk::VK_DELETE,
            KeyCode::Insert => vk::VK_INSERT,
            KeyCode::Home => vk::VK_HOME,
            KeyCode::End => vk::VK_END,
            KeyCode::PageUp => vk::VK_PRIOR,
            KeyCode::PageDown => vk::VK_NEXT,
            KeyCode::Up => vk::VK_UP,
            KeyCode::Down => vk::VK_DOWN,
            KeyCode::Left => vk::VK_LEFT,
            KeyCode::Right => vk::VK_RIGHT,
            KeyCode::F1 => 0x70,
            KeyCode::F2 => 0x71,
            KeyCode::F3 => 0x72,
            KeyCode::F4 => 0x73,
            KeyCode::F5 => 0x74,
            KeyCode::F6 => 0x75,
            KeyCode::F7 => 0x76,
            KeyCode::F8 => 0x77,
            KeyCode::F9 => 0x78,
            KeyCode::F10 => 0x79,
            KeyCode::F11 => 0x7A,
            KeyCode::F12 => 0x7B,
            KeyCode::Space => vk::VK_SPACE,
            KeyCode::ShiftLeft | KeyCode::ShiftRight => vk::VK_SHIFT,
            KeyCode::ControlLeft | KeyCode::ControlRight => vk::VK_CONTROL,
            KeyCode::AltLeft | KeyCode::AltRight => vk::VK_MENU,
            KeyCode::SuperLeft => vk::VK_LWIN,
            KeyCode::SuperRight => vk::VK_RWIN,
            KeyCode::Raw(raw) => raw as u16,
            KeyCode::Char(c) => {
                let c = c.to_ascii_uppercase();
                if c.is_ascii_alphabetic() {
                    0x41 + (c - b'A') as u16
                } else if c.is_ascii_digit() {
                    0x30 + (c - b'0') as u16
                } else {
                    anyhow::bail!("Unsupported character key: {}", c)
                }
            }
            _ => anyhow::bail!("Unsupported keycode: {:?}", keycode),
        };
        Ok(vk)
    }
}

#[cfg(target_os = "windows")]
impl Default for WindowsNativeInputEmulator {
    fn default() -> Self {
        Self::new().expect("Failed to create WindowsNativeInputEmulator")
    }
}

#[cfg(target_os = "windows")]
impl InputEmulator for WindowsNativeInputEmulator {
    fn emulate(&mut self, event: InputEvent) -> Result<()> {
        if !self.active {
            anyhow::bail!("Windows native input emulator is not active");
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

        if self.config.event_delay.as_millis() > 0 {
            std::thread::sleep(self.config.event_delay);
        }

        Ok(())
    }

    fn move_mouse(&mut self, x: i32, y: i32) -> Result<()> {
        let x = (x as f64 * self.config.mouse_scale) as i32;
        let y = (y as f64 * self.config.mouse_scale) as i32;
        self.inner.send_mouse_move(x, y)
    }

    fn move_mouse_relative(&mut self, _dx: i32, _dy: i32) -> Result<()> {
        anyhow::bail!("Relative mouse movement is not supported by the Windows native emulator")
    }

    fn press_button(&mut self, button: MouseButton) -> Result<()> {
        self.inner
            .send_button(Self::convert_mouse_button(button)?, true)
    }

    fn release_button(&mut self, button: MouseButton) -> Result<()> {
        self.inner
            .send_button(Self::convert_mouse_button(button)?, false)
    }

    fn click_button(&mut self, button: MouseButton) -> Result<()> {
        self.press_button(button)?;
        self.release_button(button)
    }

    fn scroll_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
        self.inner.send_wheel(delta_x, delta_y)
    }

    fn press_key(&mut self, keycode: KeyCode) -> Result<()> {
        self.inner.send_key(Self::convert_keycode(keycode)?, true)
    }

    fn release_key(&mut self, keycode: KeyCode) -> Result<()> {
        self.inner.send_key(Self::convert_keycode(keycode)?, false)
    }

    fn type_key(&mut self, keycode: KeyCode) -> Result<()> {
        self.press_key(keycode)?;
        self.release_key(keycode)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

/// Default input emulator.
#[cfg(target_os = "macos")]
pub type DefaultInputEmulator = MacosNativeInputEmulator;

/// Default input emulator (Windows).
#[cfg(all(windows, not(test)))]
pub type DefaultInputEmulator = WindowsNativeInputEmulator;

/// Default input emulator (fallback).
#[cfg(all(not(target_os = "macos"), not(all(windows, not(test)))))]
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
        if emulator.is_err() {
            return;
        }

        let emulator = emulator.unwrap();
        // Emulator is now auto-activated on creation
        assert!(emulator.is_active());
    }

    #[test]
    fn test_enigo_emulator_with_config() {
        let config = EmulatorConfig {
            event_delay: Duration::from_millis(10),
            mouse_scale: 1.5,
            immediate: false,
        };

        let emulator = match EnigoInputEmulator::new() {
            Ok(emulator) => emulator.with_config(config),
            Err(_) => return,
        };

        assert_eq!(emulator.config.mouse_scale, 1.5);
        assert!(!emulator.config.immediate);
    }

    #[test]
    fn test_batch_emulator() {
        let inner = match EnigoInputEmulator::new() {
            Ok(emulator) => emulator,
            Err(_) => return,
        };
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
        assert_eq!(
            EnigoInputEmulator::convert_mouse_button(MouseButton::Left),
            Button::Left
        );
        assert_eq!(
            EnigoInputEmulator::convert_mouse_button(MouseButton::Right),
            Button::Right
        );
        assert_eq!(
            EnigoInputEmulator::convert_mouse_button(MouseButton::Middle),
            Button::Middle
        );
    }

    #[test]
    fn test_keycode_conversion() {
        assert_eq!(
            EnigoInputEmulator::convert_keycode(KeyCode::Space),
            Some(Key::Space)
        );
        assert_eq!(
            EnigoInputEmulator::convert_keycode(KeyCode::Escape),
            Some(Key::Escape)
        );
        assert_eq!(
            EnigoInputEmulator::convert_keycode(KeyCode::Enter),
            Some(Key::Return)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_native_backend_is_preferred_when_available() {
        use crate::backend::InjectBackend;

        let backend = WindowsNativeInputEmulator::new();
        assert!(backend.is_ok());

        let emulator = backend.unwrap();
        // Emulator is now auto-activated on creation
        assert!(InputEmulator::is_active(&emulator));
        assert_eq!(
            InjectBackend::kind(&emulator),
            rshare_core::BackendKind::WindowsNative
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_native_new_activates_platform_emulator() {
        let emulator = WindowsNativeInputEmulator::new().unwrap();

        assert!(InputEmulator::is_active(&emulator));
        assert!(emulator.platform_emulator_is_active_for_test());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_native_inactive_inject_returns_error() {
        use crate::backend::InjectBackend;

        let mut emulator = WindowsNativeInputEmulator::new().unwrap();
        emulator.deactivate().unwrap();

        let result = InjectBackend::inject(&mut emulator, InputEvent::mouse_move(10, 10));

        assert!(result.is_err());
    }
}
