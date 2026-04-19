//! R-ShareMouse daemon service.
//!
//! Background service that handles input sharing and local IPC for status queries.

use anyhow::Result;
use rshare_core::{
    default_ipc_addr, read_json_line, write_json_line, BackendFailureReason, BackendHealth,
    BackendKind, BackendRuntimeState, CaptureSessionStateMachine, Config, DaemonDeviceSnapshot,
    DaemonRequest, DaemonResponse, DeviceId, Direction, LayoutGraph, LayoutNode, Message,
    ResolvedInputMode, ServiceStatusSnapshot,
};
use rshare_input::{
    BackendCandidate, BackendSelector, CaptureBackend, DefaultInputListener, InjectBackend,
    InputEvent, InputListener, PortableCaptureBackend, PortableInjectBackend,
};
use rshare_net::{DiscoveredDevice, NetworkEvent, NetworkManager, NetworkManagerConfig};
use std::collections::HashMap;
use std::sync::Arc;
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
        // Add local device to layout
        layout.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));

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
        if matches!(self.backend_state.aggregate_health, BackendHealth::Degraded { .. }) {
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

    fn remote_target(&self) -> Option<DeviceId> {
        self.remote_target
    }

    fn clear_remote_target(&mut self) {
        self.remote_target = None;
    }

    fn set_remote_target(&mut self, target: DeviceId) {
        self.remote_target = Some(target);
    }

    fn is_right_edge_activation(&self, event: &InputEvent) -> bool {
        let InputEvent::MouseMove { x, y } = event else {
            return false;
        };

        let right_edge_start = self.screen_width.saturating_sub(self.edge_threshold) as i32;
        *x >= right_edge_start && self.is_vertical_screen_coordinate(*y)
    }

    fn is_left_edge_release(&self, event: &InputEvent) -> bool {
        let InputEvent::MouseMove { x, y } = event else {
            return false;
        };

        *x <= self.edge_threshold as i32 && self.is_vertical_screen_coordinate(*y)
    }

    fn is_vertical_screen_coordinate(&self, y: i32) -> bool {
        y >= 0 && y < self.screen_height as i32
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
    state: &DaemonState,
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

    // Handle edge activation using layout graph
    if routing.is_right_edge_activation(&event) {
        if let Some(target) = state.layout.resolve_target(local_id, Direction::Right, &connected_peers) {
            routing.set_remote_target(target);
        }
    } else if routing.is_left_edge_release(&event) {
        routing.clear_remote_target();
        forwarder.clear_target();
        return Vec::new();
    }

    let target = if let Some(remote_target) = routing.remote_target() {
        if !is_device_connected(state, remote_target) {
            routing.clear_remote_target();
            forwarder.clear_target();
            return Vec::new();
        }
        remote_target
    } else {
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
                    let state = state.read().await;
                    let messages =
                        messages_for_input_event(&state, &mut routing, &mut forwarder, event);
                    let target = routing.remote_target();
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
                    routing
                        .remote_target()
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

    let config = load_config_with_env_overrides()?;

    let hostname = hostname::get()
        .unwrap_or_else(|_| "unknown".into())
        .to_string_lossy()
        .to_string();
    let device_id = DeviceId::new_v4();
    let device_name = format!("{}-R-ShareMouse", hostname);
    let bind_address = format!("{}:{}", config.network.bind_address, config.network.port);

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

    let state = Arc::new(RwLock::new(DaemonState::new(ServiceStatusSnapshot::new(
        device_id,
        device_name.clone(),
        hostname.clone(),
        bind_address.clone(),
        27432,
        pid,
    ))));

    // Initialize backend state
    {
        let mut s = state.write().await;
        s.update_backend_state(
            input_mode,
            available_backends,
            backend_health.clone(),  // capture health
            backend_health,          // inject health
            backend_error,
        );
    }
    let inject_backend = Arc::new(Mutex::new(create_inject_backend(input_mode)?));

    let ipc_listener = TcpListener::bind(default_ipc_addr()).await?;
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(8);
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();
    let mut input_listener = DefaultInputListener::new();
    input_listener.start(Box::new(move |event| {
        let _ = input_tx.send(event);
    }))?;

    tracing::info!("Daemon started as device {} ({})", device_name, device_id);
    tracing::info!("Listening for connections on {}", bind_address);
    tracing::info!("Device discovery on port 27432");
    tracing::info!("Local IPC listening on {}", default_ipc_addr());

    let ipc_task = tokio::spawn(run_ipc_server(
        ipc_listener,
        state.clone(),
        network_manager.clone(),
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
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let mut state = state.write().await;
                match event {
                    NetworkEvent::DeviceFound(device) => state.upsert_discovered(device),
                    NetworkEvent::DeviceConnected(id) => state.mark_connected(&id, true),
                    NetworkEvent::DeviceDisconnected(id) => {
                        // Notify session state machine of target disconnection
                        state.session.on_target_disconnect(id);
                        state.remove_device(&id);
                    }
                    NetworkEvent::MessageReceived { from, message } => {
                        drop(state);
                        inject_remote_message(&inject_backend, from, message).await;
                    }
                    NetworkEvent::ConnectionError { device_id, error } => {
                        tracing::warn!("Connection error to {}: {}", device_id, error);
                        state.mark_connected(&device_id, false);
                    }
                }
            }
        })
    };

    tokio::select! {
        _ = signal::ctrl_c() => {
            tracing::info!("Shutdown signal received");
        }
        _ = shutdown_rx.recv() => {
            tracing::info!("Shutdown requested over IPC");
        }
        result = ipc_task => {
            result??;
        }
        result = event_task => {
            result?;
        }
        result = input_forwarding_task => {
            result??;
        }
    }

    input_listener.stop()?;
    network_manager.lock().await.stop().await?;

    tracing::info!("R-ShareMouse daemon stopped");
    Ok(())
}

