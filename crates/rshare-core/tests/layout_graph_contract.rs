//! Layout graph contract tests for Alpha-2
//!
//! This test module verifies that the layout graph correctly resolves
//! peer targets based on directional links.

use rshare_core::{Direction, DisplayNode, LayoutGraph, LayoutLink, LayoutNode};
use uuid::Uuid;
use std::collections::HashSet;

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
