//! R-ShareMouse daemon service.
//!
//! Background service that handles input sharing and local IPC for status queries.

use anyhow::Result;
use rshare_core::{
    default_ipc_addr, read_json_line, write_json_line, BackendFailureReason, BackendHealth,
    BackendKind, BackendRuntimeState, CaptureSessionStateMachine, Config, ControlSessionState,
    DaemonDeviceSnapshot, DaemonRequest, DaemonResponse, DeviceId, Direction, LayoutGraph,
    LayoutNode, Message, ResolvedInputMode, ScreenInfo, ServiceStatusSnapshot,
};
use rshare_input::{
    BackendCandidate, BackendSelector, CaptureBackend, InjectBackend, InputEvent,
    PortableCaptureBackend, PortableInjectBackend, RDevInputListener,
};
use rshare_net::{DiscoveredDevice, NetworkEvent, NetworkManager, NetworkManagerConfig};

#[cfg(windows)]
use rshare_platform::firewall;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::time::{Duration, Instant};

#[derive(Clone)]
struct TrackedDevice {
    id: DeviceId,
    name: String,
    hostname: String,
    addresses: Vec<String>,
    connected: bool,
    last_seen_at: Instant,
}

struct DaemonState {
    status: ServiceStatusSnapshot,
    devices: HashMap<DeviceId, TrackedDevice>,
    // Layout and routing state
    layout: LayoutGraph,
    session: CaptureSessionStateMachine,
    // Backend state with separate capture/inject health
    backend_state: BackendRuntimeState,
}

impl DaemonState {
    fn new(status: ServiceStatusSnapshot) -> Self {
        let local_id = status.device_id;
        let mut layout = LayoutGraph::new(local_id);
        let local_screen = current_primary_screen_info();
        layout.add_node(LayoutNode::new(
            local_id,
            0,
            0,
            local_screen.width,
            local_screen.height,
        ));

        let mut backend_state = BackendRuntimeState::new();
        backend_state.available_backends = vec![BackendKind::Portable];

        Self {
            status,
            devices: HashMap::new(),
            layout,
            session: CaptureSessionStateMachine::new(),
            backend_state,
        }
    }

    fn upsert_discovered(&mut self, device: DiscoveredDevice) {
        let connected = self
            .devices
            .get(&device.id)
            .map(|existing| existing.connected)
            .unwrap_or(false);
        let screen_info = device.screen_info.clone();
        self.devices.insert(
            device.id,
            TrackedDevice {
                id: device.id,
                name: device.name,
                hostname: device.hostname,
                addresses: device
                    .addresses
                    .into_iter()
                    .map(|addr| addr.to_string())
                    .collect(),
                connected,
                last_seen_at: Instant::now(),
            },
        );
        self.layout.merge_discovered_peers_to_right_with_screens([(
            device.id,
            screen_info,
        )]);
    }

    fn remove_device(&mut self, id: &DeviceId) {
        self.devices.remove(id);
    }

    fn mark_connected(&mut self, id: &DeviceId, connected: bool) {
        if let Some(device) = self.devices.get_mut(id) {
            device.connected = connected;
            device.last_seen_at = Instant::now();
        } else if connected {
            self.devices.insert(
                *id,
                TrackedDevice {
                    id: *id,
                    name: format!("Device {}", short_device_id(*id)),
                    hostname: "unknown".to_string(),
                    addresses: Vec::new(),
                    connected: true,
                    last_seen_at: Instant::now(),
                },
            );
        }
    }

    fn status_snapshot(&self) -> ServiceStatusSnapshot {
        let mut snapshot = self.status.clone();
        snapshot.discovered_devices = self.devices.len();
        snapshot.connected_devices = self
            .devices
            .values()
            .filter(|device| device.connected)
            .count();

        // Populate backend status fields from BackendRuntimeState
        snapshot.input_mode = self.backend_state.selected_mode;
        snapshot.available_backends = Some(self.backend_state.available_backends.clone());
        snapshot.backend_health = Some(self.backend_state.aggregate_health.clone());
        snapshot.privilege_state = Some(self.backend_state.privilege_state);
        snapshot.last_backend_error = self.backend_state.last_error.clone();

        // Populate session state from CaptureSessionStateMachine
        snapshot.session_state = Some(self.session.state());
        snapshot.active_target = self.session.active_target();

        snapshot
    }

    fn device_snapshots(&self) -> Vec<DaemonDeviceSnapshot> {
        let mut devices: Vec<_> = self
            .devices
            .values()
            .map(|device| DaemonDeviceSnapshot {
                id: device.id,
                name: device.name.clone(),
                hostname: device.hostname.clone(),
                addresses: device.addresses.clone(),
                connected: device.connected,
                last_seen_secs: Some(device.last_seen_at.elapsed().as_secs()),
            })
            .collect();

        devices.sort_by(|left, right| left.name.cmp(&right.name));
        devices
    }

    fn reconcile_local_layout_geometry(&mut self) -> bool {
        let local_screen = current_primary_screen_info();
        self.layout.update_primary_display_geometry(
            self.status.device_id,
            local_screen.width,
            local_screen.height,
        )
    }

    fn update_backend_state(
        &mut self,
        mode: Option<ResolvedInputMode>,
        available: Vec<BackendKind>,
        capture_health: BackendHealth,
        inject_health: BackendHealth,
        error: Option<String>,
    ) {
        self.backend_state.selected_mode = mode;
        self.backend_state.available_backends = available;
        self.backend_state.capture_health = capture_health;
        self.backend_state.inject_health = inject_health;
        self.backend_state.last_error = error.clone();
        self.backend_state.update_aggregate_health();

        // Notify session machine if backend is degraded
        if matches!(
            self.backend_state.aggregate_health,
            BackendHealth::Degraded { .. }
        ) {
            self.session.on_backend_degraded();
        }
    }
}

fn short_device_id(id: DeviceId) -> String {
    id.to_string().chars().take(8).collect()
}

