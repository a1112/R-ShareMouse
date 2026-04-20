//! Layout graph model for Alpha-2
//!
//! This module defines the topology model that drives input routing.
//! The layout graph represents how devices are arranged physically and
//! which edges lead to which target devices.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

use crate::Direction;

const DEFAULT_DISPLAY_WIDTH: u32 = 1920;
const DEFAULT_DISPLAY_HEIGHT: u32 = 1080;

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
    pub fn new(
        from_device: Uuid,
        from_edge: Direction,
        to_device: Uuid,
        to_edge: Direction,
    ) -> Self {
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

    /// Set the target for a source device edge.
    ///
    /// A source edge can only point to one target. Replacing conflicting edge
    /// mappings keeps routing deterministic because `resolve_target` returns
    /// the first matching source edge.
    pub fn upsert_link_for_edge(&mut self, link: LayoutLink) {
        self.links.retain(|existing| {
            !(existing.from_device == link.from_device && existing.from_edge == link.from_edge)
        });
        self.links.push(link);
    }

    /// Remove a node by device ID.
    pub fn remove_node(&mut self, device_id: Uuid) {
        self.nodes.retain(|n| n.device_id != device_id);
        // Also remove any links involving this device
        self.links
            .retain(|l| l.from_device != device_id && l.to_device != device_id);
    }

    /// Get a node by device ID.
    pub fn get_node(&self, device_id: Uuid) -> Option<&LayoutNode> {
        self.nodes.iter().find(|n| n.device_id == device_id)
    }

    /// Merge newly discovered peers into the remembered graph.
    ///
    /// Existing nodes are left untouched. New peers are appended to the right
    /// of the current right-most remembered node and linked bidirectionally to
    /// their immediate left neighbor.
    pub fn merge_discovered_peers_to_right<I>(&mut self, discovered_peers: I) -> bool
    where
        I: IntoIterator<Item = Uuid>,
    {
        let mut changed = false;
        if self.get_node(self.local_device).is_none() {
            self.add_node(LayoutNode::new(
                self.local_device,
                0,
                0,
                DEFAULT_DISPLAY_WIDTH,
                DEFAULT_DISPLAY_HEIGHT,
            ));
            changed = true;
        }

        let mut missing_peers: Vec<_> = discovered_peers
            .into_iter()
            .filter(|peer_id| *peer_id != self.local_device && self.get_node(*peer_id).is_none())
            .collect();
        missing_peers.sort();

        for peer_id in missing_peers {
            let (neighbor_id, x, y) = self
                .rightmost_node()
                .map(|node| {
                    let (_, _, right, _) = node_display_bounds(node);
                    let y = node.primary_display().map(|display| display.y).unwrap_or(0);
                    (node.device_id, right, y)
                })
                .unwrap_or((self.local_device, 0, 0));

            self.add_node(LayoutNode::new(
                peer_id,
                x,
                y,
                DEFAULT_DISPLAY_WIDTH,
                DEFAULT_DISPLAY_HEIGHT,
            ));
            self.add_bidirectional_neighbor_link(neighbor_id, peer_id);
            changed = true;
        }

        changed
    }

    /// Build an online-only compact projection for display rendering.
    ///
    /// The persisted graph is not mutated. Offline nodes are omitted from the
    /// returned graph, while visible nodes are packed horizontally in remembered
    /// order so hidden offline nodes do not leave gaps in the canvas. This
    /// return value is display-only and must not be saved as remembered layout.
    pub fn compact_online_display_projection<I>(&self, online_devices: I) -> LayoutGraph
    where
        I: IntoIterator<Item = Uuid>,
    {
        let mut visible_devices: HashSet<Uuid> = online_devices.into_iter().collect();
        visible_devices.insert(self.local_device);

        let mut visible_nodes: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| visible_devices.contains(&node.device_id))
            .cloned()
            .collect();
        visible_nodes.sort_by(|left, right| {
            let left_bounds = node_display_bounds(left);
            let right_bounds = node_display_bounds(right);
            left_bounds
                .0
                .cmp(&right_bounds.0)
                .then_with(|| left.device_id.cmp(&right.device_id))
        });

        let mut projection = LayoutGraph::new(self.local_device);
        let mut cursor_x = 0;
        for mut node in visible_nodes {
            let (left, _, right, _) = node_display_bounds(&node);
            let width = right.saturating_sub(left).max(1);
            for display in &mut node.displays {
                display.x = display.x - left + cursor_x;
            }
            cursor_x += width;
            projection.add_node(node);
        }

        projection.links = self
            .links
            .iter()
            .filter(|link| {
                visible_devices.contains(&link.from_device)
                    && visible_devices.contains(&link.to_device)
            })
            .cloned()
            .collect();
        projection
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
        let link = self
            .links
            .iter()
            .find(|l| l.from_device == from_device && l.from_edge == edge)?;

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

    /// Rewrite the graph so it is owned by the current local device.
    pub fn canonicalize_local_device(&mut self, current_local: Uuid) {
        let previous_local = self.local_device;
        if previous_local != current_local {
            for node in &mut self.nodes {
                if node.device_id == previous_local {
                    node.device_id = current_local;
                }
            }

            for link in &mut self.links {
                if link.from_device == previous_local {
                    link.from_device = current_local;
                }
                if link.to_device == previous_local {
                    link.to_device = current_local;
                }
            }

            self.local_device = current_local;
        }

        let mut seen = std::collections::HashSet::new();
        self.nodes.retain(|node| seen.insert(node.device_id));

        if self.get_node(current_local).is_none() {
            self.add_node(LayoutNode::new(current_local, 0, 0, 1920, 1080));
        }
    }

    fn rightmost_node(&self) -> Option<&LayoutNode> {
        self.nodes.iter().max_by(|left, right| {
            let left_bounds = node_display_bounds(left);
            let right_bounds = node_display_bounds(right);
            left_bounds
                .2
                .cmp(&right_bounds.2)
                .then_with(|| left.device_id.cmp(&right.device_id))
        })
    }

    fn add_bidirectional_neighbor_link(&mut self, left_device: Uuid, right_device: Uuid) {
        let forward = LayoutLink::new(left_device, Direction::Right, right_device, Direction::Left);
        let reverse = forward.reverse();
        self.upsert_link_for_edge(forward);
        self.upsert_link_for_edge(reverse);
    }
}

