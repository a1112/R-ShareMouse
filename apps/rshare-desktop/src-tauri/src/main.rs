#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result as AnyhowResult;
use rshare_core::{
    daemon_client, Config, DaemonDeviceSnapshot, DeviceId, LayoutGraph, ServiceStatusSnapshot,
};
use serde::Serialize;
use std::{future::Future, pin::Pin};
use tauri::WebviewWindow;

type BoxFutureResult<'a, T> = Pin<Box<dyn Future<Output = AnyhowResult<T>> + Send + 'a>>;

#[derive(Debug, Clone, Serialize)]
struct DashboardStatePayload {
    status: Option<ServiceStatusSnapshot>,
    devices: Vec<DaemonDeviceSnapshot>,
}

#[tauri::command]
async fn dashboard_state() -> Result<DashboardStatePayload, String> {
    dashboard_state_with(
        || Box::pin(async { ensure_daemon_status().await }),
        || Box::pin(async { daemon_client::request_devices().await }),
    )
    .await
}

#[tauri::command]
async fn start_service() -> Result<ServiceStatusSnapshot, String> {
    let config = rshare_core::Config::load().unwrap_or_default();
    daemon_client::spawn_daemon(
        Some(config.network.port),
        Some(&config.network.bind_address),
    )
    .await
    .map_err(|err| err.to_string())
}

#[tauri::command]
async fn stop_service() -> Result<(), String> {
    daemon_client::request_shutdown()
        .await
        .map_err(|err| err.to_string())
}

fn parse_device_id(device_id: &str) -> Result<DeviceId, String> {
    device_id
        .parse()
        .map_err(|err| format!("Invalid device id: {err}"))
}

