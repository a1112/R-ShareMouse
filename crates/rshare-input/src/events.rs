//! Input event types

use serde::{Deserialize, Serialize};

/// Input event that can be sent between devices
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove {
        x: i32,
        y: i32,
    },
    MouseButton {
        button: MouseButton,
        state: ButtonState,
    },
    MouseWheel {
        delta_x: i32,
        delta_y: i32,
    },
    Key {
        keycode: KeyCode,
        state: ButtonState,
    },
    KeyExtended {
        keycode: KeyCode,
        state: ButtonState,
        shift: bool,
        ctrl: bool,
        alt: bool,
        meta: bool,
    },
}

/// Mouse button
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
    Other(u8),
}

impl MouseButton {
    /// Convert to platform-specific button code
    pub fn to_code(&self) -> u8 {
        match self {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
            MouseButton::Back => 4,
            MouseButton::Forward => 5,
            MouseButton::Other(n) => *n,
        }
    }

    /// Create from platform-specific button code
    pub fn from_code(code: u8) -> Self {
        match code {
            1 => MouseButton::Left,
            2 => MouseButton::Middle,
            3 => MouseButton::Right,
            4 => MouseButton::Back,
            5 => MouseButton::Forward,
            n => MouseButton::Other(n),
        }
    }
}

/// Button state (pressed or released)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonState {
    Pressed,
    Released,
}

impl ButtonState {
    pub fn is_pressed(&self) -> bool {
        matches!(self, ButtonState::Pressed)
    }

    pub fn is_released(&self) -> bool {
        matches!(self, ButtonState::Released)
    }

    /// Convert to boolean (true = pressed)
    pub fn as_bool(&self) -> bool {
        self.is_pressed()
    }
}

/// Key code (platform-independent representation)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyCode {
    /// Alphanumeric key (A-Z, 0-9)
    Char(u8),

    /// Special key
    Escape,
    Enter,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,

    /// Arrow keys
    Up,
    Down,
    Left,
    Right,

    /// Modifier keys
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    SuperLeft,
    SuperRight,

    /// Function keys
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,

    /// Space
    Space,

    /// Caps Lock
    CapsLock,
    NumLock,

    /// Keypad
    Keypad0,
    Keypad1,
    Keypad2,
    Keypad3,
    Keypad4,
    Keypad5,
    Keypad6,
    Keypad7,
    Keypad8,
    Keypad9,
    KeypadAdd,
    KeypadSubtract,
    KeypadMultiply,
    KeypadDivide,
    KeypadDecimal,
    KeypadEnter,

    /// Platform-specific key code
    Raw(u32),
}

impl KeyCode {
    /// Convert to platform-specific key code
    pub fn to_raw(&self) -> u32 {
        match self {
            KeyCode::Char(c) => *c as u32,
            KeyCode::Raw(r) => *r,
            _ => self.default_raw_code(),
        }
    }

    fn default_raw_code(&self) -> u32 {
        // Default mapping (can be overridden by platform code)
        match self {
            KeyCode::Escape => 0x1B,
            KeyCode::Enter => 0x0D,
            KeyCode::Tab => 0x09,
            KeyCode::Backspace => 0x08,
            KeyCode::Delete => 0x2E,
            KeyCode::Insert => 0x2D,
            KeyCode::Home => 0x24,
            KeyCode::End => 0x23,
            KeyCode::PageUp => 0x21,
            KeyCode::PageDown => 0x22,
            KeyCode::Up => 0x26,
            KeyCode::Down => 0x28,
            KeyCode::Left => 0x25,
            KeyCode::Right => 0x27,
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
            KeyCode::Space => 0x20,
            _ => 0,
        }
    }
}

impl InputEvent {
    pub fn mouse_move(x: i32, y: i32) -> Self {
        Self::MouseMove { x, y }
    }

    pub fn mouse_button(button: MouseButton, state: ButtonState) -> Self {
        Self::MouseButton { button, state }
    }

    pub fn mouse_wheel(delta_x: i32, delta_y: i32) -> Self {
        Self::MouseWheel { delta_x, delta_y }
    }

    pub fn key(keycode: KeyCode, state: ButtonState) -> Self {
        Self::Key { keycode, state }
    }

    pub fn key_extended(
        keycode: KeyCode,
        state: ButtonState,
        shift: bool,
        ctrl: bool,
        alt: bool,
        meta: bool,
    ) -> Self {
        Self::KeyExtended {
            keycode,
            state,
            shift,
            ctrl,
            alt,
            meta,
        }
    }

