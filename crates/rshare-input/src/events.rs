//! Input event types

use serde::{Deserialize, Serialize};

pub use rshare_core::{GamepadButton, GamepadButtonState, GamepadDeviceInfo, GamepadState};

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
    TextCommit {
        text: String,
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
        state_after: GamepadState,
    },
    GamepadState {
        state: GamepadState,
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

pub const RSHARE_KEYPAD_ENTER_RAW: u32 = 0xE01C;
/// Canonical wire value for the ISO 102nd key (evdev `KEY_102ND`).
///
/// `VK_OEM_102` is distinct from the regular backslash key (`0xDC`).
pub const RSHARE_ISO_102ND_RAW: u32 = 0xE2;

/// Return whether a raw value belongs to the canonical wire-key namespace.
///
/// Raw values are deliberately a small, explicit set. They cover keys that
/// have no `KeyCode` variant but are represented by stable Windows virtual-key
/// values across the protocol. Native injectors must not treat arbitrary wire
/// values as platform keycodes.
pub const fn is_canonical_raw_keycode(raw: u32) -> bool {
    matches!(
        raw,
        RSHARE_KEYPAD_ENTER_RAW
            | 0x13 // VK_PAUSE
            | 0x2C // VK_SNAPSHOT / PrintScreen
            | 0x91 // VK_SCROLL
            | 0xBA // VK_OEM_1 / ;
            | 0xBB // VK_OEM_PLUS / =
            | 0xBC // VK_OEM_COMMA
            | 0xBD // VK_OEM_MINUS
            | 0xBE // VK_OEM_PERIOD
            | 0xBF // VK_OEM_2 / /
            | 0xC0 // VK_OEM_3 / `
            | 0xDB // VK_OEM_4 / [
            | 0xDC // VK_OEM_5 / \
            | 0xDD // VK_OEM_6 / ]
            | 0xDE // VK_OEM_7 / '
            | RSHARE_ISO_102ND_RAW // VK_OEM_102
    )
}

impl KeyCode {
    /// Convert to the canonical wire key namespace.
    ///
    /// This namespace is intentionally platform-neutral: ASCII uppercase
    /// letters and digits use their byte values, special keys use the
    /// Windows-VK-like values below, punctuation uses the canonical values
    /// in the `0xBA..=0xDE` range, and keypad keys use the existing values
    /// (including [`RSHARE_KEYPAD_ENTER_RAW`]). Native hardware keycodes must
    /// be normalized before they reach this boundary.
    pub fn to_raw(&self) -> u32 {
        match self {
            KeyCode::Char(c) => *c as u32,
            KeyCode::Raw(r) => *r,
            _ => self.default_raw_code(),
        }
    }

    /// Restore a semantic key from the canonical wire key namespace.
    ///
    /// Unknown values remain [`KeyCode::Raw`] for wire compatibility. Native
    /// backends must validate those values before attempting injection.
    pub fn from_wire(raw: u32) -> Self {
        match raw {
            0x30..=0x39 | 0x41..=0x5A => Self::Char(raw as u8),
            0x08 => Self::Backspace,
            0x09 => Self::Tab,
            0x0D => Self::Enter,
            0x1B => Self::Escape,
            0x2E => Self::Delete,
            0x2D => Self::Insert,
            0x24 => Self::Home,
            0x23 => Self::End,
            0x21 => Self::PageUp,
            0x22 => Self::PageDown,
            0x26 => Self::Up,
            0x28 => Self::Down,
            0x25 => Self::Left,
            0x27 => Self::Right,
            0x70 => Self::F1,
            0x71 => Self::F2,
            0x72 => Self::F3,
            0x73 => Self::F4,
            0x74 => Self::F5,
            0x75 => Self::F6,
            0x76 => Self::F7,
            0x77 => Self::F8,
            0x78 => Self::F9,
            0x79 => Self::F10,
            0x7A => Self::F11,
            0x7B => Self::F12,
            0x20 => Self::Space,
            0xA0 => Self::ShiftLeft,
            0xA1 => Self::ShiftRight,
            0xA2 => Self::ControlLeft,
            0xA3 => Self::ControlRight,
            0xA4 => Self::AltLeft,
            0xA5 => Self::AltRight,
            0x5B => Self::SuperLeft,
            0x5C => Self::SuperRight,
            0x14 => Self::CapsLock,
            0x90 => Self::NumLock,
            0x60 => Self::Keypad0,
            0x61 => Self::Keypad1,
            0x62 => Self::Keypad2,
            0x63 => Self::Keypad3,
            0x64 => Self::Keypad4,
            0x65 => Self::Keypad5,
            0x66 => Self::Keypad6,
            0x67 => Self::Keypad7,
            0x68 => Self::Keypad8,
            0x69 => Self::Keypad9,
            0x6A => Self::KeypadMultiply,
            0x6B => Self::KeypadAdd,
            RSHARE_KEYPAD_ENTER_RAW => Self::KeypadEnter,
            0x6D => Self::KeypadSubtract,
            0x6E => Self::KeypadDecimal,
            0x6F => Self::KeypadDivide,
            0xBA | 0xBB | 0xBC | 0xBD | 0xBE | 0xBF | 0xC0 | 0xDB | 0xDC | 0xDD | 0xDE => {
                Self::Raw(raw)
            }
            _ => Self::Raw(raw),
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
            KeyCode::ShiftLeft => 0xA0,
            KeyCode::ShiftRight => 0xA1,
            KeyCode::ControlLeft => 0xA2,
            KeyCode::ControlRight => 0xA3,
            KeyCode::AltLeft => 0xA4,
            KeyCode::AltRight => 0xA5,
            KeyCode::SuperLeft => 0x5B,
            KeyCode::SuperRight => 0x5C,
            KeyCode::CapsLock => 0x14,
            KeyCode::NumLock => 0x90,
            KeyCode::Keypad0 => 0x60,
            KeyCode::Keypad1 => 0x61,
            KeyCode::Keypad2 => 0x62,
            KeyCode::Keypad3 => 0x63,
            KeyCode::Keypad4 => 0x64,
            KeyCode::Keypad5 => 0x65,
            KeyCode::Keypad6 => 0x66,
            KeyCode::Keypad7 => 0x67,
            KeyCode::Keypad8 => 0x68,
            KeyCode::Keypad9 => 0x69,
            KeyCode::KeypadMultiply => 0x6A,
            KeyCode::KeypadAdd => 0x6B,
            KeyCode::KeypadEnter => RSHARE_KEYPAD_ENTER_RAW,
            KeyCode::KeypadSubtract => 0x6D,
            KeyCode::KeypadDecimal => 0x6E,
            KeyCode::KeypadDivide => 0x6F,
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

    pub fn text_commit(text: String) -> Self {
        Self::TextCommit { text }
    }

    pub fn gamepad_connected(info: GamepadDeviceInfo) -> Self {
        Self::GamepadConnected { info }
    }

    pub fn gamepad_disconnected(gamepad_id: u8) -> Self {
        Self::GamepadDisconnected { gamepad_id }
    }

    pub fn gamepad_button(
        gamepad_id: u8,
        button: GamepadButton,
        pressed: bool,
        state_after: GamepadState,
    ) -> Self {
        Self::GamepadButton {
            gamepad_id,
            button,
            pressed,
            state_after,
        }
    }

    pub fn gamepad_state(state: GamepadState) -> Self {
        Self::GamepadState { state }
    }

    /// Get the event type as a string for logging
    pub fn event_type(&self) -> &'static str {
        match self {
            InputEvent::MouseMove { .. } => "MouseMove",
            InputEvent::MouseButton { .. } => "MouseButton",
            InputEvent::MouseWheel { .. } => "MouseWheel",
            InputEvent::Key { .. } => "Key",
            InputEvent::KeyExtended { .. } => "KeyExtended",
            InputEvent::TextCommit { .. } => "TextCommit",
            InputEvent::GamepadConnected { .. } => "GamepadConnected",
            InputEvent::GamepadDisconnected { .. } => "GamepadDisconnected",
            InputEvent::GamepadButton { .. } => "GamepadButton",
            InputEvent::GamepadState { .. } => "GamepadState",
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
                | InputEvent::TextCommit { .. }
                | InputEvent::GamepadConnected { .. }
                | InputEvent::GamepadDisconnected { .. }
                | InputEvent::GamepadButton { .. }
                | InputEvent::GamepadState { .. }
        )
    }

    /// Convert a native macOS platform event into the cross-platform event type.
    #[cfg(target_os = "macos")]
    pub fn from_macos_event(event: rshare_platform::MacosInputEvent) -> Option<Self> {
        match event {
            rshare_platform::MacosInputEvent::MouseMove { x, y } => Some(Self::mouse_move(x, y)),
            rshare_platform::MacosInputEvent::MouseButton { button, down } => {
                let state = if down {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                Some(Self::mouse_button(MouseButton::from_code(button), state))
            }
            rshare_platform::MacosInputEvent::MouseWheel { delta_x, delta_y } => {
                Some(Self::mouse_wheel(delta_x, delta_y))
            }
            rshare_platform::MacosInputEvent::Key { keycode, down } => {
                let state = if down {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                Some(Self::key(key_code_from_macos_keycode(keycode)?, state))
            }
        }
    }

    /// Convert a native Windows low-level hook event into the cross-platform event type.
    #[cfg(target_os = "windows")]
    pub fn from_windows_event(event: rshare_platform::WindowsInputEvent) -> Option<Self> {
        Some(match event {
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
            rshare_platform::WindowsInputEvent::Key {
                vk,
                scan_code,
                flags,
                down,
            } => {
                let state = if down {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                Self::key(key_code_from_windows_hook_key(vk, scan_code, flags)?, state)
            }
        })
    }

    /// Convert a Windows driver event (from kernel filter driver) to InputEvent.
    #[cfg(target_os = "windows")]
    pub fn from_windows_driver_event(
        event: rshare_platform::windows::WindowsDriverInputEvent,
    ) -> Option<Self> {
        use rshare_platform::windows::WindowsDriverDeviceKind;
        use rshare_platform::windows::WindowsDriverEventKind;

        match (event.device_kind, event.event_kind) {
            (WindowsDriverDeviceKind::Keyboard, WindowsDriverEventKind::Key) => {
                let state = if event.value1 != 0 {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                let flags = if event.flags != 0 {
                    event.flags
                } else {
                    event.value2 as u32
                };
                Some(InputEvent::key(
                    key_code_from_windows_scan_code(event.value0 as u32, flags)?,
                    state,
                ))
            }
            (WindowsDriverDeviceKind::Mouse, WindowsDriverEventKind::MouseMove) => {
                Some(InputEvent::mouse_move(event.value0, event.value1))
            }
            (WindowsDriverDeviceKind::Mouse, WindowsDriverEventKind::MouseButton) => {
                let button = MouseButton::from_code(event.value0 as u8);
                let state = if event.value1 != 0 {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                Some(InputEvent::mouse_button(button, state))
            }
            (WindowsDriverDeviceKind::Mouse, WindowsDriverEventKind::MouseWheel) => {
                Some(InputEvent::mouse_wheel(event.value0, event.value1))
            }
            _ => None,
        }
    }

    /// Convert a Linux evdev driver event (from kernel input subsystem) to InputEvent.
    #[cfg(target_os = "linux")]
    pub fn from_evdev_driver_event(event: rshare_platform::EvdevDriverEvent) -> Option<Self> {
        match event {
            rshare_platform::EvdevDriverEvent::MouseMove { x, y, .. } => {
                Some(InputEvent::mouse_move(x, y))
            }
            rshare_platform::EvdevDriverEvent::MouseButton {
                button, pressed, ..
            } => {
                let state = if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                Some(InputEvent::mouse_button(
                    MouseButton::from_code(button as u8),
                    state,
                ))
            }
            rshare_platform::EvdevDriverEvent::MouseWheel {
                delta_x, delta_y, ..
            } => Some(InputEvent::mouse_wheel(delta_x, delta_y)),
            rshare_platform::EvdevDriverEvent::Key {
                keycode, pressed, ..
            } => {
                let state = if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                Some(InputEvent::key(
                    linux_evdev_keycode_to_keycode(keycode)?,
                    state,
                ))
            }
        }
    }
}

/// Normalize a Linux evdev key code into the canonical wire namespace.
///
/// evdev values are Linux-native scan codes, not wire values. In particular,
/// `KEY_D` (32) and `KEY_SPACE` (57) collide with canonical wire values for
/// unrelated keys if forwarded as [`KeyCode::Raw`]. Keep this table explicit
/// and return `None` for keys without a stable representation instead.
pub fn linux_evdev_keycode_to_keycode(keycode: u32) -> Option<KeyCode> {
    let semantic = match keycode {
        1 => KeyCode::Escape,
        2..=11 => KeyCode::Char(b"1234567890"[(keycode - 2) as usize]),
        12 => KeyCode::Raw(0xBD), // KEY_MINUS
        13 => KeyCode::Raw(0xBB), // KEY_EQUAL
        14 => KeyCode::Backspace,
        15 => KeyCode::Tab,
        16..=25 => KeyCode::Char(b"QWERTYUIOP"[(keycode - 16) as usize]),
        26 => KeyCode::Raw(0xDB), // KEY_LEFTBRACE
        27 => KeyCode::Raw(0xDD), // KEY_RIGHTBRACE
        28 => KeyCode::Enter,
        29 => KeyCode::ControlLeft,
        30..=38 => KeyCode::Char(b"ASDFGHJKL"[(keycode - 30) as usize]),
        39 => KeyCode::Raw(0xBA), // KEY_SEMICOLON
        40 => KeyCode::Raw(0xDE), // KEY_APOSTROPHE
        41 => KeyCode::Raw(0xC0), // KEY_GRAVE
        42 => KeyCode::ShiftLeft,
        43 => KeyCode::Raw(0xDC), // KEY_BACKSLASH
        44..=50 => KeyCode::Char(b"ZXCVBNM"[(keycode - 44) as usize]),
        51 => KeyCode::Raw(0xBC), // KEY_COMMA
        52 => KeyCode::Raw(0xBE), // KEY_DOT
        53 => KeyCode::Raw(0xBF), // KEY_SLASH
        54 => KeyCode::ShiftRight,
        55 => KeyCode::KeypadMultiply,
        56 => KeyCode::AltLeft,
        57 => KeyCode::Space,
        58 => KeyCode::CapsLock,
        59..=68 => match keycode {
            59 => KeyCode::F1,
            60 => KeyCode::F2,
            61 => KeyCode::F3,
            62 => KeyCode::F4,
            63 => KeyCode::F5,
            64 => KeyCode::F6,
            65 => KeyCode::F7,
            66 => KeyCode::F8,
            67 => KeyCode::F9,
            68 => KeyCode::F10,
            _ => unreachable!("F-key range is exhaustive"),
        },
        69 => KeyCode::NumLock,
        70 => KeyCode::Raw(0x91), // KEY_SCROLLLOCK
        71..=82 => match keycode {
            71 => KeyCode::Keypad7,
            72 => KeyCode::Keypad8,
            73 => KeyCode::Keypad9,
            74 => KeyCode::KeypadSubtract,
            75 => KeyCode::Keypad4,
            76 => KeyCode::Keypad5,
            77 => KeyCode::Keypad6,
            78 => KeyCode::KeypadAdd,
            79 => KeyCode::Keypad1,
            80 => KeyCode::Keypad2,
            81 => KeyCode::Keypad3,
            82 => KeyCode::Keypad0,
            _ => unreachable!("keypad range is exhaustive"),
        },
        83 => KeyCode::KeypadDecimal,
        86 => KeyCode::Raw(RSHARE_ISO_102ND_RAW), // KEY_102ND
        87 => KeyCode::F11,
        88 => KeyCode::F12,
        96 => KeyCode::KeypadEnter,
        97 => KeyCode::ControlRight,
        98 => KeyCode::KeypadDivide,
        99 | 210 => KeyCode::Raw(0x2C), // KEY_SYSRQ / KEY_PRINT
        100 => KeyCode::AltRight,
        102 => KeyCode::Home,
        103 => KeyCode::Up,
        104 => KeyCode::PageUp,
        105 => KeyCode::Left,
        106 => KeyCode::Right,
        107 => KeyCode::End,
        108 => KeyCode::Down,
        109 => KeyCode::PageDown,
        110 => KeyCode::Insert,
        111 => KeyCode::Delete,
        119 => KeyCode::Raw(0x13), // KEY_PAUSE
        125 => KeyCode::SuperLeft,
        126 => KeyCode::SuperRight,
        _ => return None,
    };
    Some(semantic)
}

/// Convert a canonical key into the Linux evdev code required by uinput.
///
/// This is deliberately the inverse of [`linux_evdev_keycode_to_keycode`],
/// rather than `KeyCode::to_raw()`: canonical wire values must never be sent
/// to uinput as though they were native Linux codes.
pub fn keycode_to_linux_evdev_keycode(keycode: KeyCode) -> Option<u32> {
    Some(match keycode {
        KeyCode::Char(b'1') => 2,
        KeyCode::Char(b'2') => 3,
        KeyCode::Char(b'3') => 4,
        KeyCode::Char(b'4') => 5,
        KeyCode::Char(b'5') => 6,
        KeyCode::Char(b'6') => 7,
        KeyCode::Char(b'7') => 8,
        KeyCode::Char(b'8') => 9,
        KeyCode::Char(b'9') => 10,
        KeyCode::Char(b'0') => 11,
        KeyCode::Char(b'Q') => 16,
        KeyCode::Char(b'W') => 17,
        KeyCode::Char(b'E') => 18,
        KeyCode::Char(b'R') => 19,
        KeyCode::Char(b'T') => 20,
        KeyCode::Char(b'Y') => 21,
        KeyCode::Char(b'U') => 22,
        KeyCode::Char(b'I') => 23,
        KeyCode::Char(b'O') => 24,
        KeyCode::Char(b'P') => 25,
        KeyCode::Char(b'A') => 30,
        KeyCode::Char(b'S') => 31,
        KeyCode::Char(b'D') => 32,
        KeyCode::Char(b'F') => 33,
        KeyCode::Char(b'G') => 34,
        KeyCode::Char(b'H') => 35,
        KeyCode::Char(b'J') => 36,
        KeyCode::Char(b'K') => 37,
        KeyCode::Char(b'L') => 38,
        KeyCode::Char(b'Z') => 44,
        KeyCode::Char(b'X') => 45,
        KeyCode::Char(b'C') => 46,
        KeyCode::Char(b'V') => 47,
        KeyCode::Char(b'B') => 48,
        KeyCode::Char(b'N') => 49,
        KeyCode::Char(b'M') => 50,
        KeyCode::Escape => 1,
        KeyCode::Backspace => 14,
        KeyCode::Tab => 15,
        KeyCode::Enter => 28,
        KeyCode::Space => 57,
        KeyCode::CapsLock => 58,
        KeyCode::NumLock => 69,
        KeyCode::ShiftLeft => 42,
        KeyCode::ShiftRight => 54,
        KeyCode::ControlLeft => 29,
        KeyCode::ControlRight => 97,
        KeyCode::AltLeft => 56,
        KeyCode::AltRight => 100,
        KeyCode::SuperLeft => 125,
        KeyCode::SuperRight => 126,
        KeyCode::F1 => 59,
        KeyCode::F2 => 60,
        KeyCode::F3 => 61,
        KeyCode::F4 => 62,
        KeyCode::F5 => 63,
        KeyCode::F6 => 64,
        KeyCode::F7 => 65,
        KeyCode::F8 => 66,
        KeyCode::F9 => 67,
        KeyCode::F10 => 68,
        KeyCode::F11 => 87,
        KeyCode::F12 => 88,
        KeyCode::Home => 102,
        KeyCode::End => 107,
        KeyCode::PageUp => 104,
        KeyCode::PageDown => 109,
        KeyCode::Up => 103,
        KeyCode::Down => 108,
        KeyCode::Left => 105,
        KeyCode::Right => 106,
        KeyCode::Insert => 110,
        KeyCode::Delete => 111,
        KeyCode::Keypad0 => 82,
        KeyCode::Keypad1 => 79,
        KeyCode::Keypad2 => 80,
        KeyCode::Keypad3 => 81,
        KeyCode::Keypad4 => 75,
        KeyCode::Keypad5 => 76,
        KeyCode::Keypad6 => 77,
        KeyCode::Keypad7 => 71,
        KeyCode::Keypad8 => 72,
        KeyCode::Keypad9 => 73,
        KeyCode::KeypadAdd => 78,
        KeyCode::KeypadSubtract => 74,
        KeyCode::KeypadMultiply => 55,
        KeyCode::KeypadDivide => 98,
        KeyCode::KeypadDecimal => 83,
        KeyCode::KeypadEnter => 96,
        KeyCode::Raw(0xBD) => 12,
        KeyCode::Raw(0xBB) => 13,
        KeyCode::Raw(0xDB) => 26,
        KeyCode::Raw(0xDD) => 27,
        KeyCode::Raw(0xBA) => 39,
        KeyCode::Raw(0xDE) => 40,
        KeyCode::Raw(0xC0) => 41,
        KeyCode::Raw(0xDC) => 43,
        KeyCode::Raw(RSHARE_ISO_102ND_RAW) => 86,
        KeyCode::Raw(0xBC) => 51,
        KeyCode::Raw(0xBE) => 52,
        KeyCode::Raw(0xBF) => 53,
        KeyCode::Raw(0x91) => 70,
        KeyCode::Raw(0x2C) => 99,
        KeyCode::Raw(0x13) => 119,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
fn key_code_from_macos_keycode(keycode: u32) -> Option<KeyCode> {
    let semantic = match keycode {
        0x00 => KeyCode::Char(b'A'),
        0x01 => KeyCode::Char(b'S'),
        0x02 => KeyCode::Char(b'D'),
        0x03 => KeyCode::Char(b'F'),
        0x04 => KeyCode::Char(b'H'),
        0x05 => KeyCode::Char(b'G'),
        0x06 => KeyCode::Char(b'Z'),
        0x07 => KeyCode::Char(b'X'),
        0x08 => KeyCode::Char(b'C'),
        0x09 => KeyCode::Char(b'V'),
        0x0A => KeyCode::Raw(RSHARE_ISO_102ND_RAW),
        0x0B => KeyCode::Char(b'B'),
        0x0C => KeyCode::Char(b'Q'),
        0x0D => KeyCode::Char(b'W'),
        0x0E => KeyCode::Char(b'E'),
        0x0F => KeyCode::Char(b'R'),
        0x10 => KeyCode::Char(b'Y'),
        0x11 => KeyCode::Char(b'T'),
        0x12 => KeyCode::Char(b'1'),
        0x13 => KeyCode::Char(b'2'),
        0x14 => KeyCode::Char(b'3'),
        0x15 => KeyCode::Char(b'4'),
        0x16 => KeyCode::Char(b'6'),
        0x17 => KeyCode::Char(b'5'),
        0x19 => KeyCode::Char(b'9'),
        0x1A => KeyCode::Char(b'7'),
        0x1C => KeyCode::Char(b'8'),
        0x1D => KeyCode::Char(b'0'),
        0x1F => KeyCode::Char(b'O'),
        0x20 => KeyCode::Char(b'U'),
        0x22 => KeyCode::Char(b'I'),
        0x23 => KeyCode::Char(b'P'),
        0x25 => KeyCode::Char(b'L'),
        0x26 => KeyCode::Char(b'J'),
        0x28 => KeyCode::Char(b'K'),
        0x2D => KeyCode::Char(b'N'),
        0x2E => KeyCode::Char(b'M'),
        0x18 => KeyCode::Raw(0xBB),
        0x1B => KeyCode::Raw(0xBD),
        0x1E => KeyCode::Raw(0xDD),
        0x21 => KeyCode::Raw(0xDB),
        0x27 => KeyCode::Raw(0xDE),
        0x29 => KeyCode::Raw(0xBA),
        0x2A => KeyCode::Raw(0xDC),
        0x2B => KeyCode::Raw(0xBC),
        0x2C => KeyCode::Raw(0xBF),
        0x2F => KeyCode::Raw(0xBE),
        0x32 => KeyCode::Raw(0xC0),
        0x24 => KeyCode::Enter,
        0x30 => KeyCode::Tab,
        0x31 => KeyCode::Space,
        0x33 => KeyCode::Backspace,
        0x35 => KeyCode::Escape,
        0x38 => KeyCode::ShiftLeft,
        0x3C => KeyCode::ShiftRight,
        0x3B => KeyCode::ControlLeft,
        0x3E => KeyCode::ControlRight,
        0x3A => KeyCode::AltLeft,
        0x3D => KeyCode::AltRight,
        0x37 => KeyCode::SuperLeft,
        0x36 => KeyCode::SuperRight,
        0x39 => KeyCode::CapsLock,
        0x7A => KeyCode::F1,
        0x78 => KeyCode::F2,
        0x63 => KeyCode::F3,
        0x76 => KeyCode::F4,
        0x60 => KeyCode::F5,
        0x61 => KeyCode::F6,
        0x62 => KeyCode::F7,
        0x64 => KeyCode::F8,
        0x65 => KeyCode::F9,
        0x6D => KeyCode::F10,
        0x67 => KeyCode::F11,
        0x6F => KeyCode::F12,
        0x7B => KeyCode::Left,
        0x7C => KeyCode::Right,
        0x7E => KeyCode::Up,
        0x7D => KeyCode::Down,
        0x73 => KeyCode::Home,
        0x77 => KeyCode::End,
        0x74 => KeyCode::PageUp,
        0x79 => KeyCode::PageDown,
        0x75 => KeyCode::Delete,
        0x72 => KeyCode::Insert,
        0x47 => KeyCode::NumLock,
        0x52 => KeyCode::Keypad0,
        0x53 => KeyCode::Keypad1,
        0x54 => KeyCode::Keypad2,
        0x55 => KeyCode::Keypad3,
        0x56 => KeyCode::Keypad4,
        0x57 => KeyCode::Keypad5,
        0x58 => KeyCode::Keypad6,
        0x59 => KeyCode::Keypad7,
        0x5B => KeyCode::Keypad8,
        0x5C => KeyCode::Keypad9,
        0x45 => KeyCode::KeypadAdd,
        0x4E => KeyCode::KeypadSubtract,
        0x43 => KeyCode::KeypadMultiply,
        0x4B => KeyCode::KeypadDivide,
        0x41 => KeyCode::KeypadDecimal,
        0x4C => KeyCode::KeypadEnter,
        // Fn (0x3F), F13+, JIS-only, and media/system keys have no
        // cross-platform semantic representation. Dropping them here avoids
        // publishing native values that collide with canonical wire codes.
        _ => return None,
    };
    Some(semantic)
}

#[cfg(target_os = "windows")]
fn key_code_from_windows_hook_key(vk: u32, scan_code: u32, flags: u32) -> Option<KeyCode> {
    const LLKHF_EXTENDED: u32 = 0x01;
    let extended = (flags & LLKHF_EXTENDED) != 0;

    // Low-level hooks can report generic modifier VKs. Retain their physical
    // identity only when the set-1 scan code and extended bit agree; otherwise
    // fail closed rather than forwarding an ambiguous modifier.
    match vk {
        0x10 => match scan_code {
            0x2A => return Some(KeyCode::ShiftLeft),
            0x36 => return Some(KeyCode::ShiftRight),
            _ => return None,
        },
        0x11 if scan_code == 0x1D => {
            return Some(if extended {
                KeyCode::ControlRight
            } else {
                KeyCode::ControlLeft
            });
        }
        0x11 => return None,
        0x12 if scan_code == 0x38 => {
            return Some(if extended {
                KeyCode::AltRight
            } else {
                KeyCode::AltLeft
            });
        }
        0x12 => return None,
        // Both Enter keys report VK_RETURN. Their scan code plus the extended
        // bit is the physical identity, so inconsistent hook metadata is not
        // allowed to fall back to an ordinary Enter.
        0x0D => match (scan_code, extended) {
            (0x1C, false) => return Some(KeyCode::Enter),
            (0x1C, true) => return Some(KeyCode::KeypadEnter),
            _ => return None,
        },
        _ => {}
    }

    // Explicit left/right modifier VKs keep their existing direct mapping.
    key_code_from_windows_vk(vk)
}

#[cfg(target_os = "windows")]
fn key_code_from_windows_vk(vk: u32) -> Option<KeyCode> {
    Some(match vk {
        0x30..=0x39 | 0x41..=0x5A => KeyCode::Char(vk as u8),
        0x08 => KeyCode::Backspace,
        0x09 => KeyCode::Tab,
        0x0D => KeyCode::Enter,
        0x10 | 0xA0 => KeyCode::ShiftLeft,
        0xA1 => KeyCode::ShiftRight,
        0x11 | 0xA2 => KeyCode::ControlLeft,
        0xA3 => KeyCode::ControlRight,
        0x12 | 0xA4 => KeyCode::AltLeft,
        0xA5 => KeyCode::AltRight,
        0x14 => KeyCode::CapsLock,
        0x1B => KeyCode::Escape,
        0x20 => KeyCode::Space,
        0x21 => KeyCode::PageUp,
        0x22 => KeyCode::PageDown,
        0x23 => KeyCode::End,
        0x24 => KeyCode::Home,
        0x25 => KeyCode::Left,
        0x26 => KeyCode::Up,
        0x27 => KeyCode::Right,
        0x28 => KeyCode::Down,
        0x2D => KeyCode::Insert,
        0x2E => KeyCode::Delete,
        0x5B => KeyCode::SuperLeft,
        0x5C => KeyCode::SuperRight,
        0x60 => KeyCode::Keypad0,
        0x61 => KeyCode::Keypad1,
        0x62 => KeyCode::Keypad2,
        0x63 => KeyCode::Keypad3,
        0x64 => KeyCode::Keypad4,
        0x65 => KeyCode::Keypad5,
        0x66 => KeyCode::Keypad6,
        0x67 => KeyCode::Keypad7,
        0x68 => KeyCode::Keypad8,
        0x69 => KeyCode::Keypad9,
        0x6A => KeyCode::KeypadMultiply,
        0x6B => KeyCode::KeypadAdd,
        0x6D => KeyCode::KeypadSubtract,
        0x6E => KeyCode::KeypadDecimal,
        0x6F => KeyCode::KeypadDivide,
        0x70 => KeyCode::F1,
        0x71 => KeyCode::F2,
        0x72 => KeyCode::F3,
        0x73 => KeyCode::F4,
        0x74 => KeyCode::F5,
        0x75 => KeyCode::F6,
        0x76 => KeyCode::F7,
        0x77 => KeyCode::F8,
        0x78 => KeyCode::F9,
        0x79 => KeyCode::F10,
        0x7A => KeyCode::F11,
        0x7B => KeyCode::F12,
        0x90 => KeyCode::NumLock,
        raw if is_canonical_raw_keycode(raw) => KeyCode::Raw(raw),
        _ => return None,
    })
}

#[cfg(target_os = "windows")]
fn key_code_from_windows_scan_code(scan_code: u32, flags: u32) -> Option<KeyCode> {
    const KEY_E0: u32 = 0x02;
    let extended = (flags & KEY_E0) != 0;

    Some(match (scan_code, extended) {
        (0x01, _) => KeyCode::Escape,
        (0x02, _) => KeyCode::Char(b'1'),
        (0x03, _) => KeyCode::Char(b'2'),
        (0x04, _) => KeyCode::Char(b'3'),
        (0x05, _) => KeyCode::Char(b'4'),
        (0x06, _) => KeyCode::Char(b'5'),
        (0x07, _) => KeyCode::Char(b'6'),
        (0x08, _) => KeyCode::Char(b'7'),
        (0x09, _) => KeyCode::Char(b'8'),
        (0x0A, _) => KeyCode::Char(b'9'),
        (0x0B, _) => KeyCode::Char(b'0'),
        (0x0C, _) => KeyCode::Raw(0xBD),
        (0x0D, _) => KeyCode::Raw(0xBB),
        (0x0E, _) => KeyCode::Backspace,
        (0x0F, _) => KeyCode::Tab,
        (0x10, _) => KeyCode::Char(b'Q'),
        (0x11, _) => KeyCode::Char(b'W'),
        (0x12, _) => KeyCode::Char(b'E'),
        (0x13, _) => KeyCode::Char(b'R'),
        (0x14, _) => KeyCode::Char(b'T'),
        (0x15, _) => KeyCode::Char(b'Y'),
        (0x16, _) => KeyCode::Char(b'U'),
        (0x17, _) => KeyCode::Char(b'I'),
        (0x18, _) => KeyCode::Char(b'O'),
        (0x19, _) => KeyCode::Char(b'P'),
        (0x1A, _) => KeyCode::Raw(0xDB),
        (0x1B, _) => KeyCode::Raw(0xDD),
        (0x1C, false) => KeyCode::Enter,
        (0x1C, true) => KeyCode::KeypadEnter,
        (0x1D, false) => KeyCode::ControlLeft,
        (0x1D, true) => KeyCode::ControlRight,
        (0x1E, _) => KeyCode::Char(b'A'),
        (0x1F, _) => KeyCode::Char(b'S'),
        (0x20, _) => KeyCode::Char(b'D'),
        (0x21, _) => KeyCode::Char(b'F'),
        (0x22, _) => KeyCode::Char(b'G'),
        (0x23, _) => KeyCode::Char(b'H'),
        (0x24, _) => KeyCode::Char(b'J'),
        (0x25, _) => KeyCode::Char(b'K'),
        (0x26, _) => KeyCode::Char(b'L'),
        (0x27, _) => KeyCode::Raw(0xBA),
        (0x28, _) => KeyCode::Raw(0xDE),
        (0x29, _) => KeyCode::Raw(0xC0),
        (0x2A, _) => KeyCode::ShiftLeft,
        (0x2B, _) => KeyCode::Raw(0xDC),
        (0x2C, _) => KeyCode::Char(b'Z'),
        (0x2D, _) => KeyCode::Char(b'X'),
        (0x2E, _) => KeyCode::Char(b'C'),
        (0x2F, _) => KeyCode::Char(b'V'),
        (0x30, _) => KeyCode::Char(b'B'),
        (0x31, _) => KeyCode::Char(b'N'),
        (0x32, _) => KeyCode::Char(b'M'),
        (0x33, _) => KeyCode::Raw(0xBC),
        (0x34, _) => KeyCode::Raw(0xBE),
        (0x35, false) => KeyCode::Raw(0xBF),
        (0x35, true) => KeyCode::KeypadDivide,
        (0x36, _) => KeyCode::ShiftRight,
        (0x37, false) => KeyCode::KeypadMultiply,
        (0x38, false) => KeyCode::AltLeft,
        (0x38, true) => KeyCode::AltRight,
        (0x39, _) => KeyCode::Space,
        (0x3A, _) => KeyCode::CapsLock,
        (0x3B, _) => KeyCode::F1,
        (0x3C, _) => KeyCode::F2,
        (0x3D, _) => KeyCode::F3,
        (0x3E, _) => KeyCode::F4,
        (0x3F, _) => KeyCode::F5,
        (0x40, _) => KeyCode::F6,
        (0x41, _) => KeyCode::F7,
        (0x42, _) => KeyCode::F8,
        (0x43, _) => KeyCode::F9,
        (0x44, _) => KeyCode::F10,
        (0x45, _) => KeyCode::NumLock,
        (0x47, false) => KeyCode::Keypad7,
        (0x47, true) => KeyCode::Home,
        (0x48, false) => KeyCode::Keypad8,
        (0x48, true) => KeyCode::Up,
        (0x49, false) => KeyCode::Keypad9,
        (0x49, true) => KeyCode::PageUp,
        (0x4A, false) => KeyCode::KeypadSubtract,
        (0x4B, false) => KeyCode::Keypad4,
        (0x4B, true) => KeyCode::Left,
        (0x4C, false) => KeyCode::Keypad5,
        (0x4D, false) => KeyCode::Keypad6,
        (0x4D, true) => KeyCode::Right,
        (0x4E, false) => KeyCode::KeypadAdd,
        (0x4F, false) => KeyCode::Keypad1,
        (0x4F, true) => KeyCode::End,
        (0x50, false) => KeyCode::Keypad2,
        (0x50, true) => KeyCode::Down,
        (0x51, false) => KeyCode::Keypad3,
        (0x51, true) => KeyCode::PageDown,
        (0x52, false) => KeyCode::Keypad0,
        (0x52, true) => KeyCode::Insert,
        (0x53, false) => KeyCode::KeypadDecimal,
        (0x53, true) => KeyCode::Delete,
        (0x56, false) => KeyCode::Raw(RSHARE_ISO_102ND_RAW),
        (0x57, _) => KeyCode::F11,
        (0x58, _) => KeyCode::F12,
        (0x5B, true) => KeyCode::SuperLeft,
        (0x5C, true) => KeyCode::SuperRight,
        _ => return None,
    })
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
        assert_eq!(MouseButton::Back.to_code(), 4);
        assert_eq!(MouseButton::Forward.to_code(), 5);
        assert_eq!(MouseButton::from_code(4), MouseButton::Back);
        assert_eq!(MouseButton::from_code(5), MouseButton::Forward);
        assert_eq!(MouseButton::from_code(6), MouseButton::Other(6));
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
    fn text_commit_event_serializes_unicode_text() {
        let event = InputEvent::text_commit("你好🙂".to_string());
        let serialized = serde_json::to_string(&event).unwrap();
        let deserialized: InputEvent = serde_json::from_str(&serialized).unwrap();

        match deserialized {
            InputEvent::TextCommit { text } => assert_eq!(text, "你好🙂"),
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_gamepad_event_serialization() {
        let event = InputEvent::gamepad_state(GamepadState {
            gamepad_id: 0,
            sequence: 9,
            buttons: vec![GamepadButtonState {
                button: GamepadButton::South,
                pressed: true,
            }],
            left_stick_x: -100,
            left_stick_y: 200,
            right_stick_x: 0,
            right_stick_y: 0,
            left_trigger: 128,
            right_trigger: 1024,
            timestamp_ms: 555,
        });

        let serialized = serde_json::to_string(&event).unwrap();
        let deserialized: InputEvent = serde_json::from_str(&serialized).unwrap();

        assert!(matches!(
            deserialized,
            InputEvent::GamepadState {
                state: GamepadState {
                    gamepad_id: 0,
                    sequence: 9,
                    right_trigger: 1024,
                    ..
                }
            }
        ));
    }

    #[test]
    fn test_keycode_raw() {
        assert_eq!(KeyCode::Space.to_raw(), 0x20);
        assert_eq!(KeyCode::ShiftLeft.to_raw(), 0xA0);
        assert_eq!(KeyCode::ShiftRight.to_raw(), 0xA1);
        assert_eq!(KeyCode::ControlLeft.to_raw(), 0xA2);
        assert_eq!(KeyCode::AltRight.to_raw(), 0xA5);
        assert_eq!(KeyCode::SuperLeft.to_raw(), 0x5B);
        assert_eq!(KeyCode::Keypad5.to_raw(), 0x65);
        assert_eq!(KeyCode::KeypadDivide.to_raw(), 0x6F);
        assert_ne!(KeyCode::KeypadEnter.to_raw(), KeyCode::Enter.to_raw());
        assert_eq!(KeyCode::KeypadEnter.to_raw(), RSHARE_KEYPAD_ENTER_RAW);
        assert_eq!(KeyCode::Raw(123).to_raw(), 123);
    }

    #[test]
    fn canonical_wire_keys_restore_semantics_and_preserve_unknown_raw_values() {
        for keycode in [
            KeyCode::Char(b'A'),
            KeyCode::Char(b'W'),
            KeyCode::Char(b'7'),
            KeyCode::ShiftLeft,
            KeyCode::ControlRight,
            KeyCode::Left,
            KeyCode::F12,
            KeyCode::Keypad5,
            KeyCode::KeypadEnter,
        ] {
            assert_eq!(KeyCode::from_wire(keycode.to_raw()), keycode);
        }
        assert_eq!(KeyCode::from_wire(0xBA), KeyCode::Raw(0xBA));
        assert_eq!(KeyCode::from_wire(0xDE), KeyCode::Raw(0xDE));
        assert_eq!(KeyCode::from_wire(0xDEAD), KeyCode::Raw(0xDEAD));
    }

    #[test]
    fn linux_evdev_keycodes_are_normalized_without_wire_collisions() {
        assert_eq!(
            linux_evdev_keycode_to_keycode(32),
            Some(KeyCode::Char(b'D'))
        );
        assert_eq!(linux_evdev_keycode_to_keycode(57), Some(KeyCode::Space));
        assert_eq!(linux_evdev_keycode_to_keycode(99), Some(KeyCode::Raw(0x2C)));
        assert_eq!(linux_evdev_keycode_to_keycode(70), Some(KeyCode::Raw(0x91)));
        assert_eq!(
            linux_evdev_keycode_to_keycode(119),
            Some(KeyCode::Raw(0x13))
        );
        assert_eq!(linux_evdev_keycode_to_keycode(196), None);
    }

    #[test]
    fn linux_evdev_backslash_and_iso_102nd_have_distinct_round_trips() {
        let backslash = linux_evdev_keycode_to_keycode(43).expect("KEY_BACKSLASH is mapped");
        let iso_102nd = linux_evdev_keycode_to_keycode(86).expect("KEY_102ND is mapped");

        assert_eq!(backslash, KeyCode::Raw(0xDC));
        assert_eq!(iso_102nd, KeyCode::Raw(RSHARE_ISO_102ND_RAW));
        assert_ne!(backslash, iso_102nd);
        assert_eq!(keycode_to_linux_evdev_keycode(backslash), Some(43));
        assert_eq!(keycode_to_linux_evdev_keycode(iso_102nd), Some(86));
        assert_eq!(KeyCode::from_wire(RSHARE_ISO_102ND_RAW), iso_102nd);
    }

    #[test]
    fn linux_uinput_mapping_uses_evdev_codes_not_canonical_wire_values() {
        assert_eq!(
            keycode_to_linux_evdev_keycode(KeyCode::Char(b'D')),
            Some(32)
        );
        assert_eq!(keycode_to_linux_evdev_keycode(KeyCode::Space), Some(57));
        assert_ne!(
            keycode_to_linux_evdev_keycode(KeyCode::Char(b'D')),
            Some(KeyCode::Char(b'D').to_raw())
        );
        assert_ne!(
            keycode_to_linux_evdev_keycode(KeyCode::Space),
            Some(KeyCode::Space.to_raw())
        );
        assert_eq!(keycode_to_linux_evdev_keycode(KeyCode::Raw(32)), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_hardware_keycodes_are_normalized_before_semantic_ingress() {
        let semantic_key =
            |keycode| match InputEvent::from_macos_event(rshare_platform::MacosInputEvent::Key {
                keycode,
                down: true,
            }) {
                Some(InputEvent::Key { keycode, .. }) => keycode,
                _ => panic!("macOS key did not become a semantic key event"),
            };

        assert_eq!(semantic_key(0x00), KeyCode::Char(b'A'));
        assert_eq!(semantic_key(0x0A), KeyCode::Raw(RSHARE_ISO_102ND_RAW));
        assert_eq!(semantic_key(0x38), KeyCode::ShiftLeft);
        assert_eq!(semantic_key(0x36), KeyCode::SuperRight);
        assert_eq!(semantic_key(0x7B), KeyCode::Left);
        assert_eq!(semantic_key(0x7A), KeyCode::F1);
        assert_eq!(semantic_key(0x72), KeyCode::Insert);
        assert_eq!(semantic_key(0x52), KeyCode::Keypad0);
        assert_eq!(semantic_key(0x18), KeyCode::Raw(0xBB));
        assert_eq!(semantic_key(0x1B), KeyCode::Raw(0xBD));
        assert_eq!(semantic_key(0x1E), KeyCode::Raw(0xDD));
        assert_eq!(semantic_key(0x21), KeyCode::Raw(0xDB));
        assert_eq!(semantic_key(0x27), KeyCode::Raw(0xDE));
        assert_eq!(semantic_key(0x29), KeyCode::Raw(0xBA));
        assert_eq!(semantic_key(0x2A), KeyCode::Raw(0xDC));
        assert_eq!(semantic_key(0x2B), KeyCode::Raw(0xBC));
        assert_eq!(semantic_key(0x2C), KeyCode::Raw(0xBF));
        assert_eq!(semantic_key(0x2F), KeyCode::Raw(0xBE));
        assert_eq!(semantic_key(0x32), KeyCode::Raw(0xC0));

        let wire_round_trip =
            |hardware_keycode| KeyCode::from_wire(semantic_key(hardware_keycode).to_raw());
        assert_eq!(wire_round_trip(0x00), KeyCode::Char(b'A'));
        assert_eq!(wire_round_trip(0x0D), KeyCode::Char(b'W'));
        assert_eq!(wire_round_trip(0x38), KeyCode::ShiftLeft);
        assert_eq!(wire_round_trip(0x7B), KeyCode::Left);
        assert_eq!(wire_round_trip(0x72), KeyCode::Insert);
        assert_eq!(wire_round_trip(0x18), KeyCode::Raw(0xBB));
        assert_eq!(wire_round_trip(0x0A), KeyCode::Raw(RSHARE_ISO_102ND_RAW));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unsupported_macos_hardware_keys_are_dropped_before_wire_encoding() {
        for keycode in [
            0x3F, // Fn: tracked natively, but has no cross-platform key variant.
            0x42, // JIS/media-adjacent system key.
            0x48, // volume up.
            0x49, // volume down.
            0x4A, // mute.
            0x5D, // JIS yen.
            0x69, // F13, which collides with canonical Keypad9 if forwarded raw.
        ] {
            assert_eq!(key_code_from_macos_keycode(keycode), None);
            assert!(
                InputEvent::from_macos_event(rshare_platform::MacosInputEvent::Key {
                    keycode,
                    down: true,
                })
                .is_none()
            );
        }

        assert_eq!(KeyCode::from_wire(0x69), KeyCode::Keypad9);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_vk_codes_are_normalized_for_keyboard_feedback() {
        assert_eq!(key_code_from_windows_vk(0x41), Some(KeyCode::Char(b'A')));
        assert_eq!(key_code_from_windows_vk(0x5A), Some(KeyCode::Char(b'Z')));
        assert_eq!(key_code_from_windows_vk(0x31), Some(KeyCode::Char(b'1')));
        assert_eq!(key_code_from_windows_vk(0x30), Some(KeyCode::Char(b'0')));
        assert_eq!(key_code_from_windows_vk(0xA0), Some(KeyCode::ShiftLeft));
        assert_eq!(key_code_from_windows_vk(0x70), Some(KeyCode::F1));
        for raw in [
            0x13,
            0x2C,
            0x91,
            0xBA,
            0xBB,
            0xBC,
            0xBD,
            0xBE,
            0xBF,
            0xC0,
            0xDB,
            0xDC,
            0xDD,
            0xDE,
            RSHARE_ISO_102ND_RAW,
        ] {
            assert_eq!(key_code_from_windows_vk(raw), Some(KeyCode::Raw(raw)));
            assert_eq!(KeyCode::from_wire(raw).to_raw(), raw);
        }
        assert_eq!(key_code_from_windows_vk(0xFF), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_scan_codes_are_normalized_for_driver_capture() {
        assert_eq!(
            key_code_from_windows_scan_code(0x1E, 0),
            Some(KeyCode::Char(b'A'))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x30, 0),
            Some(KeyCode::Char(b'B'))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x02, 0),
            Some(KeyCode::Char(b'1'))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x1D, 0),
            Some(KeyCode::ControlLeft)
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x1D, 0x02),
            Some(KeyCode::ControlRight)
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x48, 0x02),
            Some(KeyCode::Up)
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x5B, 0x02),
            Some(KeyCode::SuperLeft)
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x56, 0),
            Some(KeyCode::Raw(RSHARE_ISO_102ND_RAW))
        );
        assert_eq!(key_code_from_windows_scan_code(0x7F, 0), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_punctuation_scan_codes_normalize_to_virtual_keys_for_driver_capture() {
        assert_eq!(
            key_code_from_windows_scan_code(0x0C, 0),
            Some(KeyCode::Raw(0xBD))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x0D, 0),
            Some(KeyCode::Raw(0xBB))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x1A, 0),
            Some(KeyCode::Raw(0xDB))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x1B, 0),
            Some(KeyCode::Raw(0xDD))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x27, 0),
            Some(KeyCode::Raw(0xBA))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x28, 0),
            Some(KeyCode::Raw(0xDE))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x29, 0),
            Some(KeyCode::Raw(0xC0))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x2B, 0),
            Some(KeyCode::Raw(0xDC))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x33, 0),
            Some(KeyCode::Raw(0xBC))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x34, 0),
            Some(KeyCode::Raw(0xBE))
        );
        assert_eq!(
            key_code_from_windows_scan_code(0x35, 0),
            Some(KeyCode::Raw(0xBF))
        );
    }

    #[test]
    fn test_should_forward() {
        assert!(InputEvent::mouse_move(0, 0).should_forward());
        assert!(InputEvent::key(KeyCode::Space, ButtonState::Pressed).should_forward());
        assert!(InputEvent::gamepad_state(GamepadState::neutral(0, 1, 123)).should_forward());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_event_conversion() {
        let event = InputEvent::from_windows_event(rshare_platform::WindowsInputEvent::Key {
            vk: 0x20,
            scan_code: 0x39,
            flags: 0,
            down: true,
        })
        .expect("space is a supported Windows virtual key");

        match event {
            InputEvent::Key { keycode, state } => {
                assert_eq!(keycode, KeyCode::Space);
                assert_eq!(state, ButtonState::Pressed);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_hook_preserves_keypad_enter_identity() {
        let semantic_key =
            |flags| match InputEvent::from_windows_event(rshare_platform::WindowsInputEvent::Key {
                vk: 0x0D,
                scan_code: 0x1C,
                flags,
                down: true,
            }) {
                Some(InputEvent::Key { keycode, .. }) => keycode,
                _ => panic!("Windows Enter hook event did not become a semantic key event"),
            };

        assert_eq!(semantic_key(0), KeyCode::Enter);
        assert_eq!(semantic_key(0x01), KeyCode::KeypadEnter);
        assert_eq!(key_code_from_windows_hook_key(0x0D, 0x1D, 0), None);
        assert_eq!(key_code_from_windows_hook_key(0x0D, 0x1D, 0x01), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_hook_preserves_left_and_right_modifier_identity() {
        const LLKHF_EXTENDED: u32 = 0x01;

        let cases = [
            (0x10, 0x2A, 0, KeyCode::ShiftLeft),
            (0x10, 0x36, 0, KeyCode::ShiftRight),
            (0x11, 0x1D, 0, KeyCode::ControlLeft),
            (0x11, 0x1D, LLKHF_EXTENDED, KeyCode::ControlRight),
            (0x12, 0x38, 0, KeyCode::AltLeft),
            (0x12, 0x38, LLKHF_EXTENDED, KeyCode::AltRight),
            (0xA0, 0, 0, KeyCode::ShiftLeft),
            (0xA1, 0, 0, KeyCode::ShiftRight),
            (0xA2, 0, 0, KeyCode::ControlLeft),
            (0xA3, 0, 0, KeyCode::ControlRight),
            (0xA4, 0, 0, KeyCode::AltLeft),
            (0xA5, 0, 0, KeyCode::AltRight),
        ];

        for (vk, scan_code, flags, expected) in cases {
            assert_eq!(
                key_code_from_windows_hook_key(vk, scan_code, flags),
                Some(expected),
                "vk={vk:#X}, scan={scan_code:#X}, flags={flags:#X}"
            );
        }

        assert_eq!(key_code_from_windows_hook_key(0x10, 0x1D, 0), None);
        assert_eq!(key_code_from_windows_hook_key(0x11, 0x36, 0), None);
        assert_eq!(key_code_from_windows_hook_key(0x12, 0x1D, 0), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_driver_key_event_conversion() {
        use rshare_platform::windows::{
            WindowsDriverDeviceKind, WindowsDriverEventKind, WindowsDriverEventSource,
        };

        let driver_event = rshare_platform::windows::WindowsDriverInputEvent {
            source: WindowsDriverEventSource::Hardware,
            device_kind: WindowsDriverDeviceKind::Keyboard,
            event_kind: WindowsDriverEventKind::Key,
            device_id: "test-keyboard".to_string(),
            device_instance_id: "test-instance".to_string(),
            value0: 0x1E, // A key set-1 scan code
            value1: 1,    // pressed
            value2: 0,
            flags: 0,
            timestamp_us: 0,
        };

        let event = InputEvent::from_windows_driver_event(driver_event);
        assert!(event.is_some());

        match event {
            Some(InputEvent::Key { keycode, state }) => {
                assert_eq!(keycode, KeyCode::Char(b'A'));
                assert_eq!(state, ButtonState::Pressed);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_driver_mouse_move_conversion() {
        use rshare_platform::windows::{
            WindowsDriverDeviceKind, WindowsDriverEventKind, WindowsDriverEventSource,
        };

        let driver_event = rshare_platform::windows::WindowsDriverInputEvent {
            source: WindowsDriverEventSource::Hardware,
            device_kind: WindowsDriverDeviceKind::Mouse,
            event_kind: WindowsDriverEventKind::MouseMove,
            device_id: "test-mouse".to_string(),
            device_instance_id: "test-instance".to_string(),
            value0: 100,
            value1: 200,
            value2: 0,
            flags: 0,
            timestamp_us: 0,
        };

        let event = InputEvent::from_windows_driver_event(driver_event);
        assert!(event.is_some());

        match event {
            Some(InputEvent::MouseMove { x, y }) => {
                assert_eq!(x, 100);
                assert_eq!(y, 200);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_driver_mouse_button_conversion() {
        use rshare_platform::windows::{
            WindowsDriverDeviceKind, WindowsDriverEventKind, WindowsDriverEventSource,
        };

        let driver_event = rshare_platform::windows::WindowsDriverInputEvent {
            source: WindowsDriverEventSource::Hardware,
            device_kind: WindowsDriverDeviceKind::Mouse,
            event_kind: WindowsDriverEventKind::MouseButton,
            device_id: "test-mouse".to_string(),
            device_instance_id: "test-instance".to_string(),
            value0: 1, // left button
            value1: 1, // pressed
            value2: 0,
            flags: 0,
            timestamp_us: 0,
        };

        let event = InputEvent::from_windows_driver_event(driver_event);
        assert!(event.is_some());

        match event {
            Some(InputEvent::MouseButton { button, state }) => {
                assert_eq!(button, MouseButton::Left);
                assert_eq!(state, ButtonState::Pressed);
            }
            _ => panic!("Wrong event type"),
        }
    }
}
