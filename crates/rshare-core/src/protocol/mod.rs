//! Protocol definitions for R-ShareMouse
//!
//! This module defines the message types and data structures used for
//! communication between devices.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Device identifier
pub type DeviceId = Uuid;

/// Protocol version
pub const PROTOCOL_VERSION: u32 = 1;

/// Message priority for transmission ordering
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Direction of screen transition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Left,
    Right,
    Top,
    Bottom,
}

impl Direction {
    /// Get the opposite direction
    pub fn opposite(&self) -> Self {
        match self {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
            Direction::Top => Direction::Bottom,
            Direction::Bottom => Direction::Top,
        }
    }
}

/// Button state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonState {
    Pressed,
    Released,
}

impl ButtonState {
    pub fn is_pressed(&self) -> bool {
        matches!(self, ButtonState::Pressed)
    }
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

/// Key state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyState {
    Pressed,
    Released,
}

impl KeyState {
    pub fn is_pressed(&self) -> bool {
        matches!(self, KeyState::Pressed)
    }
}

/// Screen information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenInfo {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl ScreenInfo {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Create a primary screen info (typical values)
    pub fn primary() -> Self {
        Self::new(0, 0, 1920, 1080)
    }

    /// Check if a point is within this screen
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < (self.x + self.width as i32)
            && y >= self.y
            && y < (self.y + self.height as i32)
    }

    /// Check if a point is at the edge of this screen
    pub fn is_at_edge(&self, x: i32, y: i32, threshold: u32) -> Option<Direction> {
        let th = threshold as i32;
        let right = self.x + self.width as i32;
        let bottom = self.y + self.height as i32;

        if x <= self.x + th {
            Some(Direction::Left)
        } else if x >= right - th {
            Some(Direction::Right)
        } else if y <= self.y + th {
            Some(Direction::Top)
        } else if y >= bottom - th {
            Some(Direction::Bottom)
        } else {
            None
        }
    }
}

/// Device capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    pub supports_clipboard: bool,
    pub supports_hotkeys: bool,
    pub max_devices: u32,
}

impl Default for DeviceCapabilities {
    fn default() -> Self {
        Self {
            supports_clipboard: true,
            supports_hotkeys: true,
            max_devices: 16,
        }
    }
}

/// Message type sent between devices
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    // === Discovery and Handshake ===
    /// Initial hello message for device discovery
    Hello {
        device_id: DeviceId,
        device_name: String,
        hostname: String,
        protocol_version: u32,
        capabilities: DeviceCapabilities,
    },
    /// Response to Hello message
    HelloBack {
        device_id: DeviceId,
        device_name: String,
        hostname: String,
        protocol_version: u32,
        capabilities: DeviceCapabilities,
        screen_info: ScreenInfo,
    },
    /// Device is leaving
    Goodbye { device_id: DeviceId, reason: String },

    // === Input Events ===
    /// Mouse movement (high frequency)
    MouseMove { x: i32, y: i32 },
    /// Mouse button state change
    MouseButton {
        button: MouseButton,
        state: ButtonState,
    },
    /// Mouse wheel scroll
    MouseWheel { delta_x: i32, delta_y: i32 },
    /// Key event
    Key { keycode: u32, state: KeyState },
    /// Key event with modifiers
    KeyExtended {
        keycode: u32,
        state: KeyState,
        shift: bool,
        ctrl: bool,
        alt: bool,
        meta: bool,
    },

    // === Clipboard ===
    /// Clipboard data (text only for now)
    ClipboardData { mime_type: String, data: Vec<u8> },
    /// Request clipboard data
    ClipboardRequest,
    /// Clipboard data response
    ClipboardResponse {
        success: bool,
        data: Option<Vec<u8>>,
    },

    // === Screen Control ===
    /// Cursor is entering a screen
    ScreenEnter { direction: Direction },
    /// Cursor is leaving a screen
    ScreenLeave {
        direction: Direction,
        target_device: DeviceId,
    },
    /// Screen configuration update
    ScreenUpdate { screen_info: ScreenInfo },

    // === Synchronization ===
    /// Heartbeat / keepalive
    Heartbeat { sequence: u64, timestamp: u64 },
    /// Acknowledgment for reliable delivery
    Ack { sequence: u64 },
    /// Error message
    Error { code: u32, message: String },
}

