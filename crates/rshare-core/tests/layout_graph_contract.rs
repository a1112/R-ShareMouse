//! Layout graph contract tests for Alpha-2
//!
//! This test module verifies that the layout graph correctly resolves
//! peer targets based on directional links.

use rshare_core::{
    Direction, DisplayNode, LayoutGraph, LayoutLink, LayoutNode, LocalDisplayInfo,
    LocalDisplayState, PixelRect, RouteCache, VirtualDesktopGeometry,
};
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
            scale_percent: None,
            dpi_x: None,
            dpi_y: None,
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
            scale_percent: None,
            dpi_x: None,
            dpi_y: None,
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
            scale_percent: None,
            dpi_x: None,
            dpi_y: None,
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
            scale_percent: None,
            dpi_x: None,
            dpi_y: None,
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
            scale_percent: None,
            dpi_x: None,
            dpi_y: None,
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
            scale_percent: None,
            dpi_x: None,
            dpi_y: None,
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
            scale_percent: None,
            dpi_x: None,
            dpi_y: None,
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
            scale_percent: None,
            dpi_x: None,
            dpi_y: None,
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
fn layout_graph_online_projection_preserves_remembered_coordinates() {
    let local_id = Uuid::new_v4();
    let offline = Uuid::new_v4();
    let online = Uuid::new_v4();
    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));
    graph.add_node(LayoutNode::new(offline, 1920, 0, 1920, 1080));
    graph.add_node(LayoutNode::new(online, 3840, 720, 2560, 1440));

    let visible = graph.online_display_projection(HashSet::from([local_id, online]));

    assert!(visible.get_node(offline).is_none());
    let online_display = visible
        .get_node(online)
        .and_then(LayoutNode::primary_display)
        .expect("online device should remain visible");
    assert_eq!(online_display.x, 3840);
    assert_eq!(online_display.y, 720);
}

#[test]
fn layout_graph_online_projection_shifts_visible_nodes_into_view() {
    let local_id = Uuid::new_v4();
    let remote_id = Uuid::new_v4();
    let mut graph = LayoutGraph::new(local_id);
    graph.add_node(LayoutNode::new(local_id, 10064, 466, 2560, 1440));
    graph.add_node(LayoutNode::new(remote_id, 12624, 466, 1920, 1080));

    let visible = graph.online_display_projection(HashSet::from([local_id]));
    let local_display = visible
        .get_node(local_id)
        .and_then(LayoutNode::primary_display)
        .expect("local device should remain visible");

    assert!(visible.get_node(remote_id).is_none());
    assert_eq!(local_display.x, 0);
    assert_eq!(local_display.y, 0);
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

#[test]
fn virtual_desktop_geometry_uses_active_monitor_union_with_negative_coordinates() {
    let state = LocalDisplayState {
        virtual_x: -1920,
        virtual_y: -1080,
        layout_width: 4480,
        layout_height: 2520,
        displays: vec![
            LocalDisplayInfo {
                display_id: "left".to_string(),
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
                active: true,
                ..LocalDisplayInfo::default()
            },
            LocalDisplayInfo {
                display_id: "upper-right".to_string(),
                x: 0,
                y: -1080,
                width: 2560,
                height: 1440,
                active: true,
                ..LocalDisplayInfo::default()
            },
        ],
        ..LocalDisplayState::default()
    };

    let geometry = VirtualDesktopGeometry::from(&state);

    assert_eq!(geometry.bounds(), PixelRect::new(-1920, -1080, 4480, 2160));
}

#[test]
fn display_node_deserializes_old_layouts_without_scale_or_dpi() {
    let display: DisplayNode = serde_json::from_str(
        r#"{
            "display_id":"legacy",
            "x":0,
            "y":0,
            "width":1920,
            "height":1080,
            "primary":true
        }"#,
    )
    .unwrap();

    assert_eq!(display.scale_percent, None);
    assert_eq!(display.dpi_x, None);
    assert_eq!(display.dpi_y, None);
}

#[test]
fn route_cache_indexes_four_connected_directional_targets() {
    let local = Uuid::new_v4();
    let targets = [
        (Direction::Left, Direction::Right, Uuid::new_v4()),
        (Direction::Right, Direction::Left, Uuid::new_v4()),
        (Direction::Top, Direction::Bottom, Uuid::new_v4()),
        (Direction::Bottom, Direction::Top, Uuid::new_v4()),
    ];
    let mut graph = LayoutGraph::new(local);
    graph.add_node(LayoutNode::new(local, -1920, -1080, 3840, 2160));
    for (index, (from_edge, target_edge, target)) in targets.iter().copied().enumerate() {
        graph.add_node(LayoutNode {
            device_id: target,
            displays: vec![DisplayNode::primary((index as i32) * 100, 0, 2560, 1440)],
        });
        graph.add_link(LayoutLink::new(local, from_edge, target, target_edge));
    }
    let connected = targets
        .iter()
        .map(|(_, _, target)| *target)
        .collect::<HashSet<_>>();

    let cache = RouteCache::build(&graph, local, &connected, 7);

    assert_eq!(cache.generation(), 7);
    for (from_edge, target_edge, target) in targets {
        let route = cache
            .route(from_edge)
            .expect("connected route must be cached");
        assert_eq!(route.device_id, target);
        assert_eq!(route.entry_edge, target_edge);
        assert_eq!(
            route.display,
            PixelRect::new(route.display.x, 0, 2560, 1440)
        );
    }
}

#[test]
fn route_cache_preserves_target_monitor_offsets_relative_to_node_bounds() {
    let local = Uuid::new_v4();
    let target = Uuid::new_v4();
    let mut graph = LayoutGraph::new(local);
    graph.add_node(LayoutNode::new(local, 0, 0, 1920, 1080));
    graph.add_node(LayoutNode {
        device_id: target,
        displays: vec![
            DisplayNode::secondary("left".to_string(), 5000, 300, 1920, 1080),
            DisplayNode {
                display_id: "primary".to_string(),
                x: 6920,
                y: 300,
                width: 2560,
                height: 1440,
                primary: true,
                scale_percent: Some(125),
                dpi_x: Some(120),
                dpi_y: Some(120),
            },
        ],
    });
    graph.add_link(LayoutLink::new(
        local,
        Direction::Right,
        target,
        Direction::Left,
    ));

    let cache = RouteCache::build(&graph, local, &HashSet::from([target]), 1);
    let route = cache.route(Direction::Right).unwrap();

    assert_eq!(route.display, PixelRect::new(1920, 0, 2560, 1440));
}
