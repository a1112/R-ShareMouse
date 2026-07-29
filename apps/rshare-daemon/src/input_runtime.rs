use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rshare_core::{
    AuthenticatedInputOwner, ButtonState, ControlConnectionId, DeviceId, InputRouter, KeyState,
    MouseButton, RealtimeInputFrame, ReleaseAllReason, ReliableInputEvent, ReliableInputFrame,
    RouterCommand, RouterOutput, SessionEpoch,
};
use rshare_input::{
    CapturedInput, CapturedInputPayload, ContinuousInput, IngressEvent, IngressFault,
    InjectionQueueFull, InputEvent, InputInjectionHandle, PointerSample, SemanticInputConsumer,
};
use rshare_net::qos::{ConnectionRegistry, RegisteredPeer};
use rshare_net::PeerInbound;
use rshare_platform::SystemSafetyEvent;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;

use crate::input_state::{
    ControlMetrics, InputDiscreteProjection, InputPointerProjection, InputStatePublisher,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputDispatch {
    Realtime {
        target: DeviceId,
        frame: RealtimeInputFrame,
    },
    Reliable {
        target: DeviceId,
        frame: ReliableInputFrame,
    },
}

/// Nonblocking output seam. Implementations must clone a generation-scoped
/// transport handle and return without waiting for network I/O.
pub trait InputTransport: Send + Sync + 'static {
    type Binding: Clone + Send + Sync + 'static;

    fn bind(&self, target: DeviceId) -> Option<Self::Binding>;
    fn try_send_realtime(&self, binding: &Self::Binding, frame: RealtimeInputFrame);
    fn try_send_reliable(&self, binding: &Self::Binding, frame: ReliableInputFrame) -> bool;
}

impl InputTransport for ConnectionRegistry {
    type Binding = RegisteredPeer;

    fn bind(&self, target: DeviceId) -> Option<Self::Binding> {
        self.peer(&target)
    }

    fn try_send_realtime(&self, binding: &Self::Binding, frame: RealtimeInputFrame) {
        let _ = binding.transport.try_send_realtime(frame);
    }

    fn try_send_reliable(&self, binding: &Self::Binding, frame: ReliableInputFrame) -> bool {
        binding.transport.try_send_reliable_input(frame).is_ok()
    }
}

/// Single-owner daemon input actor.
pub struct InputRuntime<T: InputTransport = ConnectionRegistry> {
    ingress: SemanticInputConsumer,
    router: InputRouter,
    transports: Arc<T>,
    state: InputStatePublisher,
    metrics: Arc<ControlMetrics>,
    injection: InputInjectionHandle,
    pressed_keys: BTreeSet<u32>,
    pressed_buttons: BTreeSet<u8>,
    current_epoch: SessionEpoch,
    last_replaced: u64,
    last_dropped: u64,
    last_overflow: u64,
    active_transports: HashMap<DeviceId, T::Binding>,
}

impl<T: InputTransport> InputRuntime<T> {
    pub fn new(
        ingress: SemanticInputConsumer,
        router: InputRouter,
        transports: Arc<T>,
        state: InputStatePublisher,
        metrics: Arc<ControlMetrics>,
        injection: InputInjectionHandle,
    ) -> Self {
        Self {
            ingress,
            router,
            transports,
            state,
            metrics,
            injection,
            pressed_keys: BTreeSet::new(),
            pressed_buttons: BTreeSet::new(),
            current_epoch: SessionEpoch(0),
            last_replaced: 0,
            last_dropped: 0,
            last_overflow: 0,
            active_transports: HashMap::new(),
        }
    }

    pub fn route_cache_generation(&self) -> u64 {
        self.router.route_cache_generation()
    }

    pub fn session_epoch(&self) -> SessionEpoch {
        self.current_epoch
    }

    pub async fn process_next(&mut self) -> bool {
        let Some(event) = self.ingress.recv_event().await else {
            return false;
        };
        self.process_ingress_event(event);
        true
    }

    pub fn process_ready(&mut self) -> bool {
        if let Some(fault) = self.ingress.try_pop_fault() {
            self.process_ingress_event(IngressEvent::Fault(fault));
            return true;
        }
        let Some(input) = self.ingress.try_recv() else {
            return false;
        };
        self.process_ingress_event(IngressEvent::Input(input));
        true
    }