impl Message {
    /// Get the priority of this message type
    pub fn priority(&self) -> Priority {
        match self {
            // Critical: connection management
            Message::Hello { .. }
            | Message::HelloBack { .. }
            | Message::Goodbye { .. }
            | Message::ScreenEnter { .. }
            | Message::ScreenLeave { .. } => Priority::Critical,

            // High: immediate input feedback
            Message::MouseButton { .. } | Message::Key { .. } | Message::KeyExtended { .. } => {
                Priority::High
            }

            // Normal: continuous updates
            Message::MouseMove { .. }
            | Message::MouseWheel { .. }
            | Message::ScreenUpdate { .. } => Priority::Normal,

            // Low: background operations
            Message::ClipboardData { .. }
            | Message::ClipboardRequest
            | Message::ClipboardResponse { .. }
            | Message::Heartbeat { .. }
            | Message::Ack { .. }
            | Message::Error { .. } => Priority::Low,
        }
    }

    /// Check if this message requires reliable delivery
    pub fn requires_ack(&self) -> bool {
        matches!(
            self,
            Message::Hello { .. }
                | Message::HelloBack { .. }
                | Message::ScreenEnter { .. }
                | Message::ScreenLeave { .. }
                | Message::ScreenUpdate { .. }
                | Message::ClipboardRequest
                | Message::ClipboardResponse { .. }
        )
    }
}

/// Helper to create a Hello message
pub fn hello_message(device_id: DeviceId, device_name: String, hostname: String) -> Message {
    Message::Hello {
        device_id,
        device_name,
        hostname,
        protocol_version: PROTOCOL_VERSION,
        capabilities: DeviceCapabilities::default(),
    }
}

/// Helper to create a HelloBack message
pub fn hello_back_message(
    device_id: DeviceId,
    device_name: String,
    hostname: String,
    screen_info: ScreenInfo,
) -> Message {
    Message::HelloBack {
        device_id,
        device_name,
        hostname,
        protocol_version: PROTOCOL_VERSION,
        capabilities: DeviceCapabilities::default(),
        screen_info,
    }
}

/// Helper to create a heartbeat message
pub fn heartbeat_message(sequence: u64) -> Message {
    Message::Heartbeat {
        sequence,
        timestamp: timestamp_ms(),
    }
}

/// Get current timestamp in milliseconds
pub fn timestamp_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direction_opposite() {
        assert_eq!(Direction::Left.opposite(), Direction::Right);
        assert_eq!(Direction::Right.opposite(), Direction::Left);
        assert_eq!(Direction::Top.opposite(), Direction::Bottom);
        assert_eq!(Direction::Bottom.opposite(), Direction::Top);
    }

    #[test]
    fn test_screen_contains() {
        let screen = ScreenInfo::new(0, 0, 1920, 1080);
        assert!(screen.contains(100, 100));
        assert!(screen.contains(0, 0));
        assert!(screen.contains(1919, 1079));
        assert!(!screen.contains(-1, 100));
        assert!(!screen.contains(2000, 100));
    }

    #[test]
    fn test_screen_edge_detection() {
        let screen = ScreenInfo::new(0, 0, 1920, 1080);
        assert_eq!(screen.is_at_edge(5, 100, 10), Some(Direction::Left));
        assert_eq!(screen.is_at_edge(1915, 100, 10), Some(Direction::Right));
        assert_eq!(screen.is_at_edge(100, 5, 10), Some(Direction::Top));
        assert_eq!(screen.is_at_edge(100, 1075, 10), Some(Direction::Bottom));
        assert_eq!(screen.is_at_edge(500, 500, 10), None);
    }

    #[test]
    fn test_mouse_button_codes() {
        assert_eq!(MouseButton::Left.to_code(), 1);
        assert_eq!(MouseButton::Middle.to_code(), 2);
        assert_eq!(MouseButton::Right.to_code(), 3);
        assert_eq!(MouseButton::from_code(1), MouseButton::Left);
        assert_eq!(MouseButton::from_code(2), MouseButton::Middle);
    }

    #[test]
    fn test_message_serialization() {
        let msg = Message::MouseMove { x: 100, y: 200 };
        let serialized = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&serialized).unwrap();
        match deserialized {
            Message::MouseMove { x, y } => {
                assert_eq!(x, 100);
                assert_eq!(y, 200);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_message_priority() {
        let critical = Message::Hello {
            device_id: Uuid::new_v4(),
            device_name: "Test".to_string(),
            hostname: "test".to_string(),
            protocol_version: 1,
            capabilities: DeviceCapabilities::default(),
        };
        assert_eq!(critical.priority(), Priority::Critical);

        let high = Message::MouseButton {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
        };
        assert_eq!(high.priority(), Priority::High);

        let normal = Message::MouseMove { x: 100, y: 200 };
        assert_eq!(normal.priority(), Priority::Normal);
    }
}
