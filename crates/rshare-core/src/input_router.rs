use std::collections::HashSet;

use smallvec::{smallvec, SmallVec};

use crate::{
    ButtonState, CaptureSessionStateMachine, ControlSessionState, DeviceId, Direction, KeyState,
    LayoutGraph, MonotonicStamp, MouseButton, PendingReleaseBatch, PressedStateLedger,
    RealtimeInputFrame, RealtimeInputPayload, ReleaseAllReason, ReliableInputEvent,
    ReliableInputFrame, RouteCache, SessionEpoch, VirtualDesktopGeometry, INPUT_PROTOCOL_VERSION,
};

/// Semantic input accepted by the pure router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterInput {
    AbsoluteMove {
        x: i32,
        y: i32,
        captured_at: MonotonicStamp,
    },
    RelativeMove {
        dx: i32,
        dy: i32,
        captured_at: MonotonicStamp,
    },
    Key {
        keycode: u32,
        state: KeyState,
        captured_at: MonotonicStamp,
    },
    MouseButton {
        button: MouseButton,
        state: ButtonState,
        x: i32,
        y: i32,
        captured_at: MonotonicStamp,
    },
    Wheel {
        delta_x: i32,
        delta_y: i32,
        captured_at: MonotonicStamp,
    },
}

impl RouterInput {
    pub const fn absolute_move(x: i32, y: i32, captured_at: MonotonicStamp) -> Self {
        Self::AbsoluteMove { x, y, captured_at }
    }

    pub const fn relative_move(dx: i32, dy: i32, captured_at: MonotonicStamp) -> Self {
        Self::RelativeMove {
            dx,
            dy,
            captured_at,
        }
    }

    pub const fn key(keycode: u32, state: KeyState, captured_at: MonotonicStamp) -> Self {
        Self::Key {
            keycode,
            state,
            captured_at,
        }
    }

    pub const fn mouse_button(
        button: MouseButton,
        state: ButtonState,
        x: i32,
        y: i32,
        captured_at: MonotonicStamp,
    ) -> Self {
        Self::MouseButton {
            button,
            state,
            x,
            y,
            captured_at,
        }
    }

    pub const fn wheel(delta_x: i32, delta_y: i32, captured_at: MonotonicStamp) -> Self {
        Self::Wheel {
            delta_x,
            delta_y,
            captured_at,
        }
    }

    fn captured_at(&self) -> MonotonicStamp {
        match self {
            Self::AbsoluteMove { captured_at, .. }
            | Self::RelativeMove { captured_at, .. }
            | Self::Key { captured_at, .. }
            | Self::MouseButton { captured_at, .. }
            | Self::Wheel { captured_at, .. } => *captured_at,
        }
    }
}

/// Serialized commands consumed by the single-owner router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterCommand {
    Input(RouterInput),
    LayoutChanged(LayoutGraph),
    ConnectivityChanged { peer: DeviceId, connected: bool },
    QuickReturn,
    BackendDegraded,
    LeaseExpired,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterMetric {
    RouteCacheRebuilt { generation: u64 },
    CounterExhausted,
    PressedStateLedgerFault,
}

/// Side effects emitted by the pure router for outer layers to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterOutput {
    SendRealtime {
        target: DeviceId,
        frame: RealtimeInputFrame,
    },
    SendReliable {
        target: DeviceId,
        frame: ReliableInputFrame,
    },
    EmergencyReleaseAll {
        target: DeviceId,
        frame: ReliableInputFrame,
    },
    LocalSessionChanged(ControlSessionState),
    SuppressLocalShortcuts(bool),
    Metric(RouterMetric),
}

/// Pure, single-owner state machine for display-edge routing and input framing.
pub struct InputRouter {
    local_id: DeviceId,
    layout: LayoutGraph,
    connected_peers: HashSet<DeviceId>,
    epoch: SessionEpoch,
    realtime_sequence: u64,
    reliable_sequence: u64,
    emergency_max_sequence_consumed: bool,
    latest_anchor_sequence: u64,
    session: CaptureSessionStateMachine,
    routes: RouteCache,
    geometry: VirtualDesktopGeometry,
    pressed: PressedStateLedger,
    pending_release: Option<PendingReleaseBatch>,
    last_captured_at: MonotonicStamp,
    last_absolute: Option<(i32, i32)>,
}

