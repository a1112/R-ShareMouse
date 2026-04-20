//! Layout graph contract tests for Alpha-2
//!
//! This test module verifies that the layout graph correctly resolves
//! peer targets based on directional links.

use rshare_core::{Direction, DisplayNode, LayoutGraph, LayoutLink, LayoutNode};
use std::collections::HashSet;
use uuid::Uuid;

fn primary_x(graph: &LayoutGraph, device_id: Uuid) -> i32 {
    graph
        .get_node(device_id)
        .and_then(LayoutNode::primary_display)
        .map(|display| display.x)
        .expect("node should have a primary display")
}

#[test]
fn layout_graph_resolves_right_link_target() {
    let local_id = Uuid::new_v4();
    let remote_id = Uuid::new_v4();

    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode {
        device_id: local_id,
        displays: vec![DisplayNode {
            display_id: "local-display".to_string(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        }],
    });
    graph.add_node(LayoutNode {
        device_id: remote_id,
        displays: vec![DisplayNode {
            display_id: "remote-display".to_string(),
            x: 1920,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        }],
    });

    // Link local right edge to remote left edge
    graph.add_link(LayoutLink {
        from_device: local_id,
        from_edge: Direction::Right,
        to_device: remote_id,
        to_edge: Direction::Left,
    });

    let mut connected_peers = HashSet::new();
    connected_peers.insert(remote_id);

    let target = graph.resolve_target(local_id, Direction::Right, &connected_peers);
    assert_eq!(target, Some(remote_id));
}

#[test]
fn layout_graph_returns_none_for_disconnected_peer() {
    let local_id = Uuid::new_v4();
    let remote_id = Uuid::new_v4();

    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode {
        device_id: local_id,
        displays: vec![DisplayNode {
            display_id: "local-display".to_string(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        }],
    });
    graph.add_node(LayoutNode {
        device_id: remote_id,
        displays: vec![DisplayNode {
            display_id: "remote-display".to_string(),
            x: 1920,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        }],
    });

    // Add link but don't add to connected peers
    graph.add_link(LayoutLink {
        from_device: local_id,
        from_edge: Direction::Right,
        to_device: remote_id,
        to_edge: Direction::Left,
    });

    let connected_peers = HashSet::new(); // Empty - no connected peers

    let target = graph.resolve_target(local_id, Direction::Right, &connected_peers);
    assert_eq!(target, None);
}

#[test]
fn layout_graph_returns_none_for_missing_link() {
    let local_id = Uuid::new_v4();
    let remote_id = Uuid::new_v4();

    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode {
        device_id: local_id,
        displays: vec![DisplayNode {
            display_id: "local-display".to_string(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        }],
    });
    graph.add_node(LayoutNode {
        device_id: remote_id,
        displays: vec![DisplayNode {
            display_id: "remote-display".to_string(),
            x: 1920,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        }],
    });

    // No link added

    let mut connected_peers = HashSet::new();
    connected_peers.insert(remote_id);

    let target = graph.resolve_target(local_id, Direction::Right, &connected_peers);
    assert_eq!(target, None);
}

#[test]
fn layout_graph_returns_none_for_non_local_device() {
    let local_id = Uuid::new_v4();
    let other_id = Uuid::new_v4();
    let remote_id = Uuid::new_v4();

    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode {
        device_id: local_id,
        displays: vec![DisplayNode {
            display_id: "local-display".to_string(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        }],
    });
    graph.add_node(LayoutNode {
        device_id: other_id,
        displays: vec![DisplayNode {
            display_id: "other-display".to_string(),
            x: 1920,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        }],
    });

    // Try to resolve from non-local device
    let mut connected_peers = HashSet::new();
    connected_peers.insert(remote_id);

    let target = graph.resolve_target(other_id, Direction::Right, &connected_peers);
    assert_eq!(target, None);
}

#[test]
fn layout_graph_appends_new_discovered_devices_to_the_right() {
    let local_id = Uuid::new_v4();
    let remote_a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let remote_b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));

    let changed = graph.merge_discovered_peers_to_right([remote_a, remote_b]);

    assert!(changed);
    assert_eq!(primary_x(&graph, remote_a), 1920);
    assert_eq!(primary_x(&graph, remote_b), 3840);
    assert!(graph.links.iter().any(|link| {
        link.from_device == local_id
            && link.from_edge == Direction::Right
            && link.to_device == remote_a
            && link.to_edge == Direction::Left
    }));
    assert!(graph.links.iter().any(|link| {
        link.from_device == remote_a
            && link.from_edge == Direction::Right
            && link.to_device == remote_b
            && link.to_edge == Direction::Left
    }));
}

#[test]
fn layout_graph_keeps_remembered_device_position_when_rediscovered() {
    let local_id = Uuid::new_v4();
    let remembered = Uuid::new_v4();
    let newly_discovered = Uuid::new_v4();
    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));
    graph.add_node(LayoutNode::new(remembered, 1920, 0, 1920, 1080));

    let changed = graph.merge_discovered_peers_to_right([newly_discovered, remembered]);

    assert!(changed);
    assert_eq!(primary_x(&graph, remembered), 1920);
    assert_eq!(primary_x(&graph, newly_discovered), 3840);
}