    pub fn handle_command(&mut self, command: RouterCommand) {
        if matches!(command, RouterCommand::SystemSuspended) {
            self.injection
                .request_release_all_sources(ReleaseAllReason::Suspended);
        }
        if matches!(
            command,
            RouterCommand::BackendDegraded
                | RouterCommand::LeaseExpired
                | RouterCommand::Shutdown
                | RouterCommand::QuickReturn
        ) {
            self.injection.request_release_all(match command {
                RouterCommand::LeaseExpired => ReleaseAllReason::Timeout,
                RouterCommand::BackendDegraded => ReleaseAllReason::BackendFailure,
                RouterCommand::QuickReturn => ReleaseAllReason::OwnershipTransfer,
                _ => ReleaseAllReason::SessionEnded,
            });
        }
        let outputs = self.router.handle(command);
        self.dispatch_outputs(outputs);
    }

    pub async fn run(mut self, mut commands: mpsc::Receiver<RouterCommand>) {
        loop {
            tokio::select! {
                biased;
                command = commands.recv() => {
                    match command {
                        Some(command) => self.handle_command(command),
                        None => {
                            self.handle_command(RouterCommand::Shutdown);
                            break;
                        }
                    }
                }
                event = self.ingress.recv_event() => {
                    match event {
                        Some(event) => self.process_ingress_event(event),
                        None => {
                            self.handle_command(RouterCommand::Shutdown);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Runs with an independent, unbounded low-frequency safety lane.
    ///
    /// Safety events are biased ahead of ordinary commands and input. The
    /// native callback also closes injection admission synchronously through
    /// `dispatch_system_safety_event`, so command-queue saturation cannot keep
    /// held input alive.
    pub async fn run_with_safety(
        mut self,
        mut commands: mpsc::Receiver<RouterCommand>,
        mut safety_events: mpsc::UnboundedReceiver<SystemSafetyEvent>,
    ) {
        let mut safety_open = true;
        loop {
            tokio::select! {
                biased;
                safety = safety_events.recv(), if safety_open => {
                    match safety {
                        Some(_) => self.handle_command(RouterCommand::SystemSuspended),
                        None => safety_open = false,
                    }
                }
                command = commands.recv() => {
                    match command {
                        Some(command) => self.handle_command(command),
                        None => {
                            self.handle_command(RouterCommand::Shutdown);
                            break;
                        }
                    }
                }
                event = self.ingress.recv_event() => {
                    match event {
                        Some(event) => self.process_ingress_event(event),
                        None => {
                            self.handle_command(RouterCommand::Shutdown);
                            break;
                        }
                    }
                }
            }
        }
    }

    fn process_ingress_event(&mut self, event: IngressEvent) {
        self.observe_ingress_stats();
        match event {
            IngressEvent::Fault(IngressFault::ReliableOverflow) => {
                self.injection
                    .request_release_all(ReleaseAllReason::BackendFailure);
                let outputs = self.router.handle(RouterCommand::BackendDegraded);
                self.dispatch_outputs(outputs);
                if let Ok(next_epoch) = self.current_epoch.next() {
                    self.current_epoch = next_epoch;
                    self.pressed_keys.clear();
                    self.pressed_buttons.clear();
                    self.state.publish_discrete(InputDiscreteProjection {
                        session_epoch: next_epoch,
                        pressed_keys: Vec::new(),
                        pressed_buttons: Vec::new(),
                    });
                }
            }
            IngressEvent::Input(input) => self.process_captured(input),
        }
    }

    fn process_captured(&mut self, input: CapturedInput) {
        self.metrics.record_captured();
        let pointer = input.pointer;
        let Some(router_input) = captured_to_router_input(input) else {
            return;
        };
        let outputs = self.router.handle(RouterCommand::Input(router_input));
        self.dispatch_outputs(outputs);
        if let Some(pointer) = pointer {
            self.state.publish_pointer(InputPointerProjection {
                session_epoch: self.current_epoch,
                x: pointer.x,
                y: pointer.y,
            });
        }
    }

    fn dispatch_outputs(&mut self, outputs: impl IntoIterator<Item = RouterOutput>) {
        for output in outputs {
            match output {
                RouterOutput::SendRealtime { target, frame } => {
                    self.current_epoch = frame.session_epoch;
                    if let Some(binding) = self.active_transports.get(&target) {
                        self.transports.try_send_realtime(binding, frame);
                        self.metrics.record_routed();
                    }
                }
                RouterOutput::SendReliable { target, frame } => {
                    self.current_epoch = frame.session_epoch;
                    if matches!(frame.event, ReliableInputEvent::Enter { .. }) {
                        if let Some(binding) = self.transports.bind(target) {
                            self.active_transports.insert(target, binding);
                        }
                    }
                    let success = self.active_transports.get(&target).is_some_and(|binding| {
                        self.transports.try_send_reliable(binding, frame.clone())
                    });
                    if success {
                        self.metrics.record_routed();
                        self.apply_reliable_projection(&frame);
                    } else {
                        let failure = self.router.handle(RouterCommand::BackendDegraded);
                        self.dispatch_outputs(failure);
                    }
                }
                RouterOutput::EmergencyReleaseAll {
                    target,
                    frame,
                    release_token,
                } => {
                    self.current_epoch = frame.session_epoch;
                    let success = self.active_transports.get(&target).is_some_and(|binding| {
                        self.transports.try_send_reliable(binding, frame.clone())
                    });
                    if success {
                        self.metrics.record_routed();
                        self.apply_reliable_projection(&frame);
                    }
                    self.active_transports.remove(&target);
                    if let Some(token) = release_token {
                        let completion = self
                            .router
                            .handle(RouterCommand::ReleaseAllCompleted { token, success });
                        self.dispatch_outputs(completion);
                    }
                }
                RouterOutput::LocalSessionChanged(session) => {
                    self.state.publish_session(session);
                }
                RouterOutput::SuppressLocalShortcuts(_) | RouterOutput::Metric(_) => {}
            }
        }
    }

    fn apply_reliable_projection(&mut self, frame: &ReliableInputFrame) {
        match &frame.event {
            ReliableInputEvent::Key { keycode, state } => match state {
                KeyState::Pressed => {
                    self.pressed_keys.insert(*keycode);
                }
                KeyState::Released => {
                    self.pressed_keys.remove(keycode);
                }
            },
            ReliableInputEvent::MouseButton { button, state, .. } => match state {
                ButtonState::Pressed => {
                    self.pressed_buttons.insert(button.to_code());
                }
                ButtonState::Released => {
                    self.pressed_buttons.remove(&button.to_code());
                }
            },
            ReliableInputEvent::ReleaseAll { .. } | ReliableInputEvent::Leave => {
                self.pressed_keys.clear();
                self.pressed_buttons.clear();
            }
            _ => {}
        }
        self.state.publish_discrete(InputDiscreteProjection {
            session_epoch: frame.session_epoch,
            pressed_keys: self.pressed_keys.iter().copied().collect(),
            pressed_buttons: self
                .pressed_buttons
                .iter()
                .copied()
                .map(MouseButton::from_code)
                .collect(),
        });
    }

    fn observe_ingress_stats(&mut self) {
        let stats = self.ingress.stats();
        self.metrics
            .record_realtime_replaced(stats.replaced_realtime.saturating_sub(self.last_replaced));
        self.metrics
            .record_realtime_dropped(stats.dropped_realtime.saturating_sub(self.last_dropped));
        self.metrics
            .record_reliable_overflow(stats.reliable_overflow.saturating_sub(self.last_overflow));
        self.last_replaced = stats.replaced_realtime;
        self.last_dropped = stats.dropped_realtime;
        self.last_overflow = stats.reliable_overflow;
    }
}

/// Synchronous native-callback bridge for lock/suspend events.
///
/// The actor close uses its reserved control path before the notification is
/// queued for router cleanup, so this remains fail-safe when the ordinary
/// router command queue is full.
pub fn dispatch_system_safety_event(
    injection: &InputInjectionHandle,
    safety_tx: &mpsc::UnboundedSender<SystemSafetyEvent>,
    event: SystemSafetyEvent,
) -> bool {
    injection.request_release_all_sources(ReleaseAllReason::Suspended);
    safety_tx.send(event).is_ok()
}

fn captured_to_router_input(input: CapturedInput) -> Option<rshare_core::RouterInput> {
    let captured_at = input.captured_at;
    match input.payload {
        CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Absolute {
            x,
            y,
        })) => Some(rshare_core::RouterInput::absolute_move(x, y, captured_at)),
        CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Relative {
            dx,
            dy,
            ..
        })) => Some(rshare_core::RouterInput::relative_move(dx, dy, captured_at)),
        CapturedInputPayload::Continuous(ContinuousInput::GamepadAxes(axes)) => {
            Some(rshare_core::RouterInput::gamepad_axes(
                axes.gamepad_id,
                axes.left_stick_x,
                axes.left_stick_y,
                axes.right_stick_x,
                axes.right_stick_y,
                axes.left_trigger,
                axes.right_trigger,
                captured_at,
            ))
        }
        CapturedInputPayload::Discrete(event) => match event {
            InputEvent::MouseMove { x, y } => {
                Some(rshare_core::RouterInput::absolute_move(x, y, captured_at))
            }
            InputEvent::MouseButton { button, state } => {
                let pointer = input.pointer?;
                Some(rshare_core::RouterInput::mouse_button(
                    MouseButton::from_code(button.to_code()),
                    if state.is_pressed() {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    },
                    pointer.x,
                    pointer.y,
                    captured_at,
                ))
            }
            InputEvent::MouseWheel { delta_x, delta_y } => Some(rshare_core::RouterInput::wheel(
                delta_x,
                delta_y,
                captured_at,
            )),
            InputEvent::Key { keycode, state } | InputEvent::KeyExtended { keycode, state, .. } => {
                Some(rshare_core::RouterInput::key(
                    keycode.to_raw(),
                    if state.is_pressed() {
                        KeyState::Pressed
                    } else {
                        KeyState::Released
                    },
                    captured_at,
                ))
            }
            InputEvent::TextCommit { text } => {
                Some(rshare_core::RouterInput::text_commit(text, captured_at))
            }
            InputEvent::GamepadConnected { info } => Some(
                rshare_core::RouterInput::gamepad_connected(info, captured_at),
            ),
            InputEvent::GamepadDisconnected { gamepad_id } => Some(
                rshare_core::RouterInput::gamepad_disconnected(gamepad_id, captured_at),
            ),
            InputEvent::GamepadButton {
                gamepad_id,
                button,
                pressed,
                ..
            } => Some(rshare_core::RouterInput::gamepad_button(
                gamepad_id,
                button,
                pressed,
                captured_at,
            )),
            InputEvent::GamepadState { state } => Some(rshare_core::RouterInput::gamepad_axes(
                state.gamepad_id,
                state.left_stick_x,
                state.left_stick_y,
                state.right_stick_x,
                state.right_stick_y,
                state.left_trigger,
                state.right_trigger,
                captured_at,
            )),
        },
    }
}

/// Consume Task-10 typed input lanes and feed the Task-11 injection actor.
pub async fn run_authenticated_input_peers(
    mut peers: mpsc::Receiver<PeerInbound>,
    injection: InputInjectionHandle,
    lease: Duration,
    mut shutdown: broadcast::Receiver<()>,
) {
    let generations = Arc::new(Mutex::new(HashMap::<DeviceId, ControlConnectionId>::new()));
    let mut workers = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.recv() => {
                injection.request_release_all(ReleaseAllReason::SessionEnded);
                break;
            }
            peer = peers.recv() => {
                let Some(peer) = peer else {
                    injection.request_release_all(ReleaseAllReason::SessionEnded);
                    break;
                };
                let owner = AuthenticatedInputOwner {
                    peer_id: peer.auth.peer_id,
                    control_connection_id: peer.auth.control_connection_id,
                };
                let replaced = generations
                    .lock()
                    .unwrap()
                    .insert(owner.peer_id, owner.control_connection_id);
                if let Some(previous) = replaced {
                    if previous != owner.control_connection_id {
                        injection.request_release_through(
                            AuthenticatedInputOwner {
                                peer_id: owner.peer_id,
                                control_connection_id: previous,
                            },
                            SessionEpoch(u64::MAX),
                            ReleaseAllReason::OwnershipTransfer,
                        );
                    }
                }
                let worker_injection = injection.clone();
                let worker_generations = generations.clone();
                workers.spawn(async move {
                    run_peer_input_lanes(
                        peer,
                        owner,
                        worker_injection,
                        lease,
                        worker_generations,
                    )
                    .await;
                });
            }
            Some(_) = workers.join_next(), if !workers.is_empty() => {}
        }
    }
    workers.abort_all();
    while workers.join_next().await.is_some() {}
}

