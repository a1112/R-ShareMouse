//! R-ShareMouse common types
//!
//! This crate contains shared types used across multiple R-ShareMouse crates,
//! preventing circular dependencies.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Device identifier
pub type DeviceId = Uuid;

/// Direction for screen edges and movement
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Left,
    Right,
    Top,
    Bottom,
}

impl Direction {
    /// Get the opposite direction
    pub fn opposite(&self) -> Direction {
        match self {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
            Direction::Top => Direction::Bottom,
            Direction::Bottom => Direction::Top,
        }
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
    /// Create primary screen info
    pub fn primary() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    /// Create from dimensions
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Check if a point is within this screen
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < (self.x + self.width as i32)
            && y >= self.y
            && y < (self.y + self.height as i32)
    }

    /// Get the right edge x coordinate
    pub fn right_edge(&self) -> i32 {
        self.x + self.width as i32
    }

    /// Get the bottom edge y coordinate
    pub fn bottom_edge(&self) -> i32 {
        self.y + self.height as i32
    }
}

impl Default for ScreenInfo {
    fn default() -> Self {
        Self::primary()
    }
}

/// Button state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonState {
    Pressed,
    Released,
}

/// Key state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyState {
    Pressed,
    Released,
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
    fn test_screen_info_contains() {
        let screen = ScreenInfo::new(0, 0, 1920, 1080);
        assert!(screen.contains(100, 100));
        assert!(screen.contains(1919, 1079));
        assert!(!screen.contains(-1, 100));
        assert!(!screen.contains(100, -1));
        assert!(!screen.contains(1920, 100));
        assert!(!screen.contains(100, 1080));
    }

    #[test]
    fn test_screen_info_edges() {
        let screen = ScreenInfo::new(100, 100, 500, 400);
        assert_eq!(screen.right_edge(), 600);
        assert_eq!(screen.bottom_edge(), 500);
    }
}