async fn run_ipc_server(
    listener: TcpListener,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let network_manager = network_manager.clone();
        let shutdown_tx = shutdown_tx.clone();

        tokio::spawn(async move {
            if let Err(err) = handle_ipc_client(stream, state, network_manager, shutdown_tx).await {
                tracing::debug!("IPC client error: {}", err);
            }
        });
    }
}

async fn handle_ipc_client(
    mut stream: TcpStream,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
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
            state.layout = layout;
            DaemonResponse::Ack
        }
        DaemonRequest::Shutdown => {
            let _ = shutdown_tx.send(());
            DaemonResponse::Ack
        }
    };

    write_json_line(&mut stream, &response).await
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
        let state = DaemonState::new(ServiceStatusSnapshot::new(
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
            &state,
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
            &state,
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
        use rshare_core::{LayoutLink, Direction};
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
        state.layout.add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let messages = messages_for_input_event(
            &state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
        );

        assert_eq!(routing.remote_target(), Some(remote_id));
        assert_eq!(forwarder.target(), Some(remote_id));
        assert!(!messages.is_empty());
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
            &state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
        );
        let messages = messages_for_input_event(
            &state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(0, 500),
        );

        assert!(messages.is_empty());
        assert_eq!(routing.remote_target(), None);
        assert_eq!(forwarder.target(), None);
    }

    #[test]
    fn input_event_forwarding_targets_first_connected_device() {
        use rshare_core::{LayoutLink, Direction};
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
        state.layout.add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let _ = messages_for_input_event(
            &state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
        );
        let messages = messages_for_input_event(
            &state,
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
        use rshare_core::{LayoutGraph, Direction};
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
                name: "z-remote-last".to_string(),  // Name sorted last, but should not be used
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
        use rshare_core::{LayoutGraph, LayoutNode, LayoutLink, Direction};
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
                name: "a-device".to_string(),  // Would be first in name sort
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
        assert_eq!(target, Some(remote_b), "Should route to layout-linked device");
        assert_ne!(target, Some(remote_a), "Should not route to first-connected device");
    }

    #[test]
    fn daemon_disconnect_clears_remote_active_session() {
        use rshare_core::{CaptureSessionStateMachine, ControlSessionState, Direction, SuspendReason};

        let remote_id = DeviceId::new_v4();
        let mut machine = CaptureSessionStateMachine::new();

        // Enter remote mode
        machine.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
        assert!(matches!(machine.state(), ControlSessionState::RemoteActive { .. }));

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
        use rshare_core::{CaptureSessionStateMachine, ControlSessionState, Direction, SuspendReason};

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
        assert_eq!(snapshot.session_state, Some(ControlSessionState::LocalReady));
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
        state.session.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
        let snapshot = state.status_snapshot();
        assert!(matches!(snapshot.session_state, Some(ControlSessionState::RemoteActive { .. })));

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
        state.session.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
        state.session.on_target_disconnect(remote_id);

        // Reset session
        state.session.reset();
        let snapshot = state.status_snapshot();
        assert_eq!(snapshot.session_state, Some(ControlSessionState::LocalReady));

        // Can enter remote mode again
        state.session.on_edge_hit(Direction::Right, Some(remote_id)).unwrap();
        let snapshot = state.status_snapshot();
        assert!(matches!(snapshot.session_state, Some(ControlSessionState::RemoteActive { .. })));
    }
}
