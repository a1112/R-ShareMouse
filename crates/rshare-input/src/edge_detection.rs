//! Screen edge detection

use crate::events::InputEvent;
use rshare_common::{Direction, ScreenInfo};
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

/// Screen edge
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// Convert to Direction
    pub fn to_direction(self) -> Direction {
        match self {
            Edge::Left => Direction::Left,
            Edge::Right => Direction::Right,
            Edge::Top => Direction::Top,
            Edge::Bottom => Direction::Bottom,
        }
    }

    /// Get the opposite edge
    pub fn opposite(self) -> Edge {
        match self {
            Edge::Left => Edge::Right,
            Edge::Right => Edge::Left,
            Edge::Top => Edge::Bottom,
            Edge::Bottom => Edge::Top,
        }
    }
}

/// Edge detection configuration
#[derive(Debug, Clone)]
pub struct EdgeDetectionConfig {
    pub threshold: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    /// Cooldown after edge detection (prevents rapid switching)
    pub cooldown: Duration,
}

impl Default for EdgeDetectionConfig {
    fn default() -> Self {
        Self {
            threshold: 10,
            screen_width: 1920,
            screen_height: 1080,
            cooldown: Duration::from_millis(500),
        }
    }
}

impl From<&ScreenInfo> for EdgeDetectionConfig {
    fn from(screen: &ScreenInfo) -> Self {
        Self {
            threshold: 10,
            screen_width: screen.width,
            screen_height: screen.height,
            cooldown: Duration::from_millis(500),
        }
    }
}

/// Edge detection result
#[derive(Debug, Clone)]
pub struct EdgeDetectionResult {
    pub edge: Edge,
    pub x: i32,
    pub y: i32,
}

/// Edge detector
pub struct EdgeDetector {
    config: EdgeDetectionConfig,
    /// Map each edge to a target device ID
    edge_targets: HashMap<Edge, Uuid>,
    /// Cooldown state
    last_edge_time: Option<std::time::Instant>,
    last_edge: Option<Edge>,
}

impl EdgeDetector {
    pub fn new(config: EdgeDetectionConfig) -> Self {
        Self {
            config,
            edge_targets: HashMap::new(),
            last_edge_time: None,
            last_edge: None,
        }
    }

    /// Create from screen info
    pub fn from_screen_info(screen: &ScreenInfo) -> Self {
        Self::new(EdgeDetectionConfig::from(screen))
    }

    /// Set the target device for an edge
    pub fn set_target(&mut self, edge: Edge, device_id: Uuid) {
        self.edge_targets.insert(edge, device_id);
    }

    /// Remove target for an edge
    pub fn remove_target(&mut self, edge: &Edge) {
        self.edge_targets.remove(edge);
    }

    /// Get target device for an edge
    pub fn get_target(&self, edge: Edge) -> Option<&Uuid> {
        self.edge_targets.get(&edge)
    }

    /// Check if a mouse position is at a screen edge
    pub fn check_edge(&self, x: i32, y: i32) -> Option<Edge> {
        let th = self.config.threshold as i32;
        let w = self.config.screen_width as i32;
        let h = self.config.screen_height as i32;

        if x <= th {
            Some(Edge::Left)
        } else if x >= w - th {
            Some(Edge::Right)
        } else if y <= th {
            Some(Edge::Top)
        } else if y >= h - th {
            Some(Edge::Bottom)
        } else {
            None
        }
    }

    /// Check if we should trigger edge transition (with cooldown)
    pub fn should_transition(&mut self, x: i32, y: i32) -> Option<EdgeDetectionResult> {
        // Check cooldown
        if let Some(last_time) = self.last_edge_time {
            if last_time.elapsed() < self.config.cooldown {
                return None;
            }
        }

        if let Some(edge) = self.check_edge(x, y) {
            // Don't transition to the same edge we just came from
            if let Some(last_edge) = self.last_edge {
                if edge == last_edge.opposite() {
                    // Coming back from opposite edge - allow transition
                } else if edge == last_edge {
                    // Still at the same edge - don't trigger again
                    return None;
                }
            }

            self.last_edge_time = Some(std::time::Instant::now());
            self.last_edge = Some(edge);

            Some(EdgeDetectionResult { edge, x, y })
        } else {
            // Not at an edge - reset last edge
            self.last_edge = None;
            None
        }
    }

    /// Process an input event and detect edge transitions
    pub fn process_event(&mut self, event: &InputEvent) -> Option<EdgeDetectionResult> {
        match event {
            InputEvent::MouseMove { x, y } => self.should_transition(*x, *y),
            _ => None,
        }
    }