fn node_display_bounds(node: &LayoutNode) -> (i32, i32, i32, i32) {
    let mut displays = node.displays.iter();
    let Some(first) = displays.next() else {
        return (
            0,
            0,
            DEFAULT_DISPLAY_WIDTH as i32,
            DEFAULT_DISPLAY_HEIGHT as i32,
        );
    };

    let mut left = first.x;
    let mut top = first.y;
    let mut right = first.x + first.width as i32;
    let mut bottom = first.y + first.height as i32;

    for display in displays {
        left = left.min(display.x);
        top = top.min(display.y);
        right = right.max(display.x + display.width as i32);
        bottom = bottom.max(display.y + display.height as i32);
    }

    (left, top, right, bottom)
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

    #[test]
    fn canonicalize_local_device_rewrites_stale_owner_ids() {
        let stale_local = Uuid::new_v4();
        let current_local = Uuid::new_v4();
        let remote_id = Uuid::new_v4();
        let mut graph = LayoutGraph::new(stale_local);

        graph.add_node(LayoutNode::new(stale_local, 0, 0, 1920, 1080));
        graph.add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        graph.add_link(LayoutLink::new(
            stale_local,
            Direction::Right,
            remote_id,
            Direction::Left,
        ));

        graph.canonicalize_local_device(current_local);

        assert_eq!(graph.local_device, current_local);
        assert!(graph.get_node(current_local).is_some());
        assert!(graph
            .links
            .iter()
            .any(|link| link.from_device == current_local && link.to_device == remote_id));
    }
}
