//! Layout graph model for Alpha-2
//!
//! This module defines the topology model that drives input routing.
//! The layout graph represents how devices are arranged physically and
//! which edges lead to which target devices.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

use crate::Direction;

/// Display geometry within a layout node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayNode {
    /// Unique display identifier within the device.
    pub display_id: String,
    /// Display X offset in global layout coordinates.
    pub x: i32,
    /// Display Y offset in global layout coordinates.
    pub y: i32,
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// Whether this is the primary display.
    pub primary: bool,
}

impl DisplayNode {
    /// Create a new primary display with standard coordinates.
    pub fn primary(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            display_id: "primary".to_string(),
            x,
            y,
            width,
            height,
            primary: true,
        }
    }

    /// Create a secondary display.
    pub fn secondary(display_id: String, x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            display_id,
            x,
            y,
            width,
            height,
            primary: false,
        }
    }
}

/// Layout node representing a device in the topology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutNode {
    /// Device identifier.
    pub device_id: Uuid,
    /// Displays belonging to this device.
    pub displays: Vec<DisplayNode>,
}

impl LayoutNode {
    /// Create a new layout node with a single primary display.
    pub fn new(device_id: Uuid, x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            device_id,
            displays: vec![DisplayNode::primary(x, y, width, height)],
        }
    }

    /// Get the primary display for this node.
    pub fn primary_display(&self) -> Option<&DisplayNode> {
        self.displays.iter().find(|d| d.primary)
    }
}

/// Directional link between two devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutLink {
    /// Source device ID.
    pub from_device: Uuid,
    /// Edge on the source device.
    pub from_edge: Direction,
    /// Target device ID.
    pub to_device: Uuid,
    /// Edge on the target device.
    pub to_edge: Direction,
}

impl LayoutLink {
    /// Create a new directional link.
    pub fn new(from_device: Uuid, from_edge: Direction, to_device: Uuid, to_edge: Direction) -> Self {
        Self {
            from_device,
            from_edge,
            to_device,
            to_edge,
        }
    }

    /// Create the reverse link for this connection.
    pub fn reverse(&self) -> Self {
        Self {
            from_device: self.to_device,
            from_edge: self.to_edge,
            to_device: self.from_device,
            to_edge: self.from_edge,
        }
    }
}

/// Layout graph representing device topology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutGraph {
    /// Layout format version.
    pub version: u32,
    /// Local device ID (the device owning this graph).
    pub local_device: Uuid,
    /// All nodes in the layout.
    pub nodes: Vec<LayoutNode>,
    /// All links between devices.
    pub links: Vec<LayoutLink>,
}

impl LayoutGraph {
    /// Create a new empty layout graph.
    pub fn new(local_device: Uuid) -> Self {
        Self {
            version: 1,
            local_device,
            nodes: Vec::new(),
            links: Vec::new(),
        }
    }

    /// Add a node to the layout.
    pub fn add_node(&mut self, node: LayoutNode) {
        self.nodes.push(node);
    }

    /// Add a link to the layout.
    pub fn add_link(&mut self, link: LayoutLink) {
        self.links.push(link);
    }

    /// Remove a node by device ID.
    pub fn remove_node(&mut self, device_id: Uuid) {
        self.nodes.retain(|n| n.device_id != device_id);
        // Also remove any links involving this device
        self.links.retain(|l| l.from_device != device_id && l.to_device != device_id);
    }

    /// Get a node by device ID.
    pub fn get_node(&self, device_id: Uuid) -> Option<&LayoutNode> {
        self.nodes.iter().find(|n| n.device_id == device_id)
    }

    /// Resolve the target device for a given edge hit from a device.
    ///
    /// Returns `Some(target_id)` if:
    /// - The requesting device is the local device
    /// - A valid link exists for the given direction
    /// - The target device is in the connected_peers set
    ///
    /// Returns `None` otherwise.
    pub fn resolve_target(
        &self,
        from_device: Uuid,
        edge: Direction,
        connected_peers: &HashSet<Uuid>,
    ) -> Option<Uuid> {
        // Only allow resolution from the local device
        if from_device != self.local_device {
            return None;
        }

        // Find a matching link
        let link = self.links.iter().find(|l| {
            l.from_device == from_device && l.from_edge == edge
        })?;

        // Check if the target is connected
        if connected_peers.contains(&link.to_device) {
            Some(link.to_device)
        } else {
            None
        }
    }

    /// Get all links for a given device.
    pub fn links_for_device(&self, device_id: Uuid) -> Vec<&LayoutLink> {
        self.links
            .iter()
            .filter(|l| l.from_device == device_id || l.to_device == device_id)
            .collect()
    }

    /// Get all connected devices in the layout (excluding local).
    pub fn remote_devices(&self) -> Vec<Uuid> {
        self.nodes
            .iter()
            .map(|n| n.device_id)
            .filter(|id| id != &self.local_device)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_node_primary() {
        let display = DisplayNode::primary(0, 0, 1920, 1080);
        assert!(display.primary);
        assert_eq!(display.display_id, "primary");
    }

    #[test]
    fn test_layout_node_new() {
        let id = Uuid::new_v4();
        let node = LayoutNode::new(id, 0, 0, 1920, 1080);
        assert_eq!(node.device_id, id);
        assert_eq!(node.displays.len(), 1);
        assert!(node.displays[0].primary);
    }

    #[test]
    fn test_layout_link_reverse() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let link = LayoutLink::new(id1, Direction::Right, id2, Direction::Left);
        let reverse = link.reverse();

        assert_eq!(reverse.from_device, id2);
        assert_eq!(reverse.from_edge, Direction::Left);
        assert_eq!(reverse.to_device, id1);
        assert_eq!(reverse.to_edge, Direction::Right);
    }

    #[test]
    fn test_layout_graph_new() {
        let local_id = Uuid::new_v4();
        let graph = LayoutGraph::new(local_id);
        assert_eq!(graph.local_device, local_id);
        assert_eq!(graph.version, 1);
        assert!(graph.nodes.is_empty());
        assert!(graph.links.is_empty());
    }

    #[test]
    fn test_layout_graph_add_remove_node() {
        let local_id = Uuid::new_v4();
        let remote_id = Uuid::new_v4();
        let mut graph = LayoutGraph::new(local_id);

        graph.add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        assert_eq!(graph.nodes.len(), 1);

        graph.remove_node(remote_id);
        assert!(graph.nodes.is_empty());
    }
}