impl InputRouter {
    pub fn new(
        local_id: DeviceId,
        layout: LayoutGraph,
        geometry: VirtualDesktopGeometry,
        connected_peers: impl IntoIterator<Item = DeviceId>,
    ) -> Self {
        let connected_peers = connected_peers.into_iter().collect::<HashSet<_>>();
        let routes = RouteCache::build(&layout, local_id, &connected_peers, 1);
        Self {
            local_id,
            layout,
            connected_peers,
            epoch: SessionEpoch(0),
            realtime_sequence: 0,
            reliable_sequence: 0,
            emergency_max_sequence_consumed: false,
            latest_anchor_sequence: 0,
            session: CaptureSessionStateMachine::new(),
            routes,
            geometry,
            pressed: PressedStateLedger::new(),
            pending_release: None,
            last_captured_at: MonotonicStamp::new(crate::ClockDomainId(0), 0),
            last_absolute: None,
        }
    }

    pub fn handle(&mut self, command: RouterCommand) -> SmallVec<[RouterOutput; 4]> {
        match command {
            RouterCommand::Input(input) => {
                self.last_captured_at = input.captured_at();
                self.handle_input(input)
            }
            RouterCommand::LayoutChanged(layout) => self.handle_layout_changed(layout),
            RouterCommand::ConnectivityChanged { peer, connected } => {
                self.handle_connectivity_changed(peer, connected)
            }
            RouterCommand::QuickReturn => self.return_to_local(ReleaseAllReason::OwnershipTransfer),
            RouterCommand::BackendDegraded => self.handle_backend_degraded(),
            RouterCommand::LeaseExpired => self.return_to_local(ReleaseAllReason::Timeout),
            RouterCommand::Shutdown => self.return_to_local(ReleaseAllReason::SessionEnded),
        }
    }

    pub fn route_cache_generation(&self) -> u64 {
        self.routes.generation()
    }

    pub fn held_key_count(&self) -> usize {
        self.pressed.held_key_count()
    }

    pub fn held_mouse_button_count(&self) -> usize {
        self.pressed.held_mouse_button_count()
    }

    pub fn pending_release_token(&self) -> Option<u64> {
        self.pending_release
            .as_ref()
            .map(PendingReleaseBatch::token)
    }

    fn handle_input(&mut self, input: RouterInput) -> SmallVec<[RouterOutput; 4]> {
        match input {
            RouterInput::AbsoluteMove { x, y, captured_at } => {
                self.handle_absolute_move(x, y, captured_at)
            }
            RouterInput::RelativeMove {
                dx,
                dy,
                captured_at,
            } => self.send_relative(dx, dy, captured_at),
            RouterInput::Key {
                keycode,
                state,
                captured_at,
            } => {
                let Some(target) = self.session.active_target() else {
                    return SmallVec::new();
                };
                if self.pressed.record_key(keycode, state).is_err() {
                    return self.handle_pressed_ledger_fault(target);
                }
                self.send_reliable(
                    target,
                    captured_at,
                    ReliableInputEvent::Key { keycode, state },
                )
            }
            RouterInput::MouseButton {
                button,
                state,
                x,
                y,
                captured_at,
            } => {
                let Some(target) = self.session.active_target() else {
                    return SmallVec::new();
                };
                if self
                    .pressed
                    .record_mouse_button(button, state, x, y, self.latest_anchor_sequence)
                    .is_err()
                {
                    return self.handle_pressed_ledger_fault(target);
                }
                self.send_reliable(
                    target,
                    captured_at,
                    ReliableInputEvent::MouseButton {
                        button,
                        state,
                        x,
                        y,
                        realtime_anchor_sequence: self.latest_anchor_sequence,
                    },
                )
            }
            RouterInput::Wheel {
                delta_x,
                delta_y,
                captured_at,
            } => {
                let Some(target) = self.session.active_target() else {
                    return SmallVec::new();
                };
                self.send_reliable(
                    target,
                    captured_at,
                    ReliableInputEvent::Wheel { delta_x, delta_y },
                )
            }
        }
    }