#[test]
fn layout_graph_merge_keeps_offline_remembered_nodes_in_persisted_graph() {
    let local_id = Uuid::new_v4();
    let offline = Uuid::new_v4();
    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));
    graph.add_node(LayoutNode::new(offline, 1920, 0, 1920, 1080));

    let changed = graph.merge_discovered_peers_to_right([]);

    assert!(!changed);
    assert!(graph.get_node(offline).is_some());
    assert_eq!(primary_x(&graph, offline), 1920);
}

#[test]
fn layout_graph_compact_online_projection_hides_offline_gaps_without_mutating_memory() {
    let local_id = Uuid::new_v4();
    let offline = Uuid::new_v4();
    let online = Uuid::new_v4();
    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));
    graph.add_node(LayoutNode::new(offline, 1920, 0, 1920, 1080));
    graph.add_node(LayoutNode::new(online, 3840, 0, 1920, 1080));
    graph.add_link(LayoutLink::new(
        local_id,
        Direction::Right,
        offline,
        Direction::Left,
    ));
    graph.add_link(LayoutLink::new(
        offline,
        Direction::Right,
        online,
        Direction::Left,
    ));

    let visible = graph.compact_online_display_projection(HashSet::from([local_id, online]));

    assert!(visible.get_node(local_id).is_some());
    assert!(visible.get_node(online).is_some());
    assert!(visible.get_node(offline).is_none());
    assert_eq!(primary_x(&visible, local_id), 0);
    assert_eq!(primary_x(&visible, online), 1920);
    assert!(visible.links.is_empty());
    assert_eq!(primary_x(&graph, online), 3840);
    assert!(graph.get_node(offline).is_some());
}

#[test]
fn layout_graph_compact_projection_uses_actual_visible_widths() {
    let local_id = Uuid::new_v4();
    let online = Uuid::new_v4();
    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode::new(local_id, 0, 0, 1280, 720));
    graph.add_node(LayoutNode::new(online, 3000, 0, 1024, 768));

    let visible = graph.compact_online_display_projection(HashSet::from([local_id, online]));

    assert_eq!(primary_x(&visible, local_id), 0);
    assert_eq!(primary_x(&visible, online), 1280);
}

#[test]
fn layout_graph_display_projection_preserves_only_remembered_visible_links() {
    let local_id = Uuid::new_v4();
    let online_a = Uuid::new_v4();
    let online_b = Uuid::new_v4();
    let offline = Uuid::new_v4();
    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));
    graph.add_node(LayoutNode::new(offline, 1920, 0, 1920, 1080));
    graph.add_node(LayoutNode::new(online_a, 3840, 0, 1920, 1080));
    graph.add_node(LayoutNode::new(online_b, 5760, 0, 1920, 1080));
    graph.add_link(LayoutLink::new(
        local_id,
        Direction::Right,
        offline,
        Direction::Left,
    ));
    graph.add_link(LayoutLink::new(
        online_a,
        Direction::Bottom,
        online_b,
        Direction::Top,
    ));

    let visible =
        graph.compact_online_display_projection(HashSet::from([local_id, online_a, online_b]));

    assert_eq!(visible.links.len(), 1);
    assert_eq!(
        visible.links[0],
        LayoutLink::new(online_a, Direction::Bottom, online_b, Direction::Top)
    );
}

#[test]
fn layout_graph_upsert_link_for_edge_replaces_conflicting_targets() {
    let local_id = Uuid::new_v4();
    let old_target = Uuid::new_v4();
    let new_target = Uuid::new_v4();
    let mut graph = LayoutGraph::new(local_id);
    graph.upsert_link_for_edge(LayoutLink::new(
        local_id,
        Direction::Right,
        old_target,
        Direction::Left,
    ));
    graph.upsert_link_for_edge(LayoutLink::new(
        local_id,
        Direction::Right,
        new_target,
        Direction::Left,
    ));
    let connected_peers = HashSet::from([old_target, new_target]);

    assert_eq!(graph.links.len(), 1);
    assert_eq!(
        graph.resolve_target(local_id, Direction::Right, &connected_peers),
        Some(new_target)
    );
}

#[test]
fn layout_graph_merge_reports_changed_when_repairing_missing_local_node() {
    let local_id = Uuid::new_v4();
    let mut graph = LayoutGraph::new(local_id);

    let changed = graph.merge_discovered_peers_to_right([]);

    assert!(changed);
    assert!(graph.get_node(local_id).is_some());
}

#[test]
fn layout_graph_merge_uses_stable_order_for_multiple_new_devices() {
    let local_id = Uuid::new_v4();
    let first = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let second = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));

    graph.merge_discovered_peers_to_right([second, first]);

    assert_eq!(primary_x(&graph, first), 1920);
    assert_eq!(primary_x(&graph, second), 3840);
}