/// Discover available backends and select the best one
fn discover_and_select_backend() -> (
    Option<ResolvedInputMode>,
    Vec<BackendKind>,
    BackendHealth,
    Option<String>,
) {
    let mut candidates = vec![];

    let portable_capture_health = PortableCaptureBackend::new_for_test()
        .map(|backend| backend.health())
        .unwrap_or(BackendHealth::Degraded {
            reason: BackendFailureReason::InitializationFailed,
        });
    let portable_inject_health = PortableInjectBackend::new_for_test()
        .map(|backend| backend.health())
        .unwrap_or(BackendHealth::Degraded {
            reason: BackendFailureReason::InitializationFailed,
        });
    candidates.push(candidate_from_component_health(
        BackendKind::Portable,
        portable_capture_health,
        portable_inject_health,
    ));

    #[cfg(target_os = "windows")]
    {
        use rshare_input::backend::{WindowsNativeCaptureBackend, WindowsNativeInjectBackend};

        let capture_health = WindowsNativeCaptureBackend::new_for_test()
            .map(|backend| backend.health())
            .unwrap_or(BackendHealth::Degraded {
                reason: BackendFailureReason::InitializationFailed,
            });
        let inject_health = WindowsNativeInjectBackend::new_for_test()
            .map(|backend| backend.health())
            .unwrap_or(BackendHealth::Degraded {
                reason: BackendFailureReason::InitializationFailed,
            });

        candidates.push(candidate_from_component_health(
            BackendKind::WindowsNative,
            capture_health,
            inject_health,
        ));
    }

    #[cfg(target_os = "windows")]
    {
        use rshare_input::backend::{VirtualHidCaptureBackend, VirtualHidInjectBackend};

        let capture_health = VirtualHidCaptureBackend::new_for_test()
            .map(|backend| backend.health())
            .unwrap_or(BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            });
        let inject_health = VirtualHidInjectBackend::new_for_test()
            .map(|backend| backend.health())
            .unwrap_or(BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            });

        candidates.push(candidate_from_component_health(
            BackendKind::VirtualHid,
            capture_health,
            inject_health,
        ));
    }

    resolve_backend_selection(&candidates)
}

fn candidate_from_component_health(
    kind: BackendKind,
    capture_health: BackendHealth,
    inject_health: BackendHealth,
) -> BackendCandidate {
    let first_failure = match (&capture_health, &inject_health) {
        (BackendHealth::Degraded { reason }, _) => Some(reason.clone()),
        (_, BackendHealth::Degraded { reason }) => Some(reason.clone()),
        _ => None,
    };

    if first_failure.is_none() {
        BackendCandidate::healthy(kind)
    } else {
        BackendCandidate::unhealthy(
            kind,
            first_failure.unwrap_or(BackendFailureReason::Unavailable),
        )
    }
}

fn resolve_backend_selection(
    candidates: &[BackendCandidate],
) -> (
    Option<ResolvedInputMode>,
    Vec<BackendKind>,
    BackendHealth,
    Option<String>,
) {
    let selector = BackendSelector::new();
    let available_kinds: Vec<_> = candidates
        .iter()
        .filter(|c| c.healthy)
        .map(|c| c.kind)
        .collect();

    match selector.select(&candidates) {
        Some(result) => {
            let mode = result
                .to_input_mode()
                .unwrap_or(ResolvedInputMode::Portable);
            let health = if result.degraded {
                BackendHealth::Degraded {
                    reason: BackendFailureReason::Unavailable,
                }
            } else {
                BackendHealth::Healthy
            };
            (
                Some(mode),
                available_kinds,
                health,
                result.degradation_reason.clone(),
            )
        }
        None => (
            None,
            Vec::new(),
            BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
            Some("No input backend initialized successfully".to_string()),
        ),
    }
}

