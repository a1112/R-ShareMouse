use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rshare_core::{CaptureSessionStateMachine, Direction, LayoutGraph, LayoutLink, LayoutNode};
use std::collections::HashSet;
use uuid::Uuid;

fn control_benches(criterion: &mut Criterion) {
    let local = Uuid::from_u128(1);
    let remote = Uuid::from_u128(2);
    let mut graph = LayoutGraph::new(local);
    graph.add_node(LayoutNode::new(local, 0, 0, 1920, 1080));
    graph.add_node(LayoutNode::new(remote, 1920, 0, 1920, 1080));
    graph.add_link(LayoutLink::new(
        local,
        Direction::Right,
        remote,
        Direction::Left,
    ));
    let connected = HashSet::from([remote]);

    let mut group = criterion.benchmark_group("control");
    group.bench_function("layout_resolve_target", |bencher| {
        bencher.iter(|| {
            black_box(graph.resolve_target(
                black_box(local),
                black_box(Direction::Right),
                black_box(&connected),
            ))
        })
    });
    group.bench_function("session_transition", |bencher| {
        bencher.iter(|| {
            let mut session = CaptureSessionStateMachine::new();
            session
                .on_edge_hit(black_box(Direction::Right), black_box(Some(remote)))
                .unwrap();
            session
                .on_return_edge_hit(black_box(Direction::Left))
                .unwrap();
            black_box(session)
        })
    });

    group.finish();
}

criterion_group!(benches, control_benches);
criterion_main!(benches);