    fn handle_absolute_move(
        &mut self,
        x: i32,
        y: i32,
        captured_at: MonotonicStamp,
    ) -> SmallVec<[RouterOutput; 4]> {
        if self.session.is_remote_active() {
            let previous = self.last_absolute.replace((x, y));
            return previous.map_or_else(SmallVec::new, |(previous_x, previous_y)| {
                self.send_relative(
                    x.saturating_sub(previous_x),
                    y.saturating_sub(previous_y),
                    captured_at,
                )
            });
        }
        self.last_absolute = Some((x, y));
        if !self.session.is_local_ready() {
            return SmallVec::new();
        }
        let source = self.geometry.bounds();
        let Some(edge) = source.edge_at(x, y) else {
            return SmallVec::new();
        };
        let Some(route) = self.routes.route(edge) else {
            return SmallVec::new();
        };
        let target = route.device_id;
        let entry_edge = route.entry_edge;
        let display = route.display;
        let display_id = route.display_id.clone();
        let (mut target_x, mut target_y) = source.project_to(display, x, y);
        match entry_edge {
            Direction::Left => target_x = display.x,
            Direction::Right => target_x = display.last_x(),
            Direction::Top => target_y = display.y,
            Direction::Bottom => target_y = display.last_y(),
        }

        if self.epoch.advance().is_err() {
            self.session.on_backend_degraded();
            return smallvec![
                RouterOutput::LocalSessionChanged(self.session.state()),
                RouterOutput::SuppressLocalShortcuts(false),
                RouterOutput::Metric(RouterMetric::CounterExhausted),
            ];
        }
        self.realtime_sequence = 0;
        self.reliable_sequence = 0;
        self.emergency_max_sequence_consumed = false;
        self.latest_anchor_sequence = 0;
        if self.session.on_edge_hit(edge, Some(target)).is_err() {
            return SmallVec::new();
        }

        let Some(enter_sequence) = self.take_reliable_sequence() else {
            return self.handle_counter_exhausted(target);
        };
        let enter = ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: self.epoch,
            sequence: enter_sequence,
            captured_at,
            event: ReliableInputEvent::Enter {
                target_display_id: display_id,
                x: target_x,
                y: target_y,
            },
        };
        let Some(anchor_sequence) = self.take_realtime_sequence() else {
            return self.handle_counter_exhausted(target);
        };
        self.latest_anchor_sequence = anchor_sequence;
        let anchor = RealtimeInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: self.epoch,
            sequence: anchor_sequence,
            captured_at,
            payload: RealtimeInputPayload::AbsoluteAnchor {
                x: target_x,
                y: target_y,
            },
        };
        smallvec![
            RouterOutput::SendReliable {
                target,
                frame: enter,
            },
            RouterOutput::SendRealtime {
                target,
                frame: anchor,
            },
            RouterOutput::LocalSessionChanged(self.session.state()),
            RouterOutput::SuppressLocalShortcuts(true),
        ]
    }

    fn send_relative(
        &mut self,
        dx: i32,
        dy: i32,
        captured_at: MonotonicStamp,
    ) -> SmallVec<[RouterOutput; 4]> {
        let Some(target) = self.session.active_target() else {
            return SmallVec::new();
        };
        let Some(sequence) = self.take_realtime_sequence() else {
            return self.handle_counter_exhausted(target);
        };
        smallvec![RouterOutput::SendRealtime {
            target,
            frame: RealtimeInputFrame {
                protocol_version: INPUT_PROTOCOL_VERSION,
                session_epoch: self.epoch,
                sequence,
                captured_at,
                payload: RealtimeInputPayload::RelativeMouse { dx, dy },
            },
        }]
    }

    fn send_reliable(
        &mut self,
        target: DeviceId,
        captured_at: MonotonicStamp,
        event: ReliableInputEvent,
    ) -> SmallVec<[RouterOutput; 4]> {
        let Some(sequence) = self.take_reliable_sequence() else {
            return self.handle_counter_exhausted(target);
        };
        smallvec![RouterOutput::SendReliable {
            target,
            frame: ReliableInputFrame {
                protocol_version: INPUT_PROTOCOL_VERSION,
                session_epoch: self.epoch,
                sequence,
                captured_at,
                event,
            },
        }]
    }

    fn handle_layout_changed(&mut self, layout: LayoutGraph) -> SmallVec<[RouterOutput; 4]> {
        if self.layout == layout {
            return SmallVec::new();
        }
        if let Some(geometry) = layout
            .get_node(self.local_id)
            .and_then(VirtualDesktopGeometry::from_layout_node)
        {
            self.geometry = geometry;
        }
        self.layout = layout;
        self.rebuild_routes()
    }

    fn handle_connectivity_changed(
        &mut self,
        peer: DeviceId,
        connected: bool,
    ) -> SmallVec<[RouterOutput; 4]> {
        let changed = if connected {
            self.connected_peers.insert(peer)
        } else {
            self.connected_peers.remove(&peer)
        };
        if !changed {
            return SmallVec::new();
        }

        let active_target = self.session.active_target();
        let mut outputs = SmallVec::new();
        let mut ledger_fault = false;
        if !connected && active_target == Some(peer) {
            ledger_fault =
                self.push_emergency_release(&mut outputs, peer, ReleaseAllReason::Suspended);
            self.session.on_target_disconnect(peer);
            outputs.push(RouterOutput::LocalSessionChanged(self.session.state()));
            outputs.push(RouterOutput::SuppressLocalShortcuts(false));
        }
        let rebuilt = self.rebuild_routes();
        if ledger_fault {
            outputs.push(RouterOutput::Metric(RouterMetric::PressedStateLedgerFault));
        } else {
            outputs.extend(rebuilt);
        }
        outputs
    }

    fn handle_backend_degraded(&mut self) -> SmallVec<[RouterOutput; 4]> {
        let active_target = self.session.active_target();
        let mut outputs = SmallVec::new();
        let mut ledger_fault = false;
        if let Some(target) = active_target {
            ledger_fault =
                self.push_emergency_release(&mut outputs, target, ReleaseAllReason::BackendFailure);
        }
        self.session.on_backend_degraded();
        outputs.push(RouterOutput::LocalSessionChanged(self.session.state()));
        outputs.push(RouterOutput::SuppressLocalShortcuts(false));
        if ledger_fault {
            outputs.push(RouterOutput::Metric(RouterMetric::PressedStateLedgerFault));
        }
        outputs
    }

    fn return_to_local(&mut self, reason: ReleaseAllReason) -> SmallVec<[RouterOutput; 4]> {
        let Some(target) = self.session.active_target() else {
            return SmallVec::new();
        };
        let return_edge = match self.session.state() {
            ControlSessionState::RemoteActive { entered_via, .. } => entered_via.opposite(),
            _ => return SmallVec::new(),
        };
        let mut outputs = SmallVec::new();
        let ledger_fault = self.push_emergency_release(&mut outputs, target, reason);
        if self.session.on_return_edge_hit(return_edge).is_ok() {
            outputs.push(RouterOutput::LocalSessionChanged(self.session.state()));
            outputs.push(RouterOutput::SuppressLocalShortcuts(false));
        }
        if ledger_fault {
            outputs.push(RouterOutput::Metric(RouterMetric::PressedStateLedgerFault));
        }
        outputs
    }

    fn push_emergency_release(
        &mut self,
        outputs: &mut SmallVec<[RouterOutput; 4]>,
        target: DeviceId,
        reason: ReleaseAllReason,
    ) -> bool {
        let Some(sequence) = self.take_emergency_reliable_sequence() else {
            outputs.push(RouterOutput::Metric(RouterMetric::CounterExhausted));
            return false;
        };
        let ledger_fault = match self.pressed.release_all_events(reason) {
            Ok(batch) => {
                self.pending_release = Some(batch);
                false
            }
            Err(_) => true,
        };
        outputs.push(RouterOutput::EmergencyReleaseAll {
            target,
            frame: ReliableInputFrame {
                protocol_version: INPUT_PROTOCOL_VERSION,
                session_epoch: self.epoch,
                sequence,
                captured_at: self.last_captured_at,
                event: ReliableInputEvent::ReleaseAll { reason },
            },
        });
        ledger_fault
    }

    fn handle_pressed_ledger_fault(&mut self, target: DeviceId) -> SmallVec<[RouterOutput; 4]> {
        let mut outputs = SmallVec::new();
        self.push_emergency_release(&mut outputs, target, ReleaseAllReason::Suspended);
        self.session.on_backend_degraded();
        outputs.push(RouterOutput::LocalSessionChanged(self.session.state()));
        outputs.push(RouterOutput::SuppressLocalShortcuts(false));
        outputs.push(RouterOutput::Metric(RouterMetric::PressedStateLedgerFault));
        outputs
    }

    fn handle_counter_exhausted(&mut self, target: DeviceId) -> SmallVec<[RouterOutput; 4]> {
        let mut outputs = SmallVec::new();
        self.push_emergency_release(&mut outputs, target, ReleaseAllReason::Suspended);
        self.session.on_backend_degraded();
        outputs.push(RouterOutput::LocalSessionChanged(self.session.state()));
        outputs.push(RouterOutput::SuppressLocalShortcuts(false));
        outputs.push(RouterOutput::Metric(RouterMetric::CounterExhausted));
        outputs
    }

    fn rebuild_routes(&mut self) -> SmallVec<[RouterOutput; 4]> {
        let Some(generation) = self.routes.generation().checked_add(1) else {
            return smallvec![RouterOutput::Metric(RouterMetric::CounterExhausted)];
        };
        self.routes = RouteCache::build(
            &self.layout,
            self.local_id,
            &self.connected_peers,
            generation,
        );
        smallvec![RouterOutput::Metric(RouterMetric::RouteCacheRebuilt {
            generation,
        })]
    }

    fn take_realtime_sequence(&mut self) -> Option<u64> {
        let sequence = self.realtime_sequence;
        self.realtime_sequence = self.realtime_sequence.checked_add(1)?;
        Some(sequence)
    }

    fn take_reliable_sequence(&mut self) -> Option<u64> {
        let sequence = self.reliable_sequence;
        self.reliable_sequence = self.reliable_sequence.checked_add(1)?;
        Some(sequence)
    }

    fn take_emergency_reliable_sequence(&mut self) -> Option<u64> {
        let sequence = self.reliable_sequence;
        if sequence == u64::MAX {
            if self.emergency_max_sequence_consumed {
                return None;
            }
            self.emergency_max_sequence_consumed = true;
            return Some(sequence);
        }
        self.reliable_sequence = sequence.checked_add(1)?;
        Some(sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ButtonState, ClockDomainId, Direction, DisplayNode, KeyState, LayoutGraph, LayoutLink,
        LayoutNode, MonotonicStamp, MouseButton, PixelRect, RealtimeInputFrame,
        RealtimeInputPayload, ReliableInputEvent, ReliableInputFrame, SessionEpoch,
    };

    fn stamp(value_us: u64) -> MonotonicStamp {
        MonotonicStamp::new(ClockDomainId(9), value_us)
    }

    fn linked_router(
        local_bounds: PixelRect,
        from_edge: Direction,
        target_edge: Direction,
        target_bounds: PixelRect,
    ) -> (InputRouter, crate::DeviceId, crate::DeviceId) {
        let local = crate::DeviceId::new_v4();
        let target = crate::DeviceId::new_v4();
        let mut graph = LayoutGraph::new(local);
        graph.add_node(LayoutNode::new(
            local,
            local_bounds.x,
            local_bounds.y,
            local_bounds.width,
            local_bounds.height,
        ));
        graph.add_node(LayoutNode {
            device_id: target,
            displays: vec![DisplayNode::primary(
                target_bounds.x,
                target_bounds.y,
                target_bounds.width,
                target_bounds.height,
            )],
        });
        graph.add_link(LayoutLink::new(local, from_edge, target, target_edge));
        (
            InputRouter::new(
                local,
                graph,
                VirtualDesktopGeometry::new(local_bounds),
                [target],
            ),
            local,
            target,
        )
    }

    #[test]
    fn all_four_entries_use_real_negative_virtual_desktop_bounds() {
        let local = PixelRect::new(-1920, -1080, 3840, 2160);
        let cases = [
            (Direction::Left, Direction::Right, (-1920, 0), (2559, 720)),
            (Direction::Right, Direction::Left, (1919, 0), (0, 720)),
            (Direction::Top, Direction::Bottom, (0, -1080), (1280, 1439)),
            (Direction::Bottom, Direction::Top, (0, 1079), (1280, 0)),
        ];

        for (from_edge, target_edge, point, projected) in cases {
            let (mut router, _, target) = linked_router(
                local,
                from_edge,
                target_edge,
                PixelRect::new(0, 0, 2560, 1440),
            );
            let outputs = router.handle(RouterCommand::Input(RouterInput::absolute_move(
                point.0,
                point.1,
                stamp(10),
            )));
            assert!(matches!(
                outputs.first(),
                Some(RouterOutput::SendReliable {
                    target: actual_target,
                    frame: ReliableInputFrame {
                        event: ReliableInputEvent::Enter { x, y, .. },
                        ..
                    },
                }) if *actual_target == target && (*x, *y) == projected
            ));
        }
    }

    #[test]
    fn entry_projects_1080p_to_1440p_and_4k_to_1080p() {
        let (mut vertical, _, _) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 2560, 1440),
        );
        let outputs = vertical.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            540,
            stamp(1),
        )));
        assert!(matches!(
            outputs.first(),
            Some(RouterOutput::SendReliable {
                frame: ReliableInputFrame {
                    event: ReliableInputEvent::Enter { x: 0, y: 720, .. },
                    ..
                },
                ..
            })
        ));

        let (mut horizontal, _, _) = linked_router(
            PixelRect::new(0, 0, 3840, 2160),
            Direction::Bottom,
            Direction::Top,
            PixelRect::new(0, 0, 1920, 1080),
        );
        let outputs = horizontal.handle(RouterCommand::Input(RouterInput::absolute_move(
            1920,
            2159,
            stamp(2),
        )));
        assert!(matches!(
            outputs.first(),
            Some(RouterOutput::SendReliable {
                frame: ReliableInputFrame {
                    event: ReliableInputEvent::Enter { x: 960, y: 0, .. },
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn entry_normalizes_global_target_layout_coordinates_to_target_local_pixels() {
        let (mut router, _, _) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(1920, 240, 2560, 1440),
        );

        let outputs = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            540,
            stamp(1),
        )));

        assert!(matches!(
            outputs.first(),
            Some(RouterOutput::SendReliable {
                frame: ReliableInputFrame {
                    event: ReliableInputEvent::Enter { x: 0, y: 720, .. },
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            outputs.get(1),
            Some(RouterOutput::SendRealtime {
                frame: RealtimeInputFrame {
                    payload: RealtimeInputPayload::AbsoluteAnchor { x: 0, y: 720 },
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn enter_barrier_precedes_first_new_epoch_realtime_frame() {
        let (mut router, _, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 2560, 1440),
        );

        let outputs = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            700,
            stamp(10),
        )));

        assert!(matches!(
            outputs.as_slice(),
            [
                RouterOutput::SendReliable {
                    target: reliable_target,
                    frame: ReliableInputFrame {
                        session_epoch: SessionEpoch(1),
                        event: ReliableInputEvent::Enter { .. },
                        ..
                    },
                },
                RouterOutput::SendRealtime {
                    target: realtime_target,
                    frame: RealtimeInputFrame {
                        session_epoch: SessionEpoch(1),
                        payload: RealtimeInputPayload::AbsoluteAnchor { .. },
                        ..
                    },
                },
                RouterOutput::LocalSessionChanged(_),
                RouterOutput::SuppressLocalShortcuts(true),
            ] if *reliable_target == target && *realtime_target == target
        ));
    }

    #[test]
    fn relative_motion_after_entry_uses_the_captured_target() {
        let (mut router, _, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 2560, 1440),
        );
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            700,
            stamp(10),
        )));

        let moved = router.handle(RouterCommand::Input(RouterInput::relative_move(
            7,
            -3,
            stamp(11),
        )));
        assert!(matches!(
            moved.as_slice(),
            [RouterOutput::SendRealtime {
                target: actual_target,
                frame: RealtimeInputFrame {
                    payload: RealtimeInputPayload::RelativeMouse { dx: 7, dy: -3 },
                    ..
                },
            }] if *actual_target == target
        ));
    }

    #[test]
    fn quick_return_emits_release_all_before_local_ownership_and_keeps_ledger_pending() {
        let (mut router, _, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));
        let _ = router.handle(RouterCommand::Input(RouterInput::key(
            0x41,
            KeyState::Pressed,
            stamp(2),
        )));

        let outputs = router.handle(RouterCommand::QuickReturn);

        assert!(matches!(
            outputs.as_slice(),
            [
                RouterOutput::EmergencyReleaseAll {
                    target: actual_target,
                    frame: ReliableInputFrame {
                        event: ReliableInputEvent::ReleaseAll { .. },
                        ..
                    },
                },
                RouterOutput::LocalSessionChanged(crate::ControlSessionState::LocalReady),
                RouterOutput::SuppressLocalShortcuts(false),
            ] if *actual_target == target
        ));
        assert_eq!(router.held_key_count(), 1);
        assert!(router.pending_release_token().is_some());
    }

    #[test]
    fn disconnect_captures_target_and_releases_before_session_transition() {
        let (mut router, _, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));
        let before = router.route_cache_generation();

        let outputs = router.handle(RouterCommand::ConnectivityChanged {
            peer: target,
            connected: false,
        });

        assert!(matches!(
            outputs.first(),
            Some(RouterOutput::EmergencyReleaseAll {
                target: actual_target,
                frame: ReliableInputFrame {
                    event: ReliableInputEvent::ReleaseAll { .. },
                    ..
                },
            }) if *actual_target == target
        ));
        assert!(matches!(
            outputs.get(1),
            Some(RouterOutput::LocalSessionChanged(
                crate::ControlSessionState::Suspended { .. }
            ))
        ));
        assert_eq!(router.route_cache_generation(), before + 1);
    }

    #[test]
    fn mouse_button_is_reliable_and_snapshots_latest_anchor_sequence() {
        let (mut router, _, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));
        let outputs = router.handle(RouterCommand::Input(RouterInput::mouse_button(
            MouseButton::Left,
            ButtonState::Pressed,
            0,
            500,
            stamp(2),
        )));

        assert!(matches!(
            outputs.as_slice(),
            [RouterOutput::SendReliable {
                target: actual_target,
                frame: ReliableInputFrame {
                    event: ReliableInputEvent::MouseButton {
                        button: MouseButton::Left,
                        state: ButtonState::Pressed,
                        realtime_anchor_sequence: 0,
                        ..
                    },
                    ..
                },
            }] if *actual_target == target
        ));
    }

    #[test]
    fn normal_motion_never_rebuilds_routes_and_duplicate_updates_are_noops() {
        let (mut router, _, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        let generation = router.route_cache_generation();
        for value in 0..10_000 {
            let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
                500,
                500,
                stamp(value),
            )));
        }
        assert_eq!(router.route_cache_generation(), generation);

        let outputs = router.handle(RouterCommand::ConnectivityChanged {
            peer: target,
            connected: true,
        });
        assert!(outputs.is_empty());
        assert_eq!(router.route_cache_generation(), generation);
    }

    #[test]
    fn layout_change_rebuilds_local_virtual_desktop_geometry() {
        let (mut router, local, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(1920, 0, 1920, 1080),
        );
        let mut widened = LayoutGraph::new(local);
        widened.add_node(LayoutNode::new(local, 0, 0, 3840, 1080));
        widened.add_node(LayoutNode::new(target, 3840, 0, 1920, 1080));
        widened.add_link(LayoutLink::new(
            local,
            Direction::Right,
            target,
            Direction::Left,
        ));
        let _ = router.handle(RouterCommand::LayoutChanged(widened));

        let old_edge = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));
        let new_edge = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            3839,
            500,
            stamp(2),
        )));

        assert!(old_edge.is_empty());
        assert!(matches!(
            new_edge.first(),
            Some(RouterOutput::SendReliable {
                target: actual_target,
                frame: ReliableInputFrame {
                    event: ReliableInputEvent::Enter { x: 0, .. },
                    ..
                },
            }) if *actual_target == target
        ));
    }

    #[test]
    fn conservative_ledger_does_not_block_reliable_input_after_reentry() {
        let (mut router, _, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));
        let _ = router.handle(RouterCommand::Input(RouterInput::key(
            0x41,
            KeyState::Pressed,
            stamp(2),
        )));
        let _ = router.handle(RouterCommand::QuickReturn);
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(3),
        )));

        let outputs = router.handle(RouterCommand::Input(RouterInput::key(
            0x41,
            KeyState::Pressed,
            stamp(4),
        )));

        assert!(matches!(
            outputs.as_slice(),
            [RouterOutput::SendReliable {
                target: actual_target,
                frame: ReliableInputFrame {
                    session_epoch: SessionEpoch(2),
                    event: ReliableInputEvent::Key {
                        keycode: 0x41,
                        state: KeyState::Pressed,
                    },
                    ..
                },
            }] if *actual_target == target
        ));
        assert_eq!(router.held_key_count(), 1);
    }

    #[test]
    fn repeated_quick_return_is_idempotent_and_preserves_pending_release() {
        let (mut router, _, _) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));
        let _ = router.handle(RouterCommand::Input(RouterInput::key(
            0x41,
            KeyState::Pressed,
            stamp(2),
        )));
        let first = router.handle(RouterCommand::QuickReturn);
        let pending_token = router.pending_release_token();

        let repeated = router.handle(RouterCommand::QuickReturn);

        assert!(matches!(
            first.first(),
            Some(RouterOutput::EmergencyReleaseAll { .. })
        ));
        assert!(repeated.is_empty());
        assert_eq!(router.held_key_count(), 1);
        assert_eq!(router.pending_release_token(), pending_token);
    }

    #[test]
    fn realtime_sequence_overflow_releases_captured_target_before_suspending() {
        let (mut router, _, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));
        router.realtime_sequence = u64::MAX;

        let outputs = router.handle(RouterCommand::Input(RouterInput::relative_move(
            1,
            1,
            stamp(2),
        )));

        assert!(matches!(
            outputs.first(),
            Some(RouterOutput::EmergencyReleaseAll {
                target: actual_target,
                ..
            }) if *actual_target == target
        ));
        assert!(matches!(
            outputs.get(1),
            Some(RouterOutput::LocalSessionChanged(
                crate::ControlSessionState::Suspended { .. }
            ))
        ));
    }

    #[test]
    fn repeated_sessions_keep_pending_release_storage_bounded() {
        let (mut router, _, _) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );

        for round in 0..32 {
            let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
                1919,
                500,
                stamp(round * 3 + 1),
            )));
            let _ = router.handle(RouterCommand::Input(RouterInput::key(
                0x41,
                KeyState::Pressed,
                stamp(round * 3 + 2),
            )));
            let _ = router.handle(RouterCommand::QuickReturn);
        }

        assert_eq!(usize::from(router.pending_release.is_some()), 1);
        assert_eq!(router.held_key_count(), 1);
        assert!(router.pending_release_token().is_some());
    }

    #[test]
    fn counter_exhaustion_emits_max_emergency_sequence_only_once() {
        let (mut router, _, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        router.reliable_sequence = u64::MAX;

        let first = router.handle_counter_exhausted(target);
        let repeated = router.handle_counter_exhausted(target);

        assert!(matches!(
            first.first(),
            Some(RouterOutput::EmergencyReleaseAll {
                frame: ReliableInputFrame {
                    sequence: u64::MAX,
                    ..
                },
                ..
            })
        ));
        assert!(!repeated
            .iter()
            .any(|output| matches!(output, RouterOutput::EmergencyReleaseAll { .. })));
    }

    #[test]
    fn epoch_exhaustion_projects_suspended_state_without_network_frames() {
        let (mut router, _, _) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        router.epoch = SessionEpoch(u64::MAX);

        let outputs = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));

        assert!(matches!(
            outputs.as_slice(),
            [
                RouterOutput::LocalSessionChanged(crate::ControlSessionState::Suspended { .. }),
                RouterOutput::SuppressLocalShortcuts(false),
                RouterOutput::Metric(RouterMetric::CounterExhausted),
            ]
        ));
        assert!(!outputs.iter().any(|output| matches!(
            output,
            RouterOutput::SendRealtime { .. }
                | RouterOutput::SendReliable { .. }
                | RouterOutput::EmergencyReleaseAll { .. }
        )));
    }

    #[test]
    fn pressed_ledger_fault_releases_target_before_suspending() {
        let (mut router, _, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));

        let outputs = router.handle_pressed_ledger_fault(target);

        assert!(matches!(
            outputs.as_slice(),
            [
                RouterOutput::EmergencyReleaseAll {
                    target: actual_target,
                    ..
                },
                RouterOutput::LocalSessionChanged(
                    crate::ControlSessionState::Suspended { .. }
                ),
                RouterOutput::SuppressLocalShortcuts(false),
                RouterOutput::Metric(RouterMetric::PressedStateLedgerFault),
            ] if *actual_target == target
        ));
    }
}
