use std::collections::HashSet;

use smallvec::{smallvec, SmallVec};

use crate::{
    ButtonState, CaptureSessionStateMachine, ControlSessionState, DeviceId, Direction,
    GamepadButton, GamepadDeviceInfo, KeyState, LayoutGraph, MonotonicStamp, MouseButton,
    PendingReleaseBatch, PressedStateLedger, RealtimeInputFrame, RealtimeInputPayload,
    ReleaseAllReason, ReliableInputEvent, ReliableInputFrame, RouteCache, SessionEpoch,
    VirtualDesktopGeometry, INPUT_PROTOCOL_VERSION,
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
    TextCommit {
        text: String,
        captured_at: MonotonicStamp,
    },
    GamepadConnected {
        info: GamepadDeviceInfo,
        captured_at: MonotonicStamp,
    },
    GamepadDisconnected {
        gamepad_id: u8,
        captured_at: MonotonicStamp,
    },
    GamepadButton {
        gamepad_id: u8,
        button: GamepadButton,
        pressed: bool,
        captured_at: MonotonicStamp,
    },
    GamepadAxes {
        gamepad_id: u8,
        left_stick_x: i16,
        left_stick_y: i16,
        right_stick_x: i16,
        right_stick_y: i16,
        left_trigger: u16,
        right_trigger: u16,
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

    pub fn text_commit(text: impl Into<String>, captured_at: MonotonicStamp) -> Self {
        Self::TextCommit {
            text: text.into(),
            captured_at,
        }
    }

    pub const fn gamepad_connected(info: GamepadDeviceInfo, captured_at: MonotonicStamp) -> Self {
        Self::GamepadConnected { info, captured_at }
    }

    pub const fn gamepad_disconnected(gamepad_id: u8, captured_at: MonotonicStamp) -> Self {
        Self::GamepadDisconnected {
            gamepad_id,
            captured_at,
        }
    }

    pub const fn gamepad_button(
        gamepad_id: u8,
        button: GamepadButton,
        pressed: bool,
        captured_at: MonotonicStamp,
    ) -> Self {
        Self::GamepadButton {
            gamepad_id,
            button,
            pressed,
            captured_at,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn gamepad_axes(
        gamepad_id: u8,
        left_stick_x: i16,
        left_stick_y: i16,
        right_stick_x: i16,
        right_stick_y: i16,
        left_trigger: u16,
        right_trigger: u16,
        captured_at: MonotonicStamp,
    ) -> Self {
        Self::GamepadAxes {
            gamepad_id,
            left_stick_x,
            left_stick_y,
            right_stick_x,
            right_stick_y,
            left_trigger,
            right_trigger,
            captured_at,
        }
    }

    fn captured_at(&self) -> MonotonicStamp {
        match self {
            Self::AbsoluteMove { captured_at, .. }
            | Self::RelativeMove { captured_at, .. }
            | Self::Key { captured_at, .. }
            | Self::MouseButton { captured_at, .. }
            | Self::Wheel { captured_at, .. }
            | Self::TextCommit { captured_at, .. }
            | Self::GamepadConnected { captured_at, .. }
            | Self::GamepadDisconnected { captured_at, .. }
            | Self::GamepadButton { captured_at, .. }
            | Self::GamepadAxes { captured_at, .. } => *captured_at,
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
    SystemSuspended,
    BackendDegraded,
    LeaseExpired,
    Shutdown,
    ReleaseAllCompleted { token: u64, success: bool },
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
        release_token: Option<u64>,
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

#[derive(Debug, Clone, Copy)]
struct EmergencyReleaseStatus {
    ledger_fault: bool,
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
            RouterCommand::SystemSuspended => self.return_to_local(ReleaseAllReason::Suspended),
            RouterCommand::BackendDegraded => self.handle_backend_degraded(),
            RouterCommand::LeaseExpired => self.return_to_local(ReleaseAllReason::Timeout),
            RouterCommand::Shutdown => self.return_to_local(ReleaseAllReason::SessionEnded),
            RouterCommand::ReleaseAllCompleted { token, success } => {
                self.handle_release_all_completed(token, success)
            }
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
            RouterInput::TextCommit { text, captured_at } => {
                let Some(target) = self.session.active_target() else {
                    return SmallVec::new();
                };
                self.send_reliable(target, captured_at, ReliableInputEvent::TextCommit { text })
            }
            RouterInput::GamepadConnected { info, captured_at } => {
                let Some(target) = self.session.active_target() else {
                    return SmallVec::new();
                };
                self.send_reliable(
                    target,
                    captured_at,
                    ReliableInputEvent::GamepadConnected { info },
                )
            }
            RouterInput::GamepadDisconnected {
                gamepad_id,
                captured_at,
            } => {
                let Some(target) = self.session.active_target() else {
                    return SmallVec::new();
                };
                self.send_reliable(
                    target,
                    captured_at,
                    ReliableInputEvent::GamepadDisconnected { gamepad_id },
                )
            }
            RouterInput::GamepadButton {
                gamepad_id,
                button,
                pressed,
                captured_at,
            } => {
                let Some(target) = self.session.active_target() else {
                    return SmallVec::new();
                };
                self.send_reliable(
                    target,
                    captured_at,
                    ReliableInputEvent::GamepadButton {
                        gamepad_id,
                        button,
                        pressed,
                    },
                )
            }
            RouterInput::GamepadAxes {
                gamepad_id,
                left_stick_x,
                left_stick_y,
                right_stick_x,
                right_stick_y,
                left_trigger,
                right_trigger,
                captured_at,
            } => {
                let Some(target) = self.session.active_target() else {
                    return SmallVec::new();
                };
                self.send_realtime(
                    target,
                    captured_at,
                    RealtimeInputPayload::GamepadAxes {
                        gamepad_id,
                        left_stick_x,
                        left_stick_y,
                        right_stick_x,
                        right_stick_y,
                        left_trigger,
                        right_trigger,
                    },
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
        if self.pending_release.is_some() {
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
        self.send_realtime(
            target,
            captured_at,
            RealtimeInputPayload::RelativeMouse { dx, dy },
        )
    }

    fn send_realtime(
        &mut self,
        target: DeviceId,
        captured_at: MonotonicStamp,
        payload: RealtimeInputPayload,
    ) -> SmallVec<[RouterOutput; 4]> {
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
                payload,
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
        let active_route = match self.session.state() {
            ControlSessionState::RemoteActive {
                target,
                entered_via,
            } => Some((target, entered_via)),
            _ => None,
        };
        if let Some(geometry) = layout
            .get_node(self.local_id)
            .and_then(VirtualDesktopGeometry::from_layout_node)
        {
            self.geometry = geometry;
        }
        self.layout = layout;
        let rebuilt = self.rebuild_routes();
        let Some((target, entered_via)) = active_route else {
            return rebuilt;
        };
        if self
            .routes
            .route(entered_via)
            .is_some_and(|route| route.device_id == target)
        {
            return rebuilt;
        }

        let mut outputs = SmallVec::new();
        let release =
            self.push_emergency_release(&mut outputs, target, ReleaseAllReason::Suspended);
        self.session.on_target_disconnect(target);
        outputs.push(RouterOutput::LocalSessionChanged(self.session.state()));
        outputs.push(RouterOutput::SuppressLocalShortcuts(false));
        if release.ledger_fault {
            outputs.push(RouterOutput::Metric(RouterMetric::PressedStateLedgerFault));
        } else {
            outputs.extend(rebuilt);
        }
        outputs
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
            ledger_fault = self
                .push_emergency_release(&mut outputs, peer, ReleaseAllReason::Suspended)
                .ledger_fault;
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
            ledger_fault = self
                .push_emergency_release(&mut outputs, target, ReleaseAllReason::BackendFailure)
                .ledger_fault;
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
        let release = self.push_emergency_release(&mut outputs, target, reason);
        if release.ledger_fault {
            self.session.on_backend_degraded();
        } else {
            let _ = self.session.on_return_edge_hit(return_edge);
        }
        if !self.session.is_remote_active() {
            outputs.push(RouterOutput::LocalSessionChanged(self.session.state()));
            outputs.push(RouterOutput::SuppressLocalShortcuts(false));
        }
        if release.ledger_fault {
            outputs.push(RouterOutput::Metric(RouterMetric::PressedStateLedgerFault));
        }
        outputs
    }

    fn push_emergency_release(
        &mut self,
        outputs: &mut SmallVec<[RouterOutput; 4]>,
        target: DeviceId,
        reason: ReleaseAllReason,
    ) -> EmergencyReleaseStatus {
        let Some(sequence) = self.take_emergency_reliable_sequence() else {
            outputs.push(RouterOutput::Metric(RouterMetric::CounterExhausted));
            return EmergencyReleaseStatus {
                ledger_fault: false,
            };
        };
        let (release_token, ledger_fault) = if let Some(batch) = &self.pending_release {
            (Some(batch.token()), false)
        } else {
            match self.pressed.release_all_events(reason) {
                Ok(batch) => {
                    let token = batch.token();
                    self.pending_release = Some(batch);
                    (Some(token), false)
                }
                Err(_) => (None, true),
            }
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
            release_token,
        });
        EmergencyReleaseStatus { ledger_fault }
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

    fn handle_release_all_completed(
        &mut self,
        token: u64,
        success: bool,
    ) -> SmallVec<[RouterOutput; 4]> {
        if !success
            || self
                .pending_release
                .as_ref()
                .is_none_or(|pending| pending.token() != token)
        {
            return SmallVec::new();
        }
        if let Some(batch) = self.pending_release.take() {
            self.pressed.confirm_release_all(&batch);
        }
        SmallVec::new()
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
            self.routes = RouteCache::empty(self.routes.generation());
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
        ButtonState, ClockDomainId, Direction, DisplayNode, GamepadButton, GamepadDeviceInfo,
        KeyState, LayoutGraph, LayoutLink, LayoutNode, MonotonicStamp, MouseButton, PixelRect,
        RealtimeInputFrame, RealtimeInputPayload, ReliableInputEvent, ReliableInputFrame,
        SessionEpoch,
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

    fn emergency_release_token(outputs: &[RouterOutput]) -> u64 {
        match outputs.first() {
            Some(RouterOutput::EmergencyReleaseAll {
                release_token: Some(token),
                ..
            }) => *token,
            other => panic!("expected emergency release token, got {other:?}"),
        }
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
                    release_token: Some(token),
                },
                RouterOutput::LocalSessionChanged(crate::ControlSessionState::LocalReady),
                RouterOutput::SuppressLocalShortcuts(false),
            ] if *actual_target == target
                && router.pending_release_token() == Some(*token)
        ));
        assert_eq!(router.held_key_count(), 1);
        assert!(router.pending_release_token().is_some());
    }

    #[test]
    fn system_suspend_emits_suspended_release_and_clears_shortcut_suppression() {
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
        let outputs = router.handle(RouterCommand::SystemSuspended);

        assert!(matches!(
            outputs.as_slice(),
            [
                RouterOutput::EmergencyReleaseAll {
                    target: actual_target,
                    frame: ReliableInputFrame {
                        event: ReliableInputEvent::ReleaseAll {
                            reason: ReleaseAllReason::Suspended,
                        },
                        ..
                    },
                    ..
                },
                RouterOutput::LocalSessionChanged(crate::ControlSessionState::LocalReady),
                RouterOutput::SuppressLocalShortcuts(false),
            ] if *actual_target == target
        ));
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
                ..
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
    fn layout_change_invalidates_active_route_before_suspending_for_missing_parts() {
        for missing_part in ["link", "node", "primary-display"] {
            let (mut router, local, target) = linked_router(
                PixelRect::new(0, 0, 1920, 1080),
                Direction::Right,
                Direction::Left,
                PixelRect::new(1920, 0, 1920, 1080),
            );
            let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
                1919,
                500,
                stamp(1),
            )));
            let mut changed = LayoutGraph::new(local);
            changed.add_node(LayoutNode::new(local, 0, 0, 1920, 1080));
            if missing_part != "node" {
                if missing_part == "primary-display" {
                    changed.add_node(LayoutNode {
                        device_id: target,
                        displays: vec![DisplayNode::secondary(
                            "secondary".to_string(),
                            1920,
                            0,
                            1920,
                            1080,
                        )],
                    });
                } else {
                    changed.add_node(LayoutNode::new(target, 1920, 0, 1920, 1080));
                }
            }
            if missing_part != "link" {
                changed.add_link(LayoutLink::new(
                    local,
                    Direction::Right,
                    target,
                    Direction::Left,
                ));
            }

            let outputs = router.handle(RouterCommand::LayoutChanged(changed));

            assert!(
                matches!(
                    outputs.first(),
                    Some(RouterOutput::EmergencyReleaseAll {
                        target: actual_target,
                        ..
                    }) if *actual_target == target
                ),
                "missing {missing_part}"
            );
            assert!(matches!(
                outputs.get(1),
                Some(RouterOutput::LocalSessionChanged(
                    crate::ControlSessionState::Suspended { .. }
                ))
            ));
            assert!(matches!(
                outputs.get(2),
                Some(RouterOutput::SuppressLocalShortcuts(false))
            ));
        }
    }

    #[test]
    fn unrelated_layout_change_keeps_active_route_and_session() {
        let (mut router, local, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(1920, 0, 1920, 1080),
        );
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));
        let unrelated = crate::DeviceId::new_v4();
        let mut changed = router.layout.clone();
        changed.add_node(LayoutNode::new(unrelated, -1280, 0, 1280, 720));
        changed.add_link(LayoutLink::new(
            local,
            Direction::Left,
            unrelated,
            Direction::Right,
        ));

        let outputs = router.handle(RouterCommand::LayoutChanged(changed));
        let moved = router.handle(RouterCommand::Input(RouterInput::relative_move(
            4,
            -2,
            stamp(2),
        )));

        assert!(matches!(
            outputs.as_slice(),
            [RouterOutput::Metric(RouterMetric::RouteCacheRebuilt { .. })]
        ));
        assert!(matches!(
            moved.as_slice(),
            [RouterOutput::SendRealtime {
                target: actual_target,
                ..
            }] if *actual_target == target
        ));
    }

    #[test]
    fn pending_release_blocks_reentry_until_matching_success() {
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
        let released = router.handle(RouterCommand::QuickReturn);
        let token = emergency_release_token(&released);

        let blocked = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(3),
        )));
        assert!(blocked.is_empty());
        assert_eq!(router.epoch, SessionEpoch(1));

        assert!(router
            .handle(RouterCommand::ReleaseAllCompleted {
                token,
                success: false,
            })
            .is_empty());
        assert!(router
            .handle(RouterCommand::ReleaseAllCompleted {
                token: token + 1,
                success: true,
            })
            .is_empty());
        assert_eq!(router.pending_release_token(), Some(token));
        assert_eq!(router.held_key_count(), 1);
        assert!(router
            .handle(RouterCommand::Input(RouterInput::absolute_move(
                1919,
                500,
                stamp(4),
            )))
            .is_empty());

        assert!(router
            .handle(RouterCommand::ReleaseAllCompleted {
                token,
                success: true,
            })
            .is_empty());
        assert_eq!(router.pending_release_token(), None);
        assert_eq!(router.held_key_count(), 0);
        let entered = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(5),
        )));

        assert!(matches!(
            entered.first(),
            Some(
                RouterOutput::SendReliable {
                    target: actual_target,
                    frame: ReliableInputFrame {
                        session_epoch: SessionEpoch(2),
                        event: ReliableInputEvent::Enter { .. },
                        ..
                    },
                }
            ) if *actual_target == target
        ));
    }

    #[test]
    fn late_release_completion_token_cannot_clear_a_new_batch() {
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
        let first_token = emergency_release_token(&router.handle(RouterCommand::QuickReturn));
        let _ = router.handle(RouterCommand::ReleaseAllCompleted {
            token: first_token,
            success: true,
        });
        let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(3),
        )));
        let _ = router.handle(RouterCommand::Input(RouterInput::key(
            0x42,
            KeyState::Pressed,
            stamp(4),
        )));
        let second_token = emergency_release_token(&router.handle(RouterCommand::QuickReturn));

        let _ = router.handle(RouterCommand::ReleaseAllCompleted {
            token: first_token,
            success: true,
        });

        assert_ne!(first_token, second_token);
        assert_eq!(router.pending_release_token(), Some(second_token));
        assert_eq!(router.held_key_count(), 1);
        let _ = router.handle(RouterCommand::ReleaseAllCompleted {
            token: second_token,
            success: true,
        });
        assert_eq!(router.pending_release_token(), None);
        assert_eq!(router.held_key_count(), 0);
    }

    #[test]
    fn reliable_input_after_confirmed_reentry_is_not_blocked_by_old_ledger_state() {
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
        let token = emergency_release_token(&router.handle(RouterCommand::QuickReturn));
        let _ = router.handle(RouterCommand::ReleaseAllCompleted {
            token,
            success: true,
        });
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

        let mut previous_token = None;
        for round in 0..32 {
            let _ = router.handle(RouterCommand::Input(RouterInput::absolute_move(
                1919,
                500,
                stamp(round * 3 + 1),
            )));
            let _ = router.handle(RouterCommand::Input(RouterInput::key(
                0x100 + round as u32,
                KeyState::Pressed,
                stamp(round * 3 + 2),
            )));
            let released = router.handle(RouterCommand::QuickReturn);
            let token = emergency_release_token(&released);
            assert!(previous_token.is_none_or(|previous| token > previous));
            assert_eq!(usize::from(router.pending_release.is_some()), 1);
            assert_eq!(router.held_key_count(), 1);
            let _ = router.handle(RouterCommand::ReleaseAllCompleted {
                token,
                success: true,
            });
            assert_eq!(router.pending_release_token(), None);
            assert_eq!(router.held_key_count(), 0);
            previous_token = Some(token);
        }

        assert_eq!(usize::from(router.pending_release.is_some()), 0);
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

    #[test]
    fn task4_reliable_inputs_share_active_target_epoch_and_reliable_sequence() {
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
        let info = GamepadDeviceInfo {
            gamepad_id: 2,
            name: "Pad".to_string(),
            vendor_id: Some(0x1234),
            product_id: Some(0x5678),
        };
        let commands = [
            (
                RouterInput::TextCommit {
                    text: "你好".to_string(),
                    captured_at: stamp(2),
                },
                ReliableInputEvent::TextCommit {
                    text: "你好".to_string(),
                },
            ),
            (
                RouterInput::GamepadConnected {
                    info: info.clone(),
                    captured_at: stamp(3),
                },
                ReliableInputEvent::GamepadConnected { info: info.clone() },
            ),
            (
                RouterInput::GamepadDisconnected {
                    gamepad_id: 2,
                    captured_at: stamp(4),
                },
                ReliableInputEvent::GamepadDisconnected { gamepad_id: 2 },
            ),
            (
                RouterInput::GamepadButton {
                    gamepad_id: 2,
                    button: GamepadButton::South,
                    pressed: true,
                    captured_at: stamp(5),
                },
                ReliableInputEvent::GamepadButton {
                    gamepad_id: 2,
                    button: GamepadButton::South,
                    pressed: true,
                },
            ),
        ];

        for (index, (input, expected_event)) in commands.into_iter().enumerate() {
            let outputs = router.handle(RouterCommand::Input(input));
            assert!(matches!(
                outputs.as_slice(),
                [RouterOutput::SendReliable {
                    target: actual_target,
                    frame: ReliableInputFrame {
                        session_epoch: SessionEpoch(1),
                        sequence,
                        event,
                        ..
                    },
                }] if *actual_target == target
                    && *sequence == index as u64 + 1
                    && *event == expected_event
            ));
        }
    }

    #[test]
    fn task4_extended_input_constructors_preserve_semantic_payloads() {
        let info = GamepadDeviceInfo {
            gamepad_id: 4,
            name: "Constructor Pad".to_string(),
            vendor_id: Some(1),
            product_id: Some(2),
        };
        assert!(matches!(
            RouterInput::text_commit("hello", stamp(1)),
            RouterInput::TextCommit { text, .. } if text == "hello"
        ));
        assert!(matches!(
            RouterInput::gamepad_connected(info.clone(), stamp(2)),
            RouterInput::GamepadConnected {
                info: actual_info,
                ..
            } if actual_info == info
        ));
        assert!(matches!(
            RouterInput::gamepad_disconnected(4, stamp(3)),
            RouterInput::GamepadDisconnected { gamepad_id: 4, .. }
        ));
        assert!(matches!(
            RouterInput::gamepad_button(4, GamepadButton::Guide, true, stamp(4)),
            RouterInput::GamepadButton {
                gamepad_id: 4,
                button: GamepadButton::Guide,
                pressed: true,
                ..
            }
        ));
        assert!(matches!(
            RouterInput::gamepad_axes(4, -1, 2, -3, 4, 5, 6, stamp(5)),
            RouterInput::GamepadAxes {
                gamepad_id: 4,
                left_stick_x: -1,
                left_stick_y: 2,
                right_stick_x: -3,
                right_stick_y: 4,
                left_trigger: 5,
                right_trigger: 6,
                ..
            }
        ));
    }

    #[test]
    fn exhausted_route_cache_rebuild_clears_stale_routes_and_suspends_active_session() {
        let (mut active, local, target) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(1920, 0, 1920, 1080),
        );
        let _ = active.handle(RouterCommand::Input(RouterInput::absolute_move(
            1919,
            500,
            stamp(1),
        )));
        active.routes = RouteCache::build(&active.layout, local, &active.connected_peers, u64::MAX);
        let mut without_route = LayoutGraph::new(local);
        without_route.add_node(LayoutNode::new(local, 0, 0, 1920, 1080));
        without_route.add_node(LayoutNode::new(target, 1920, 0, 1920, 1080));

        let outputs = active.handle(RouterCommand::LayoutChanged(without_route.clone()));

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
        assert!(active.routes.route(Direction::Right).is_none());

        let (mut local_only, local, _) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(1920, 0, 1920, 1080),
        );
        local_only.routes = RouteCache::build(
            &local_only.layout,
            local,
            &local_only.connected_peers,
            u64::MAX,
        );
        let _ = local_only.handle(RouterCommand::LayoutChanged(without_route));
        assert!(local_only
            .handle(RouterCommand::Input(RouterInput::absolute_move(
                1919,
                500,
                stamp(2),
            )))
            .is_empty());
    }

    #[test]
    fn gamepad_axes_use_realtime_sequence_independent_from_reliable_input() {
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
        let _ = router.handle(RouterCommand::Input(RouterInput::TextCommit {
            text: "a".to_string(),
            captured_at: stamp(2),
        }));

        let outputs = router.handle(RouterCommand::Input(RouterInput::GamepadAxes {
            gamepad_id: 3,
            left_stick_x: -100,
            left_stick_y: 200,
            right_stick_x: -300,
            right_stick_y: 400,
            left_trigger: 500,
            right_trigger: 600,
            captured_at: stamp(3),
        }));

        assert!(matches!(
            outputs.as_slice(),
            [RouterOutput::SendRealtime {
                target: actual_target,
                frame: RealtimeInputFrame {
                    session_epoch: SessionEpoch(1),
                    sequence: 1,
                    payload: RealtimeInputPayload::GamepadAxes {
                        gamepad_id: 3,
                        left_stick_x: -100,
                        left_stick_y: 200,
                        right_stick_x: -300,
                        right_stick_y: 400,
                        left_trigger: 500,
                        right_trigger: 600,
                    },
                    ..
                },
            }] if *actual_target == target
        ));
    }

    #[test]
    fn inactive_router_drops_all_task4_extended_inputs() {
        let (mut router, _, _) = linked_router(
            PixelRect::new(0, 0, 1920, 1080),
            Direction::Right,
            Direction::Left,
            PixelRect::new(0, 0, 1920, 1080),
        );
        let inputs = vec![
            RouterInput::TextCommit {
                text: "local".to_string(),
                captured_at: stamp(1),
            },
            RouterInput::GamepadConnected {
                info: GamepadDeviceInfo {
                    gamepad_id: 1,
                    name: "Pad".to_string(),
                    vendor_id: None,
                    product_id: None,
                },
                captured_at: stamp(2),
            },
            RouterInput::GamepadDisconnected {
                gamepad_id: 1,
                captured_at: stamp(3),
            },
            RouterInput::GamepadButton {
                gamepad_id: 1,
                button: GamepadButton::East,
                pressed: false,
                captured_at: stamp(4),
            },
            RouterInput::GamepadAxes {
                gamepad_id: 1,
                left_stick_x: 0,
                left_stick_y: 0,
                right_stick_x: 0,
                right_stick_y: 0,
                left_trigger: 0,
                right_trigger: 0,
                captured_at: stamp(5),
            },
        ];

        for input in inputs {
            assert!(router.handle(RouterCommand::Input(input)).is_empty());
        }
    }
}