    /// Reset cooldown state
    pub fn reset_cooldown(&mut self) {
        self.last_edge_time = None;
        self.last_edge = None;
    }

    /// Update configuration
    pub fn update_config(&mut self, config: EdgeDetectionConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn config(&self) -> &EdgeDetectionConfig {
        &self.config
    }
}

/// Multi-screen edge detector for setups with multiple monitors
pub struct MultiScreenEdgeDetector {
    screens: Vec<(ScreenInfo, EdgeDetector)>,
}

impl MultiScreenEdgeDetector {
    pub fn new() -> Self {
        Self {
            screens: Vec::new(),
        }
    }

    /// Add a screen to the detector
    pub fn add_screen(&mut self, screen: ScreenInfo) {
        let detector = EdgeDetector::from_screen_info(&screen);
        self.screens.push((screen, detector));
    }

    /// Remove a screen
    pub fn remove_screen(&mut self, screen_index: usize) {
        if screen_index < self.screens.len() {
            self.screens.remove(screen_index);
        }
    }

    /// Check which screen a point is on
    pub fn find_screen(&self, x: i32, y: i32) -> Option<usize> {
        self.screens
            .iter()
            .position(|(screen, _)| screen.contains(x, y))
    }

    /// Process an event on all screens
    pub fn process_event(&mut self, event: &InputEvent) -> Option<(usize, EdgeDetectionResult)> {
        match event {
            InputEvent::MouseMove { x, y } => {
                // Find which screen we're on
                let screen_idx = self.find_screen(*x, *y)?;
                let (_, detector) = &mut self.screens[screen_idx];

                detector
                    .process_event(event)
                    .map(|result| (screen_idx, result))
            }
            _ => None,
        }
    }

    /// Get a specific screen's detector
    pub fn get_detector(&mut self, screen_index: usize) -> Option<&mut EdgeDetector> {
        self.screens.get_mut(screen_index).map(|(_, detector)| detector)
    }
}

impl Default for MultiScreenEdgeDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edge_conversion() {
        assert_eq!(Edge::Left.to_direction(), Direction::Left);
        assert_eq!(Edge::Right.to_direction(), Direction::Right);
        assert_eq!(Edge::Top.opposite(), Edge::Bottom);
    }

    #[test]
    fn test_edge_detection() {
        let config = EdgeDetectionConfig {
            threshold: 10,
            screen_width: 1920,
            screen_height: 1080,
            cooldown: Duration::from_millis(0),
        };
        let detector = EdgeDetector::new(config);

        // Test left edge
        assert_eq!(detector.check_edge(5, 500), Some(Edge::Left));
        // Test right edge
        assert_eq!(detector.check_edge(1915, 500), Some(Edge::Right));
        // Test top edge
        assert_eq!(detector.check_edge(500, 5), Some(Edge::Top));
        // Test bottom edge
        assert_eq!(detector.check_edge(500, 1075), Some(Edge::Bottom));
        // Test no edge
        assert_eq!(detector.check_edge(500, 500), None);
    }

    #[test]
    fn test_cooldown() {
        let config = EdgeDetectionConfig {
            threshold: 10,
            screen_width: 1920,
            screen_height: 1080,
            cooldown: Duration::from_millis(100),
        };
        let mut detector = EdgeDetector::new(config);

        // First detection should work
        let result1 = detector.should_transition(5, 500);
        assert!(result1.is_some());

        // Immediate second detection should be blocked by cooldown
        let result2 = detector.should_transition(5, 500);
        assert!(result2.is_none());
    }

    #[test]
    fn test_from_screen_info() {
        let screen = ScreenInfo::new(0, 0, 2560, 1440);
        let detector = EdgeDetector::from_screen_info(&screen);

        assert_eq!(detector.config.screen_width, 2560);
        assert_eq!(detector.config.screen_height, 1440);
    }

    #[test]
    fn test_multi_screen() {
        let mut multi = MultiScreenEdgeDetector::new();

        let screen1 = ScreenInfo::new(0, 0, 1920, 1080);
        let screen2 = ScreenInfo::new(1920, 0, 1920, 1080);

        multi.add_screen(screen1.clone());
        multi.add_screen(screen2);

        assert_eq!(multi.find_screen(100, 100), Some(0));
        assert_eq!(multi.find_screen(2000, 100), Some(1));
        assert_eq!(multi.find_screen(-10, 100), None);
    }
}