async fn run_peer_input_lanes(
    peer: PeerInbound,
    owner: AuthenticatedInputOwner,
    injection: InputInjectionHandle,
    lease: Duration,
    generations: Arc<Mutex<HashMap<DeviceId, ControlConnectionId>>>,
) {
    let PeerInbound {
        mut realtime_rx,
        mut reliable_input_rx,
        control_rx,
        telemetry_rx,
        bulk_rx,
        ..
    } = peer;
    // Task-10 still mirrors non-input traffic through NetworkEvent. Closing
    // these unused typed receivers prevents their bounded queues from
    // backpressuring the dedicated input lanes.
    drop(control_rx);
    drop(telemetry_rx);
    drop(bulk_rx);
    let mut active_epoch = None;
    let mut terminal_epoch = None;
    let mut realtime_open = true;
    let mut reliable_open = true;
    while realtime_open || reliable_open {
        let frame = tokio::select! {
            realtime = realtime_rx.recv(), if realtime_open => {
                match realtime {
                    Some(frame) => Some(PeerInputFrame::Realtime(frame)),
                    None => {
                        realtime_open = false;
                        None
                    }
                }
            },
            reliable = reliable_input_rx.recv(), if reliable_open => {
                match reliable {
                    Some(frame) => Some(PeerInputFrame::Reliable(frame)),
                    None => {
                        reliable_open = false;
                        None
                    }
                }
            },
        };
        let Some(frame) = frame else {
            continue;
        };
        if !is_current_generation(&generations, owner) {
            continue;
        }
        let epoch = frame.epoch();
        if active_epoch != Some(epoch) {
            if let Err(error) = injection.begin_session(owner, epoch, lease) {
                if !matches!(error, InjectionQueueFull::WrongOwnerOrEpoch)
                    || is_current_generation(&generations, owner)
                {
                    injection.request_release_through(
                        owner,
                        epoch,
                        ReleaseAllReason::BackendFailure,
                    );
                    terminal_epoch = Some(epoch);
                }
                break;
            }
            active_epoch = Some(epoch);
        }
        match frame {
            PeerInputFrame::Realtime(frame) => {
                let _ = injection.submit_realtime(owner, frame);
            }
            PeerInputFrame::Reliable(frame) => {
                let epoch = frame.session_epoch;
                if let Err(error) = injection.try_submit_reliable(owner, frame) {
                    if matches!(error, InjectionQueueFull::WrongOwnerOrEpoch)
                        && !is_current_generation(&generations, owner)
                    {
                        break;
                    }
                    injection.request_release_through(
                        owner,
                        epoch,
                        ReleaseAllReason::BackendFailure,
                    );
                    terminal_epoch = Some(epoch);
                    break;
                }
            }
        }
    }
    if remove_generation_if_current(&generations, owner) {
        if terminal_epoch.is_none() {
            injection.request_release_through(
                owner,
                SessionEpoch(u64::MAX),
                ReleaseAllReason::SessionEnded,
            );
        }
    }
}