#[tauri::command]
async fn connect_device(device_id: String) -> Result<(), String> {
    let device_id = parse_device_id(&device_id)?;
    daemon_client::request_connect(device_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn disconnect_device(device_id: String) -> Result<(), String> {
    let device_id = parse_device_id(&device_id)?;
    daemon_client::request_disconnect(device_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn minimize_window(window: WebviewWindow) -> Result<(), String> {
    window.minimize().map_err(|err| err.to_string())
}

#[tauri::command]
fn toggle_maximize_window(window: WebviewWindow) -> Result<(), String> {
    if window.is_maximized().map_err(|err| err.to_string())? {
        window.unmaximize().map_err(|err| err.to_string())
    } else {
        window.maximize().map_err(|err| err.to_string())
    }
}

#[tauri::command]
fn close_window(window: WebviewWindow) -> Result<(), String> {
    window.close().map_err(|err| err.to_string())
}

#[tauri::command]
fn start_drag_window(window: WebviewWindow) -> Result<(), String> {
    window.start_dragging().map_err(|err| err.to_string())
}

// Configuration management
#[tauri::command]
async fn get_config() -> Result<Config, String> {
    Config::load().map_err(|err| err.to_string())
}

#[tauri::command]
async fn set_config(config: Config) -> Result<(), String> {
    config.save().map_err(|err| err.to_string())
}

// Layout management
#[tauri::command]
async fn get_layout() -> Result<LayoutGraph, String> {
    daemon_client::request_layout()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn set_layout(layout: LayoutGraph) -> Result<(), String> {
    daemon_client::request_set_layout(layout)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn show_tray() -> Result<(), String> {
    // TODO: Implement system tray via JavaScript frontend API
    Ok(())
}

fn is_ipc_unavailable(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(|io_err| io_err.kind() == std::io::ErrorKind::ConnectionRefused)
            .unwrap_or(false)
    })
}

async fn ensure_daemon_status() -> AnyhowResult<ServiceStatusSnapshot> {
    let config = Config::load().unwrap_or_default();
    let port = config.network.port;
    let bind_address = config.network.bind_address.clone();
    ensure_daemon_status_with(
        || Box::pin(async { daemon_client::request_status().await }),
        move || {
            let bind_address = bind_address.clone();
            Box::pin(async move {
                daemon_client::spawn_daemon(Some(port), Some(&bind_address)).await
            })
        },
    )
    .await
}

async fn ensure_daemon_status_with<Probe, Spawn>(
    mut probe_status: Probe,
    mut spawn_daemon: Spawn,
) -> AnyhowResult<ServiceStatusSnapshot>
where
    Probe: FnMut() -> BoxFutureResult<'static, ServiceStatusSnapshot>,
    Spawn: FnMut() -> BoxFutureResult<'static, ServiceStatusSnapshot>,
{
    match probe_status().await {
        Ok(status) => Ok(status),
        Err(err) if is_ipc_unavailable(&err) => spawn_daemon().await,
        Err(err) => Err(err),
    }
}

async fn dashboard_state_with<Ensure, Devices>(
    mut ensure_status: Ensure,
    mut request_devices: Devices,
) -> Result<DashboardStatePayload, String>
where
    Ensure: FnMut() -> BoxFutureResult<'static, ServiceStatusSnapshot>,
    Devices: FnMut() -> BoxFutureResult<'static, Vec<DaemonDeviceSnapshot>>,
{
    let status = ensure_status().await.map_err(|err| err.to_string())?;
    let devices = request_devices().await.unwrap_or_default();

    Ok(DashboardStatePayload {
        status: Some(status),
        devices,
    })
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            dashboard_state,
            start_service,
            stop_service,
            connect_device,
            disconnect_device,
            minimize_window,
            toggle_maximize_window,
            close_window,
            start_drag_window,
            get_config,
            set_config,
            get_layout,
            set_layout,
            show_tray
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Tauri desktop app");
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn sample_status() -> ServiceStatusSnapshot {
        ServiceStatusSnapshot {
            device_id: DeviceId::nil(),
            device_name: "desktop".to_string(),
            hostname: "localhost".to_string(),
            bind_address: "127.0.0.1:24801".to_string(),
            discovery_port: 24800,
            pid: 1,
            discovered_devices: 0,
            connected_devices: 0,
            healthy: true,
            input_mode: None,
            available_backends: Some(Vec::new()),
            backend_health: None,
            privilege_state: None,
            last_backend_error: None,
            session_state: None,
            active_target: None,
        }
    }

    #[tokio::test]
    async fn successful_probe_does_not_trigger_spawn() {
        let spawn_attempts = Arc::new(AtomicUsize::new(0));
        let expected = sample_status();

        let result = ensure_daemon_status_with(
            {
                let expected = expected.clone();
                move || Box::pin({
                    let expected = expected.clone();
                    async move { Ok(expected) }
                })
            },
            {
                let spawn_attempts = Arc::clone(&spawn_attempts);
                move || Box::pin({
                    let spawn_attempts = Arc::clone(&spawn_attempts);
                    async move {
                        spawn_attempts.fetch_add(1, Ordering::SeqCst);
                        Ok(sample_status())
                    }
                })
            },
        )
        .await
        .expect("probe should succeed");

        assert_eq!(result.device_id, expected.device_id);
        assert_eq!(spawn_attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn ipc_unavailable_startup_triggers_exactly_one_spawn_attempt() {
        let spawn_attempts = Arc::new(AtomicUsize::new(0));

        let result = ensure_daemon_status_with(
            || {
                Box::pin(async {
                    Err(anyhow!(std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "daemon offline",
                    )))
                })
            },
            {
                let spawn_attempts = Arc::clone(&spawn_attempts);
                move || Box::pin({
                    let spawn_attempts = Arc::clone(&spawn_attempts);
                    async move {
                        spawn_attempts.fetch_add(1, Ordering::SeqCst);
                        Ok(sample_status())
                    }
                })
            },
        )
        .await
        .expect("spawn should recover IPC-unavailable startup");

        assert!(result.healthy);
        assert_eq!(spawn_attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn non_ipc_failures_do_not_trigger_spawn() {
        let spawn_attempts = Arc::new(AtomicUsize::new(0));

        let result = ensure_daemon_status_with(
            || Box::pin(async { Err(anyhow!("daemon rejected status probe")) }),
            {
                let spawn_attempts = Arc::clone(&spawn_attempts);
                move || Box::pin({
                    let spawn_attempts = Arc::clone(&spawn_attempts);
                    async move {
                        spawn_attempts.fetch_add(1, Ordering::SeqCst);
                        Ok(sample_status())
                    }
                })
            },
        )
        .await;

        assert!(result.is_err());
        assert_eq!(spawn_attempts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn only_connection_refused_counts_as_ipc_unavailable() {
        assert!(is_ipc_unavailable(&anyhow!(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "daemon offline",
        ))));

        assert!(!is_ipc_unavailable(&anyhow!(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "probe timed out",
        ))));
    }

    #[tokio::test]
    async fn dashboard_state_surfaces_non_ipc_probe_failures() {
        let result = dashboard_state_with(
            || Box::pin(async { Err(anyhow!("daemon rejected status probe")) }),
            || Box::pin(async { Ok(Vec::new()) }),
        )
        .await;

        let err = result.expect_err("non-IPC probe failure should be surfaced");
        assert!(err.contains("daemon rejected status probe"));
    }

    #[tokio::test]
    async fn dashboard_state_surfaces_spawn_failures_after_ipc_miss() {
        let result = dashboard_state_with(
            || {
                Box::pin(async {
                    ensure_daemon_status_with(
                        || {
                            Box::pin(async {
                                Err(anyhow!(std::io::Error::new(
                                    std::io::ErrorKind::ConnectionRefused,
                                    "daemon offline",
                                )))
                            })
                        },
                        || Box::pin(async { Err(anyhow!("spawn failed")) }),
                    )
                    .await
                })
            },
            || Box::pin(async { Ok(Vec::new()) }),
        )
        .await;

        let err = result.expect_err("spawn failure should be surfaced");
        assert!(err.contains("spawn failed"));
    }
}
