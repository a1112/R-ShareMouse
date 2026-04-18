//! R-ShareMouse daemon service.
//!
//! Background service that handles input sharing and local IPC for status queries.

use anyhow::Result;
use rshare_core::{
    default_ipc_addr, read_json_line, write_json_line, BackendFailureReason, BackendHealth,
    BackendKind, Config, DaemonDeviceSnapshot, DaemonRequest, DaemonResponse, DeviceId,
    PrivilegeState, ResolvedInputMode, ServiceStatusSnapshot,
};
use rshare_net::{DiscoveredDevice, NetworkEvent, NetworkManager, NetworkManagerConfig};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::time::Instant;

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
    input_mode: Option<ResolvedInputMode>,
    available_backends: Vec<BackendKind>,
    backend_health: BackendHealth,
    privilege_state: PrivilegeState,
    last_backend_error: Option<String>,
}

impl DaemonState {
    fn new(status: ServiceStatusSnapshot) -> Self {
        Self {
            status,
            devices: HashMap::new(),
            input_mode: None,
            available_backends: vec![BackendKind::Portable],
            backend_health: BackendHealth::Healthy,
            privilege_state: PrivilegeState::UnlockedDesktop,
            last_backend_error: None,
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
                addresses: device.addresses.into_iter().map(|addr| addr.to_string()).collect(),
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
        }
    }

    fn status_snapshot(&self) -> ServiceStatusSnapshot {
        let mut snapshot = self.status.clone();
        snapshot.discovered_devices = self.devices.len();
        snapshot.connected_devices = self.devices.values().filter(|device| device.connected).count();
        snapshot.input_mode = self.input_mode;
        snapshot.available_backends = Some(self.available_backends.clone());
        snapshot.backend_health = Some(self.backend_health.clone());
        snapshot.privilege_state = Some(self.privilege_state);
        snapshot.last_backend_error = self.last_backend_error.clone();
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
        health: BackendHealth,
        error: Option<String>,
    ) {
        self.input_mode = mode;
        self.available_backends = available;
        self.backend_health = health;
        self.last_backend_error = error;
    }
}

fn default_backend_state() -> (
    Option<ResolvedInputMode>,
    Vec<BackendKind>,
    BackendHealth,
    Option<String>,
) {
    (
        Some(ResolvedInputMode::Portable),
        vec![BackendKind::Portable],
        BackendHealth::Degraded {
            reason: BackendFailureReason::Unavailable,
        },
        Some("Portable backend status is assumed until backend probing is reintroduced.".to_string()),
    )
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

    let (input_mode, available_backends, backend_health, backend_error) = default_backend_state();

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

    {
        let mut state_guard = state.write().await;
        state_guard.update_backend_state(
            input_mode,
            available_backends,
            backend_health,
            backend_error,
        );
    }

    let ipc_listener = TcpListener::bind(default_ipc_addr()).await?;
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(8);

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

    let event_task = {
        let state = state.clone();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let mut state_guard = state.write().await;
                match event {
                    NetworkEvent::DeviceFound(device) => state_guard.upsert_discovered(device),
                    NetworkEvent::DeviceConnected(id) => state_guard.mark_connected(&id, true),
                    NetworkEvent::DeviceDisconnected(id) => state_guard.remove_device(&id),
                    NetworkEvent::MessageReceived { .. } => {}
                    NetworkEvent::ConnectionError { device_id, error } => {
                        tracing::warn!("Connection error to {}: {}", device_id, error);
                        state_guard.mark_connected(&device_id, false);
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
    }

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
            let state_guard = state.read().await;
            DaemonResponse::Status(state_guard.status_snapshot())
        }
        DaemonRequest::Devices => {
            let state_guard = state.read().await;
            DaemonResponse::Devices(state_guard.device_snapshots())
        }
        DaemonRequest::Connect { device_id } => {
            let address = {
                let state_guard = state.read().await;
                state_guard
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