enum PeerInputFrame {
    Realtime(RealtimeInputFrame),
    Reliable(ReliableInputFrame),
}

impl PeerInputFrame {
    fn epoch(&self) -> SessionEpoch {
        match self {
            Self::Realtime(frame) => frame.session_epoch,
            Self::Reliable(frame) => frame.session_epoch,
        }
    }
}

fn is_current_generation(
    generations: &Mutex<HashMap<DeviceId, ControlConnectionId>>,
    owner: AuthenticatedInputOwner,
) -> bool {
    generations
        .lock()
        .unwrap()
        .get(&owner.peer_id)
        .is_some_and(|generation| *generation == owner.control_connection_id)
}

fn remove_generation_if_current(
    generations: &Mutex<HashMap<DeviceId, ControlConnectionId>>,
    owner: AuthenticatedInputOwner,
) -> bool {
    let mut generations = generations.lock().unwrap();
    if generations.get(&owner.peer_id) != Some(&owner.control_connection_id) {
        return false;
    }
    generations.remove(&owner.peer_id);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_worker_compare_remove_preserves_generation_inserted_in_check_window() {
        let peer_id = DeviceId::new_v4();
        let old_owner = AuthenticatedInputOwner {
            peer_id,
            control_connection_id: ControlConnectionId::new(),
        };
        let new_generation = ControlConnectionId::new();
        let generations = Mutex::new(HashMap::from([(peer_id, old_owner.control_connection_id)]));

        assert!(is_current_generation(&generations, old_owner));
        generations.lock().unwrap().insert(peer_id, new_generation);

        assert!(!remove_generation_if_current(&generations, old_owner));
        assert_eq!(
            generations.lock().unwrap().get(&peer_id),
            Some(&new_generation)
        );
    }
}