fn is_device_connected(state: &DaemonState, id: DeviceId) -> bool {
    state
        .devices
        .get(&id)
        .map(|device| device.connected)
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
struct InputRoutingState {
    remote_target: Option<DeviceId>,
    screen_width: u32,
    screen_height: u32,
    edge_threshold: u32,
}

impl InputRoutingState {
    fn new(screen_width: u32, screen_height: u32, edge_threshold: u32) -> Self {
        Self {
            remote_target: None,
            screen_width: screen_width.max(1),
            screen_height: screen_height.max(1),
            edge_threshold: edge_threshold.max(1),
        }
    }

    fn default_with_threshold(edge_threshold: u32) -> Self {
        Self::new(1920, 1080, edge_threshold)
    }

    #[cfg(test)]
    fn for_test(screen_width: u32, screen_height: u32, edge_threshold: u32) -> Self {
        Self::new(screen_width, screen_height, edge_threshold)
    }

    #[cfg(test)]
    fn remote_target(&self) -> Option<DeviceId> {
        self.remote_target
    }

    fn clear_remote_target(&mut self) {
        self.remote_target = None;
    }

    fn set_remote_target(&mut self, target: DeviceId) {
        self.remote_target = Some(target);
    }

    fn hit_edges(&self, event: &InputEvent) -> Vec<Direction> {
        let InputEvent::MouseMove { x, y } = event else {
            return Vec::new();
        };

        let mut edges = Vec::with_capacity(4);
        let right_edge_start = self.screen_width.saturating_sub(self.edge_threshold) as i32;
        let bottom_edge_start = self.screen_height.saturating_sub(self.edge_threshold) as i32;

        if *x <= self.edge_threshold as i32 && self.is_vertical_screen_coordinate(*y) {
            edges.push(Direction::Left);
        }
        if *x >= right_edge_start && self.is_vertical_screen_coordinate(*y) {
            edges.push(Direction::Right);
        }
        if *y <= self.edge_threshold as i32 && self.is_horizontal_screen_coordinate(*x) {
            edges.push(Direction::Top);
        }
        if *y >= bottom_edge_start && self.is_horizontal_screen_coordinate(*x) {
            edges.push(Direction::Bottom);
        }

        edges
    }

    fn is_vertical_screen_coordinate(&self, y: i32) -> bool {
        y >= 0 && y < self.screen_height as i32
    }

    fn is_horizontal_screen_coordinate(&self, x: i32) -> bool {
        x >= 0 && x < self.screen_width as i32
    }
}

fn input_event_to_raw_event(
    event: rshare_input::InputEvent,
) -> Option<rshare_core::engine::RawInputEvent> {
    match event {
        rshare_input::InputEvent::MouseMove { x, y } => {
            Some(rshare_core::engine::RawInputEvent::MouseMove { x, y })
        }
        rshare_input::InputEvent::MouseButton { button, state } => {
            Some(rshare_core::engine::RawInputEvent::MouseButton {
                button: button.to_code(),
                pressed: state.is_pressed(),
            })
        }
        rshare_input::InputEvent::MouseWheel { delta_x, delta_y } => {
            Some(rshare_core::engine::RawInputEvent::MouseWheel { delta_x, delta_y })
        }
        rshare_input::InputEvent::Key { keycode, state } => {
            Some(rshare_core::engine::RawInputEvent::Key {
                keycode: keycode.to_raw(),
                pressed: state.is_pressed(),
            })
        }
        rshare_input::InputEvent::KeyExtended { keycode, state, .. } => {
            Some(rshare_core::engine::RawInputEvent::Key {
                keycode: keycode.to_raw(),
                pressed: state.is_pressed(),
            })
        }
    }
}

fn messages_for_input_event(
    state: &mut DaemonState,
    routing: &mut InputRoutingState,
    forwarder: &mut rshare_core::engine::ForwardingEngine,
    event: InputEvent,
) -> Vec<Message> {
    // Get connected peers set
    let connected_peers: std::collections::HashSet<_> = state
        .devices
        .values()
        .filter(|device| device.connected)
        .map(|device| device.id)
        .collect();
    let local_id = state.status.device_id;
    let edge_hits = routing.hit_edges(&event);

    match state.session.state() {
        ControlSessionState::RemoteActive {
            target,
            entered_via,
        } => {
            routing.set_remote_target(target);
            if !is_device_connected(state, target) {
                state.session.on_target_disconnect(target);
                routing.clear_remote_target();
                forwarder.clear_target();
                return Vec::new();
            }

            let return_edge = entered_via.opposite();
            if edge_hits.contains(&return_edge) {
                let _ = state.session.on_return_edge_hit(return_edge);
                routing.clear_remote_target();
                forwarder.clear_target();
                return Vec::new();
            }
        }
        ControlSessionState::Suspended { .. } => {
            routing.clear_remote_target();
            forwarder.clear_target();
            return Vec::new();
        }
        _ => {
            routing.clear_remote_target();
            if let Some((edge, target)) = edge_hits.iter().find_map(|edge| {
                state
                    .layout
                    .resolve_target(local_id, *edge, &connected_peers)
                    .map(|target| (*edge, target))
            }) {
                if state.session.on_edge_hit(edge, Some(target)).is_ok() {
                    routing.set_remote_target(target);
                } else {
                    forwarder.clear_target();
                    return Vec::new();
                }
            } else {
                forwarder.clear_target();
                return Vec::new();
            }
        }
    }

    let target = if let Some(remote_target) = state.session.active_target() {
        if !is_device_connected(state, remote_target) {
            state.session.on_target_disconnect(remote_target);
            routing.clear_remote_target();
            forwarder.clear_target();
            return Vec::new();
        }
        routing.set_remote_target(remote_target);
        remote_target
    } else {
        routing.clear_remote_target();
        forwarder.clear_target();
        return Vec::new();
    };

    let activated_on_this_event = Some(target) != forwarder.target();
    forwarder.set_target(target);
    let Some(raw_event) = input_event_to_raw_event(event) else {
        return Vec::new();
    };

    let mut messages = forwarder.process_event(raw_event);
    if activated_on_this_event && messages.is_empty() {
        messages = forwarder.flush_batch();
    }
    messages
}

fn message_to_input_event(message: Message) -> Option<InputEvent> {
    match message {
        Message::MouseMove { x, y } => Some(InputEvent::mouse_move(x, y)),
        Message::MouseButton { button, state } => Some(InputEvent::mouse_button(
            rshare_input::MouseButton::from_code(button.to_code()),
            input_button_state(state),
        )),
        Message::MouseWheel { delta_x, delta_y } => Some(InputEvent::mouse_wheel(delta_x, delta_y)),
        Message::Key { keycode, state } => Some(InputEvent::key(
            rshare_input::KeyCode::Raw(keycode),
            input_key_state(state),
        )),
        Message::KeyExtended {
            keycode,
            state,
            shift,
            ctrl,
            alt,
            meta,
        } => Some(InputEvent::key_extended(
            rshare_input::KeyCode::Raw(keycode),
            input_key_state(state),
            shift,
            ctrl,
            alt,
            meta,
        )),
        _ => None,
    }
}

fn input_button_state(state: rshare_core::ButtonState) -> rshare_input::ButtonState {
    match state {
        rshare_core::ButtonState::Pressed => rshare_input::ButtonState::Pressed,
        rshare_core::ButtonState::Released => rshare_input::ButtonState::Released,
    }
}

fn input_key_state(state: rshare_core::KeyState) -> rshare_input::ButtonState {
    match state {
        rshare_core::KeyState::Pressed => rshare_input::ButtonState::Pressed,
        rshare_core::KeyState::Released => rshare_input::ButtonState::Released,
    }
}

fn create_inject_backend(mode: Option<ResolvedInputMode>) -> Result<Box<dyn InjectBackend>> {
    #[cfg(target_os = "windows")]
    if matches!(mode, Some(ResolvedInputMode::WindowsNative)) {
        use rshare_input::backend::WindowsNativeInjectBackend;
        return Ok(Box::new(WindowsNativeInjectBackend::new()?));
    }

    Ok(Box::new(PortableInjectBackend::new()?))
}

async fn inject_remote_message(
    inject_backend: &Arc<Mutex<Box<dyn InjectBackend>>>,
    from: DeviceId,
    message: Message,
) {
    let Some(event) = message_to_input_event(message) else {
        return;
    };

    let result = {
        let mut backend = inject_backend.lock().await;
        backend.inject(event)
    };

    if let Err(error) = result {
        tracing::warn!("Failed to inject input from {}: {}", from, error);
    }
}

async fn send_forwarded_messages(
    network_manager: &Arc<Mutex<NetworkManager>>,
    target: DeviceId,
    messages: Vec<Message>,
) {
    for message in messages {
        let result = {
            let mut manager = network_manager.lock().await;
            manager.send_to(&target, message).await
        };

        if let Err(error) = result {
            tracing::warn!("Failed to forward input to {}: {}", target, error);
        }
    }
}

async fn run_input_forwarding_loop(
    mut input_rx: tokio::sync::mpsc::UnboundedReceiver<InputEvent>,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    mut shutdown_rx: broadcast::Receiver<()>,
    edge_threshold: u32,
) -> Result<()> {
    let mut forwarder = rshare_core::engine::ForwardingEngine::new();
    let mut routing = InputRoutingState::default_with_threshold(edge_threshold);
    let mut flush_interval = tokio::time::interval(Duration::from_millis(8));

    loop {
        tokio::select! {
            event = input_rx.recv() => {
                let Some(event) = event else {
                    break;
                };

                let (target, messages) = {
                    let mut state = state.write().await;
                    let messages =
                        messages_for_input_event(&mut state, &mut routing, &mut forwarder, event);
                    let target = state.session.active_target();
                    (target, messages)
                };

                if let Some(target) = target {
                    send_forwarded_messages(&network_manager, target, messages).await;
                }
            }
            _ = flush_interval.tick() => {
                if !forwarder.should_flush_batch() {
                    continue;
                }

                let target = {
                    let state = state.read().await;
                    state
                        .session
                        .active_target()
                        .filter(|target| is_device_connected(&state, *target))
                };

                let Some(target) = target else {
                    routing.clear_remote_target();
                    forwarder.clear_target();
                    continue;
                };

                forwarder.set_target(target);
                let messages = forwarder.flush_batch();
                send_forwarded_messages(&network_manager, target, messages).await;
            }
            _ = shutdown_rx.recv() => {
                break;
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("R-ShareMouse daemon starting...");

    // Configure firewall on Windows to allow discovery and service ports
    #[cfg(windows)]
    {
        match firewall::configure_firewall() {
            Ok(result) => {
                if result.is_success() {
                    tracing::info!("Firewall configured successfully for R-ShareMouse");
                } else {
                    tracing::warn!("Firewall configuration incomplete: {:?}", result);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to configure firewall: {}", e);
                tracing::warn!("Device discovery may not work. Please run as administrator or add firewall rules manually.");
            }
        }
    }

    let config = load_config_with_env_overrides()?;

    let hostname = hostname::get()
        .unwrap_or_else(|_| "unknown".into())
        .to_string_lossy()
        .to_string();
    let device_id = rshare_core::service::load_or_create_local_device_id()?;
    let device_name = format!("{}-R-ShareMouse", hostname);
    let bind_address = format!("{}:{}", config.network.bind_address, config.network.port);
    let layout_path = rshare_core::service::layout_graph_path()?;

    let mut network_manager = NetworkManager::new(device_id, device_name.clone(), hostname.clone())
        .with_config(NetworkManagerConfig {
            bind_address: bind_address.clone(),
            ..Default::default()
        });

    let mut events = network_manager.events();
    let network_manager = Arc::new(Mutex::new(network_manager));
    {
        let mut manager = network_manager.lock().await;
        manager.start().await?;
    }

    let mut service_manager = rshare_core::service::ServiceManager::new()?;
    let _service_handle = service_manager.start().await?;
    let pid = std::process::id();

    // Discover and select backend
    let (input_mode, available_backends, backend_health, backend_error) =
        discover_and_select_backend();

    tracing::info!(
        "Backend selected: {:?} (available: {:?})",
        input_mode,
        available_backends
    );

    let mut daemon_state = DaemonState::new(ServiceStatusSnapshot::new(
        device_id,
        device_name.clone(),
        hostname.clone(),
        bind_address.clone(),
        27432,
        pid,
    ));
    daemon_state.layout = load_layout_from_path(device_id, &layout_path)?;
    let should_save_runtime_layout = daemon_state.reconcile_local_layout_geometry();
    if should_save_runtime_layout {
        save_layout_to_path(&daemon_state.layout, &layout_path)?;
    }
    let state = Arc::new(RwLock::new(daemon_state));

    // Initialize backend state
    {
        let mut s = state.write().await;
        s.update_backend_state(
            input_mode,
            available_backends,
            backend_health.clone(), // capture health
            backend_health,         // inject health
            backend_error,
        );
    }
    let inject_backend = Arc::new(Mutex::new(create_inject_backend(input_mode)?));

    let ipc_listener = TcpListener::bind(default_ipc_addr()).await?;
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(8);

    // Use RDevInputListener for cross-platform input capture
    let mut input_listener = RDevInputListener::new();
    let input_rx = input_listener.receiver();
    input_listener.start().await?;

    tracing::info!("Daemon started as device {} ({})", device_name, device_id);
    tracing::info!("Listening for connections on {}", bind_address);
    tracing::info!("Device discovery on port 27432");
    tracing::info!("Local IPC listening on {}", default_ipc_addr());

    let layout_path = Arc::new(layout_path);

    let ipc_task = tokio::spawn(run_ipc_server(
        ipc_listener,
        state.clone(),
        network_manager.clone(),
        layout_path.clone(),
        shutdown_tx.clone(),
    ));

    let input_forwarding_task = tokio::spawn(run_input_forwarding_loop(
        input_rx,
        state.clone(),
        network_manager.clone(),
        shutdown_tx.subscribe(),
        config.edge_threshold(),
    ));

    let event_task = {
        let state = state.clone();
        let inject_backend = inject_backend.clone();
        let layout_path = layout_path.clone();
        tokio::spawn(async move {
            tracing::info!("Event task: starting to wait for events");
            while let Some(event) = events.recv().await {
                match event {
                    NetworkEvent::DeviceFound(device) => {
                        let layout_to_save = {
                            let mut state = state.write().await;
                            state.upsert_discovered(device);
                            state.layout.clone()
                        };
                        if let Err(err) = save_layout_to_path(&layout_to_save, layout_path.as_ref())
                        {
                            tracing::warn!("Failed to persist auto-updated layout: {}", err);
                        }
                    }
                    NetworkEvent::DeviceConnected(id) => {
                        let mut state = state.write().await;
                        state.mark_connected(&id, true)
                    }
                    NetworkEvent::DeviceDisconnected(id) => {
                        let mut state = state.write().await;
                        // Notify session state machine of target disconnection
                        state.session.on_target_disconnect(id);
                        state.remove_device(&id);
                    }
                    NetworkEvent::MessageReceived { from, message } => {
                        inject_remote_message(&inject_backend, from, message).await;
                    }
                    NetworkEvent::ConnectionError { device_id, error } => {
                        tracing::warn!("Connection error to {}: {}", device_id, error);
                        let mut state = state.write().await;
                        state.mark_connected(&device_id, false);
                    }
                }
            }
            tracing::debug!("Event task: events channel closed");
        })
    };

    tracing::info!("Entering tokio::select! loop");
    tokio::select! {
        result = signal::ctrl_c() => {
            match result {
                Ok(()) => tracing::info!("Shutdown signal received"),
                Err(e) => tracing::warn!("Ctrl-C handler error: {}", e),
            }
        }
        _ = shutdown_rx.recv() => {
            tracing::info!("Shutdown requested over IPC");
        }
        result = ipc_task => {
            tracing::info!("IPC task completed");
            result??;
        }
        result = event_task => {
            tracing::info!("Event task completed");
            result?;
        }
        result = input_forwarding_task => {
            tracing::info!("Input forwarding task completed");
            result??;
        }
    }

    tracing::info!("tokio::select! exited, cleaning up");
    input_listener.stop().await?;
    network_manager.lock().await.stop().await?;

    tracing::info!("R-ShareMouse daemon stopped");
    Ok(())
}

async fn run_ipc_server(
    listener: TcpListener,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    layout_path: Arc<PathBuf>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let network_manager = network_manager.clone();
        let layout_path = layout_path.clone();
        let shutdown_tx = shutdown_tx.clone();

        tokio::spawn(async move {
            if let Err(err) =
                handle_ipc_client(stream, state, network_manager, layout_path, shutdown_tx).await
            {
                tracing::debug!("IPC client error: {}", err);
            }
        });
    }
}

async fn handle_ipc_client(
    mut stream: TcpStream,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    layout_path: Arc<PathBuf>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    let request: DaemonRequest = read_json_line(&mut stream).await?;

    let response = match request {
        DaemonRequest::Status => {
            let state = state.read().await;
            DaemonResponse::Status(state.status_snapshot())
        }
        DaemonRequest::Devices => {
            let state = state.read().await;
            DaemonResponse::Devices(state.device_snapshots())
        }
        DaemonRequest::Connect { device_id } => {
            let address = {
                let state = state.read().await;
                state
                    .devices
                    .get(&device_id)
                    .and_then(|device| device.addresses.first().cloned())
            };

            match address {
                Some(address) => {
                    let result = {
                        let mut manager = network_manager.lock().await;
                        manager.connect_to(device_id, &address).await
                    };

                    match result {
                        Ok(_) => {
                            state.write().await.mark_connected(&device_id, true);
                            DaemonResponse::Ack
                        }
                        Err(err) => DaemonResponse::Error(err.to_string()),
                    }
                }
                None => DaemonResponse::Error(format!("No known address for device {}", device_id)),
            }
        }
        DaemonRequest::Disconnect { device_id } => {
            let result = {
                let mut manager = network_manager.lock().await;
                manager.disconnect_from(&device_id).await
            };

            match result {
                Ok(_) => {
                    state.write().await.mark_connected(&device_id, false);
                    DaemonResponse::Ack
                }
                Err(err) => DaemonResponse::Error(err.to_string()),
            }
        }
        DaemonRequest::GetLayout => {
            let state = state.read().await;
            DaemonResponse::Layout(state.layout.clone())
        }
        DaemonRequest::SetLayout { layout } => {
            let mut state = state.write().await;
            let mut canonical_layout = layout;
            canonical_layout.canonicalize_local_device(state.status.device_id);

            match save_layout_to_path(&canonical_layout, layout_path.as_ref()) {
                Ok(()) => {
                    state.layout = canonical_layout;
                    DaemonResponse::Ack
                }
                Err(err) => DaemonResponse::Error(err.to_string()),
            }
        }
        DaemonRequest::Shutdown => {
            let _ = shutdown_tx.send(());
            DaemonResponse::Ack
        }
    };

    write_json_line(&mut stream, &response).await
}

#[cfg(test)]
fn apply_layout_update(state: &mut DaemonState, mut layout: LayoutGraph) {
    layout.canonicalize_local_device(state.status.device_id);
    state.layout = layout;
}

fn current_primary_screen_info() -> ScreenInfo {
    #[cfg(windows)]
    {
        let screen = rshare_platform::WindowsInputListener::get_screen_info();
        return ScreenInfo::new(0, 0, screen.width, screen.height);
    }

    #[cfg(target_os = "macos")]
    {
        let screen = rshare_platform::get_screen_info();
        return ScreenInfo::new(0, 0, screen.width, screen.height);
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        ScreenInfo::primary()
    }
}

fn default_local_only_layout(local_device: DeviceId) -> LayoutGraph {
    let local_screen = current_primary_screen_info();
    let mut layout = LayoutGraph::new(local_device);
    layout.add_node(LayoutNode::new(
        local_device,
        0,
        0,
        local_screen.width,
        local_screen.height,
    ));
    layout
}

fn invalid_layout_backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.invalid")
}

fn preserve_invalid_layout_file(path: &Path) {
    let backup_path = invalid_layout_backup_path(path);
    if backup_path.exists() {
        let _ = fs::remove_file(&backup_path);
    }

    if let Err(error) = fs::rename(path, &backup_path) {
        tracing::warn!(
            "Failed to preserve invalid layout file {} as {}: {}",
            path.display(),
            backup_path.display(),
            error
        );
    }

}

fn load_layout_from_path(local_device: DeviceId, path: impl AsRef<Path>) -> Result<LayoutGraph> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(default_local_only_layout(local_device));
    }

    let loaded = fs::read_to_string(path)
        .map_err(anyhow::Error::from)
        .and_then(|content| {
            serde_json::from_str::<LayoutGraph>(&content).map_err(anyhow::Error::from)
        });

    match loaded {
        Ok(mut layout) => {
            layout.canonicalize_local_device(local_device);
            Ok(layout)
        }
        Err(error) => {
            tracing::warn!(
                "Failed to load persisted layout from {}: {}. Falling back to local-only layout.",
                path.display(),
                error
            );
            preserve_invalid_layout_file(path);
            Ok(default_local_only_layout(local_device))
        }
    }
}

fn save_layout_to_path(layout: &LayoutGraph, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let encoded = serde_json::to_string_pretty(layout)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let tmp_path = path.with_extension(format!("json.tmp-{}", nanos));
    fs::write(&tmp_path, encoded)?;
    if let Err(error) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error.into());
    }
    Ok(())
}