    /// Get the event type as a string for logging
    pub fn event_type(&self) -> &'static str {
        match self {
            InputEvent::MouseMove { .. } => "MouseMove",
            InputEvent::MouseButton { .. } => "MouseButton",
            InputEvent::MouseWheel { .. } => "MouseWheel",
            InputEvent::Key { .. } => "Key",
            InputEvent::KeyExtended { .. } => "KeyExtended",
        }
    }

    /// Check if this event should be forwarded to remote device
    pub fn should_forward(&self) -> bool {
        matches!(
            self,
            InputEvent::MouseMove { .. }
                | InputEvent::MouseButton { .. }
                | InputEvent::MouseWheel { .. }
                | InputEvent::Key { .. }
                | InputEvent::KeyExtended { .. }
        )
    }

    /// Convert a native macOS platform event into the cross-platform event type.
    #[cfg(target_os = "macos")]
    pub fn from_macos_event(event: rshare_platform::MacosInputEvent) -> Self {
        match event {
            rshare_platform::MacosInputEvent::MouseMove { x, y } => Self::mouse_move(x, y),
            rshare_platform::MacosInputEvent::MouseButton { button, down } => {
                let state = if down {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                Self::mouse_button(MouseButton::from_code(button), state)
            }
            rshare_platform::MacosInputEvent::MouseWheel { delta_x, delta_y } => {
                Self::mouse_wheel(delta_x, delta_y)
            }
            rshare_platform::MacosInputEvent::Key { keycode, down } => {
                let state = if down {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                Self::key(KeyCode::Raw(keycode), state)
            }
        }
    }

    /// Convert a native Windows low-level hook event into the cross-platform event type.
    #[cfg(target_os = "windows")]
    pub fn from_windows_event(event: rshare_platform::WindowsInputEvent) -> Self {
        match event {
            rshare_platform::WindowsInputEvent::MouseMove { x, y } => Self::mouse_move(x, y),
            rshare_platform::WindowsInputEvent::MouseButton { button, down } => {
                let state = if down {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                Self::mouse_button(MouseButton::from_code(button), state)
            }
            rshare_platform::WindowsInputEvent::MouseWheel { delta_x, delta_y } => {
                Self::mouse_wheel(delta_x, delta_y)
            }
            rshare_platform::WindowsInputEvent::Key { vk, down } => {
                let state = if down {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                Self::key(KeyCode::Raw(vk), state)
            }
        }
    }
}

/// Convert platform event to InputEvent
pub trait FromPlatformEvent {
    fn from_platform_event(event: PlatformEvent) -> Option<InputEvent>;
}

/// Platform-specific event (received from rdev or platform hooks)
#[derive(Debug, Clone)]
pub enum PlatformEvent {
    MouseEvent {
        event_type: MouseEventType,
        x: i32,
        y: i32,
        button: Option<MouseButton>,
    },
    KeyEvent {
        keycode: u32,
        state: ButtonState,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum MouseEventType {
    Move,
    ButtonPress,
    ButtonRelease,
    Wheel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mouse_button_codes() {
        assert_eq!(MouseButton::Left.to_code(), 1);
        assert_eq!(MouseButton::from_code(1), MouseButton::Left);
        assert_eq!(MouseButton::from_code(2), MouseButton::Middle);
    }

    #[test]
    fn test_button_state() {
        assert!(ButtonState::Pressed.is_pressed());
        assert!(!ButtonState::Pressed.is_released());
        assert!(ButtonState::Released.is_released());
        assert!(ButtonState::Released.as_bool() == false);
    }

    #[test]
    fn test_input_event_creation() {
        let event = InputEvent::mouse_move(100, 200);
        match event {
            InputEvent::MouseMove { x, y } => {
                assert_eq!(x, 100);
                assert_eq!(y, 200);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_event_serialization() {
        let event = InputEvent::key(KeyCode::Space, ButtonState::Pressed);
        let serialized = serde_json::to_string(&event).unwrap();
        let deserialized: InputEvent = serde_json::from_str(&serialized).unwrap();
        match deserialized {
            InputEvent::Key { keycode, state } => {
                assert_eq!(keycode, KeyCode::Space);
                assert_eq!(state, ButtonState::Pressed);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_keycode_raw() {
        assert_eq!(KeyCode::Space.to_raw(), 0x20);
        assert_eq!(KeyCode::Raw(123).to_raw(), 123);
    }

    #[test]
    fn test_should_forward() {
        assert!(InputEvent::mouse_move(0, 0).should_forward());
        assert!(InputEvent::key(KeyCode::Space, ButtonState::Pressed).should_forward());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_event_conversion() {
        let event = InputEvent::from_windows_event(rshare_platform::WindowsInputEvent::Key {
            vk: 0x20,
            down: true,
        });

        match event {
            InputEvent::Key { keycode, state } => {
                assert_eq!(keycode, KeyCode::Raw(0x20));
                assert_eq!(state, ButtonState::Pressed);
            }
            _ => panic!("Wrong event type"),
        }
    }
}