fn load_config_with_env_overrides() -> Result<Config> {
    let mut config = Config::load()?;

    if let Ok(bind) = std::env::var("RSHARE_BIND") {
        config.network.bind_address = bind;
    }

    if let Ok(port) = std::env::var("RSHARE_PORT") {
        config.network.port = port.parse()?;
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn backend_with_missing_capture_is_not_reported_as_available() {
        let candidates = vec![candidate_from_component_health(
            BackendKind::Portable,
            BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
            BackendHealth::Healthy,
        )];

        let (mode, available, health, error) = resolve_backend_selection(&candidates);

        assert!(mode.is_none());
        assert!(available.is_empty());
        assert!(matches!(
            health,
            BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable
            }
        ));
        assert!(error.unwrap().contains("No input backend"));
    }

    #[test]
    fn discovered_device_updates_in_memory_layout_without_desktop_roundtrip() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "0.0.0.0:27431".to_string(),
            27432,
            42,
        ));

        state.upsert_discovered(DiscoveredDevice {
            id: remote_id,
            name: "remote".to_string(),
            hostname: "remote-host".to_string(),
            addresses: vec!["192.168.1.241:27431".parse().unwrap()],
            screen_info: Some(ScreenInfo::new(0, 0, 2560, 1440)),
            last_seen: Instant::now(),
        });

        let remote_node = state.layout.get_node(remote_id);
        assert!(
            remote_node.is_some(),
            "daemon discovery should populate layout immediately"
        );
        assert!(state.layout.links.iter().any(|link| {
            link.from_device == local_id
                && link.to_device == remote_id
                && link.from_edge == Direction::Right
        }));
    }

    #[test]
    fn healthy_candidates_remain_visible_after_selection() {
        let candidates = vec![
            candidate_from_component_health(
                BackendKind::Portable,
                BackendHealth::Healthy,
                BackendHealth::Healthy,
            ),
            candidate_from_component_health(
                BackendKind::WindowsNative,
                BackendHealth::Degraded {
                    reason: BackendFailureReason::Unavailable,
                },
                BackendHealth::Healthy,
            ),
        ];

        let (mode, available, health, error) = resolve_backend_selection(&candidates);

        assert_eq!(mode, Some(ResolvedInputMode::Portable));
        assert_eq!(available, vec![BackendKind::Portable]);
        assert!(matches!(
            health,
            BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable
            }
        ));
        assert!(error.unwrap().contains("using Portable"));
    }

    #[test]
    fn input_event_maps_to_forwarding_raw_event() {
        let raw = input_event_to_raw_event(rshare_input::InputEvent::mouse_button(
            rshare_input::MouseButton::Back,
            rshare_input::ButtonState::Pressed,
        ))
        .unwrap();

        match raw {
            rshare_core::engine::RawInputEvent::MouseButton { button, pressed } => {
                assert_eq!(button, 4);
                assert!(pressed);
            }
            _ => panic!("Wrong raw input event"),
        }
    }

    #[test]
    fn mark_connected_tracks_unknown_inbound_device() {
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        state.mark_connected(&remote_id, true);

        let device = state.devices.get(&remote_id).unwrap();
        assert_eq!(device.id, remote_id);
        assert!(device.connected);
        assert_eq!(device.hostname, "unknown");
    }

    #[test]
    fn input_event_forwarding_requires_connected_target() {
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key(
                rshare_input::KeyCode::Raw(0x20),
                rshare_input::ButtonState::Pressed,
            ),
        );

        assert!(messages.is_empty());
    }

    #[test]
    fn input_event_forwarding_stays_local_until_edge_activation() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                last_seen_at: Instant::now(),
            },
        );
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key(
                rshare_input::KeyCode::Raw(0x20),
                rshare_input::ButtonState::Pressed,
            ),
        );

        assert!(messages.is_empty());
        assert_eq!(forwarder.target(), None);
    }

    #[test]
    fn right_edge_activates_remote_forwarding() {
        use rshare_core::{Direction, LayoutLink};
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                last_seen_at: Instant::now(),
            },
        );
        // Add layout link for routing
        state
            .layout
            .add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
        );

        assert_eq!(routing.remote_target(), Some(remote_id));
        assert_eq!(forwarder.target(), Some(remote_id));
        assert!(!messages.is_empty());
        assert!(matches!(
            state.status_snapshot().session_state,
            Some(rshare_core::ControlSessionState::RemoteActive {
                target,
                entered_via: Direction::Right
            }) if target == remote_id
        ));
    }

    #[test]
    fn left_edge_releases_remote_forwarding() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                last_seen_at: Instant::now(),
            },
        );
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let _ = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
        );
        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(0, 500),
        );

        assert!(messages.is_empty());
        assert_eq!(routing.remote_target(), None);
        assert_eq!(forwarder.target(), None);
    }

    #[test]
    fn left_edge_layout_can_activate_remote_forwarding() {
        use rshare_core::{Direction, LayoutLink};

        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                last_seen_at: Instant::now(),
            },
        );
        state
            .layout
            .add_node(LayoutNode::new(remote_id, -1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Left,
            to_device: remote_id,
            to_edge: Direction::Right,
        });

        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(0, 500),
        );

        assert_eq!(routing.remote_target(), Some(remote_id));
        assert_eq!(forwarder.target(), Some(remote_id));
        assert!(!messages.is_empty());
    }

    #[test]
    fn input_event_forwarding_targets_first_connected_device() {
        use rshare_core::{Direction, LayoutLink};
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                last_seen_at: Instant::now(),
            },
        );
        // Add layout link for routing
        state
            .layout
            .add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let _ = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
        );
        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key(
                rshare_input::KeyCode::Raw(0x20),
                rshare_input::ButtonState::Pressed,
            ),
        );

        assert_eq!(forwarder.target(), Some(remote_id));
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            messages[0],
            rshare_core::Message::Key {
                keycode: 0x20,
                state: rshare_core::KeyState::Pressed
            }
        ));
    }

    #[test]
    fn remote_input_message_maps_to_injectable_input_event() {
        let event = message_to_input_event(rshare_core::Message::MouseButton {
            button: rshare_core::MouseButton::Forward,
            state: rshare_core::ButtonState::Released,
        })
        .unwrap();

        match event {
            rshare_input::InputEvent::MouseButton { button, state } => {
                assert_eq!(button, rshare_input::MouseButton::Forward);
                assert_eq!(state, rshare_input::ButtonState::Released);
            }
            _ => panic!("Wrong input event"),
        }
    }

    #[test]
    fn non_input_message_is_not_injected() {
        let event = message_to_input_event(rshare_core::Message::Heartbeat {
            sequence: 1,
            timestamp: 2,
        });

        assert!(event.is_none());
    }

    // Alpha-2 layout-driven routing tests
    // These tests verify that the daemon uses LayoutGraph instead of first_connected_device

    #[test]
    fn daemon_does_not_forward_to_first_connected_without_layout_link() {
        use rshare_core::{Direction, LayoutGraph};
        use std::collections::HashSet;

        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "z-remote-last".to_string(), // Name sorted last, but should not be used
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                last_seen_at: Instant::now(),
            },
        );

        // Create a layout with local device only (no link to remote)
        let layout = LayoutGraph::new(local_id);
        let connected_peers: HashSet<DeviceId> = [remote_id].into_iter().collect();

        // Edge hit should not find target without layout link
        let target = layout.resolve_target(local_id, Direction::Right, &connected_peers);
        assert_eq!(target, None, "Should not forward without layout link");
    }

    #[test]
    fn daemon_routes_through_layout_graph_not_first_connected() {
        use rshare_core::{Direction, LayoutGraph, LayoutLink, LayoutNode};
        use std::collections::HashSet;

        let local_id = DeviceId::new_v4();
        let remote_a = DeviceId::new_v4();
        let remote_b = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        // Add two connected devices
        state.devices.insert(
            remote_a,
            TrackedDevice {
                id: remote_a,
                name: "a-device".to_string(), // Would be first in name sort
                hostname: "a-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                last_seen_at: Instant::now(),
            },
        );
        state.devices.insert(
            remote_b,
            TrackedDevice {
                id: remote_b,
                name: "b-device".to_string(),
                hostname: "b-host".to_string(),
                addresses: vec!["127.0.0.1:27432".to_string()],
                connected: true,
                last_seen_at: Instant::now(),
            },
        );

        // Create layout that links local->remote_b (not remote_a)
        let mut layout = LayoutGraph::new(local_id);
        layout.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));
        layout.add_node(LayoutNode::new(remote_a, -1920, 0, 1920, 1080));
        layout.add_node(LayoutNode::new(remote_b, 1920, 0, 1920, 1080));
        layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_b,
            to_edge: Direction::Left,
        });

        let connected_peers: HashSet<DeviceId> = [remote_a, remote_b].into_iter().collect();

        // Should route to remote_b based on layout, not remote_a (first by name)
        let target = layout.resolve_target(local_id, Direction::Right, &connected_peers);
        assert_eq!(
            target,
            Some(remote_b),
            "Should route to layout-linked device"
        );
        assert_ne!(
            target,
            Some(remote_a),
            "Should not route to first-connected device"
        );
    }

    #[test]
    fn daemon_disconnect_clears_remote_active_session() {
        use rshare_core::{
            CaptureSessionStateMachine, ControlSessionState, Direction, SuspendReason,
        };

        let remote_id = DeviceId::new_v4();
        let mut machine = CaptureSessionStateMachine::new();

        // Enter remote mode
        machine
            .on_edge_hit(Direction::Right, Some(remote_id))
            .unwrap();
        assert!(matches!(
            machine.state(),
            ControlSessionState::RemoteActive { .. }
        ));

        // Disconnect should clear session
        machine.on_target_disconnect(remote_id);
        assert!(matches!(
            machine.state(),
            ControlSessionState::Suspended {
                reason: SuspendReason::TargetUnavailable
            }
        ));
    }

    #[test]
    fn daemon_backend_degradation_prevents_forwarding() {
        use rshare_core::{
            CaptureSessionStateMachine, ControlSessionState, Direction, SuspendReason,
        };

        let remote_id = DeviceId::new_v4();
        let mut machine = CaptureSessionStateMachine::new();

        // Backend degrades
        machine.on_backend_degraded();

        // Edge hit should not work
        let result = machine.on_edge_hit(Direction::Right, Some(remote_id));
        assert!(result.is_err());

        // State should be suspended
        assert!(matches!(
            machine.state(),
            ControlSessionState::Suspended {
                reason: SuspendReason::BackendDegraded
            }
        ));
    }

    #[test]
    fn daemon_session_state_exposed_in_snapshot() {
        use rshare_core::ControlSessionState;

        let local_id = DeviceId::new_v4();
        let state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        // Session state should be accessible
        let snapshot = state.status_snapshot();
        assert_eq!(
            snapshot.session_state,
            Some(ControlSessionState::LocalReady)
        );
        assert_eq!(snapshot.active_target, None);
    }

    #[test]
    fn daemon_disconnect_clears_active_session_in_snapshot() {
        use rshare_core::{ControlSessionState, Direction, SuspendReason};

        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        // Enter remote mode
        state
            .session
            .on_edge_hit(Direction::Right, Some(remote_id))
            .unwrap();
        let snapshot = state.status_snapshot();
        assert!(matches!(
            snapshot.session_state,
            Some(ControlSessionState::RemoteActive { .. })
        ));

        // Disconnect should update session
        state.session.on_target_disconnect(remote_id);
        let snapshot = state.status_snapshot();
        assert!(matches!(
            snapshot.session_state,
            Some(ControlSessionState::Suspended {
                reason: SuspendReason::TargetUnavailable
            })
        ));
    }

    #[test]
    fn daemon_reconnect_after_session_reset() {
        use rshare_core::ControlSessionState;

        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        // Enter and disconnect
        state
            .session
            .on_edge_hit(Direction::Right, Some(remote_id))
            .unwrap();
        state.session.on_target_disconnect(remote_id);

        // Reset session
        state.session.reset();
        let snapshot = state.status_snapshot();
        assert_eq!(
            snapshot.session_state,
            Some(ControlSessionState::LocalReady)
        );

        // Can enter remote mode again
        state
            .session
            .on_edge_hit(Direction::Right, Some(remote_id))
            .unwrap();
        let snapshot = state.status_snapshot();
        assert!(matches!(
            snapshot.session_state,
            Some(ControlSessionState::RemoteActive { .. })
        ));
    }

    #[test]
    fn stale_layout_from_previous_daemon_run_must_be_canonicalized_to_current_local_device() {
        use rshare_core::{Direction, LayoutGraph, LayoutLink};

        let current_local = DeviceId::new_v4();
        let stale_local = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            current_local,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        let mut layout = LayoutGraph::new(stale_local);
        layout.add_node(LayoutNode::new(stale_local, 0, 0, 1920, 1080));
        layout.add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        layout.add_link(LayoutLink::new(
            stale_local,
            Direction::Right,
            remote_id,
            Direction::Left,
        ));

        apply_layout_update(&mut state, layout);

        assert_eq!(state.layout.local_device, current_local);
        assert!(state
            .layout
            .links
            .iter()
            .any(|link| link.from_device == current_local && link.to_device == remote_id));
    }

    fn test_status(local_id: DeviceId) -> ServiceStatusSnapshot {
        ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        )
    }

    fn temp_state_dir() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("rshare-daemon-layout-test-{}", DeviceId::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn remembered_layout(local_id: DeviceId, remote_id: DeviceId) -> LayoutGraph {
        use rshare_core::{Direction, LayoutLink};

        let mut layout = LayoutGraph::new(local_id);
        layout.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));
        layout.add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        layout.add_link(LayoutLink::new(
            local_id,
            Direction::Right,
            remote_id,
            Direction::Left,
        ));
        layout
    }

    #[test]
    fn daemon_loads_saved_layout_from_state_dir() {
        let state_dir = temp_state_dir();
        let layout_path = rshare_core::service::layout_graph_path_in(&state_dir);
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let expected = remembered_layout(local_id, remote_id);

        save_layout_to_path(&expected, &layout_path).unwrap();

        let loaded = load_layout_from_path(local_id, &layout_path).unwrap();

        assert_eq!(loaded, expected);
        let _ = std::fs::remove_dir_all(state_dir);
    }

    #[test]
    fn daemon_falls_back_to_local_only_layout_when_no_saved_layout_exists() {
        let state_dir = temp_state_dir();
        let layout_path = rshare_core::service::layout_graph_path_in(&state_dir);
        let local_id = DeviceId::new_v4();

        let loaded = load_layout_from_path(local_id, &layout_path).unwrap();

        let state = DaemonState::new(test_status(local_id));
        assert_eq!(loaded, state.layout);
        let _ = std::fs::remove_dir_all(state_dir);
    }

    #[test]
    fn daemon_saved_layout_survives_restart_semantics() {
        let state_dir = temp_state_dir();
        let layout_path = rshare_core::service::layout_graph_path_in(&state_dir);
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let expected = remembered_layout(local_id, remote_id);

        let mut first_start = DaemonState::new(test_status(local_id));
        apply_layout_update(&mut first_start, expected.clone());
        save_layout_to_path(&first_start.layout, &layout_path).unwrap();

        let restarted = load_layout_from_path(local_id, &layout_path).unwrap();

        assert_eq!(restarted, first_start.layout);
        let _ = std::fs::remove_dir_all(state_dir);
    }

    #[test]
    fn daemon_falls_back_to_local_only_layout_when_saved_layout_is_invalid_json() {
        let state_dir = temp_state_dir();
        let layout_path = rshare_core::service::layout_graph_path_in(&state_dir);
        let local_id = DeviceId::new_v4();

        std::fs::write(&layout_path, "{ definitely-not-json").unwrap();

        let loaded = load_layout_from_path(local_id, &layout_path).unwrap();

        assert_eq!(loaded, default_local_only_layout(local_id));
        assert!(
            layout_path.with_extension("json.invalid").exists(),
            "invalid layout should be retained for inspection"
        );
        let _ = std::fs::remove_dir_all(state_dir);
    }
}
