#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result as AnyhowResult;
use rshare_core::{
    daemon_client, BackgroundProcessOwner, BackgroundRunMode, BackendHealth, Config,
    DaemonDeviceSnapshot, DeviceId, LayoutGraph, ServiceStatusSnapshot,
};
use serde::Serialize;
use std::{future::Future, pin::Pin};
use tauri::WebviewWindow;

type BoxFutureResult<'a, T> = Pin<Box<dyn Future<Output = AnyhowResult<T>> + Send + 'a>>;

#[derive(Debug, Clone, Serialize)]
struct DashboardStatePayload {
    status: Option<ServiceStatusSnapshot>,
    devices: Vec<DaemonDeviceSnapshot>,
    layout: Option<LayoutGraph>,
    visible_layout: Option<LayoutGraph>,
    layout_error: Option<String>,
    acceptance: DesktopAcceptancePayload,
    auto_started: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DesktopAcceptancePayload {
    daemon_online: bool,
    background_ready: bool,
    tray_owned_by_daemon: bool,
    tray_state: String,
    local_endpoint: String,
    discovered_devices: usize,
    connected_devices: usize,
    visible_layout_devices: usize,
    input_ready: bool,
    dual_machine_ready: bool,
    next_step: String,
}

#[derive(Debug, Clone)]
struct DesktopDaemonStatus {
    status: ServiceStatusSnapshot,
    auto_started: bool,
}

#[tauri::command]
async fn dashboard_state() -> Result<DashboardStatePayload, String> {
    dashboard_state_with(
        || Box::pin(async { ensure_daemon_status().await }),
        || Box::pin(async { daemon_client::request_devices().await }),
        || Box::pin(async { daemon_client::request_layout().await }),
        |layout| Box::pin(async move { daemon_client::request_set_layout(layout).await }),
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

async fn ensure_daemon_status() -> AnyhowResult<DesktopDaemonStatus> {
    let config = Config::load().unwrap_or_default();
    let port = config.network.port;
    let bind_address = config.network.bind_address.clone();
    ensure_daemon_status_with(
        || Box::pin(async { daemon_client::request_status().await }),
        move || {
            let bind_address = bind_address.clone();
            Box::pin(
                async move { daemon_client::spawn_daemon(Some(port), Some(&bind_address)).await },
            )
        },
    )
    .await
}

async fn ensure_daemon_status_with<Probe, Spawn>(
    mut probe_status: Probe,
    mut spawn_daemon: Spawn,
) -> AnyhowResult<DesktopDaemonStatus>
where
    Probe: FnMut() -> BoxFutureResult<'static, ServiceStatusSnapshot>,
    Spawn: FnMut() -> BoxFutureResult<'static, ServiceStatusSnapshot>,
{
    match probe_status().await {
        Ok(status) => Ok(DesktopDaemonStatus {
            status,
            auto_started: false,
        }),
        Err(err) if is_ipc_unavailable(&err) => {
            let status = spawn_daemon().await?;
            Ok(DesktopDaemonStatus {
                status,
                auto_started: true,
            })
        }
        Err(err) => Err(err),
    }
}

async fn dashboard_state_with<Ensure, Devices, Layout, SaveLayout>(
    mut ensure_status: Ensure,
    mut request_devices: Devices,
    mut request_layout: Layout,
    mut save_layout: SaveLayout,
) -> Result<DashboardStatePayload, String>
where
    Ensure: FnMut() -> BoxFutureResult<'static, DesktopDaemonStatus>,
    Devices: FnMut() -> BoxFutureResult<'static, Vec<DaemonDeviceSnapshot>>,
    Layout: FnMut() -> BoxFutureResult<'static, LayoutGraph>,
    SaveLayout: FnMut(LayoutGraph) -> BoxFutureResult<'static, ()>,
{
    let daemon = ensure_status().await.map_err(|err| err.to_string())?;
    let mut status = daemon.status;
    status.started_by_desktop = daemon.auto_started;
    let devices = request_devices().await.unwrap_or_default();
    let mut layout_error = None;
    let mut layout = match request_layout().await {
        Ok(layout) => {
            let original_layout = layout.clone();
            let mut remembered = layout;
            let changed =
                remembered.merge_discovered_peers_to_right(devices.iter().map(|device| device.id));
            if changed {
                match save_layout(remembered.clone()).await {
                    Ok(()) => Some(remembered),
                    Err(err) => {
                        layout_error = Some(err.to_string());
                        Some(original_layout)
                    }
                }
            } else {
                Some(remembered)
            }
        }
        Err(err) => {
            layout_error = Some(err.to_string());
            None
        }
    };

    let visible_layout = layout.as_ref().map(|remembered| {
        remembered.compact_online_display_projection(
            std::iter::once(status.device_id).chain(devices.iter().map(|device| device.id)),
        )
    });
    let acceptance = build_acceptance(
        &status,
        &devices,
        visible_layout.as_ref(),
        layout_error.as_deref(),
    );

    Ok(DashboardStatePayload {
        status: Some(status),
        devices,
        layout: layout.take(),
        visible_layout,
        layout_error,
        acceptance,
        auto_started: daemon.auto_started,
    })
}

fn build_acceptance(
    status: &ServiceStatusSnapshot,
    devices: &[DaemonDeviceSnapshot],
    visible_layout: Option<&LayoutGraph>,
    layout_error: Option<&str>,
) -> DesktopAcceptancePayload {
    let visible_layout_devices = visible_layout
        .map(|layout| layout.nodes.len())
        .unwrap_or_default();
    let input_ready = status.input_mode.is_some()
        && matches!(status.backend_health, Some(BackendHealth::Healthy));
    let background_ready = status.background_owner == BackgroundProcessOwner::Daemon
        && status.background_mode == BackgroundRunMode::BackgroundProcess;
    let tray_owned_by_daemon = status.tray_owner == BackgroundProcessOwner::Daemon;
    let dual_machine_ready = background_ready
        && input_ready
        && layout_error.is_none()
        && !devices.is_empty()
        && visible_layout_devices > 1;
    let next_step = if !background_ready {
        "后台服务未就绪，先启动守护进程"
    } else if !input_ready {
        "输入后端未就绪，先检查权限或后端降级"
    } else if devices.is_empty() {
        "打开另一台机器并保持同一局域网，等待自动发现"
    } else if layout_error.is_some() || visible_layout_devices <= 1 {
        "检查布局持久化，确认发现设备进入布局画布"
    } else {
        "打开另一台机器并连接设备，开始边缘切换验收"
    };

    DesktopAcceptancePayload {
        daemon_online: true,
        background_ready,
        tray_owned_by_daemon,
        tray_state: format!("{:?}", status.tray_state),
        local_endpoint: status.bind_address.clone(),
        discovered_devices: devices.len(),
        connected_devices: devices.iter().filter(|device| device.connected).count(),
        visible_layout_devices,
        input_ready,
        dual_machine_ready,
        next_step: next_step.to_string(),
    }
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
        Arc, Mutex,
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
            background_owner: rshare_core::BackgroundProcessOwner::Daemon,
            background_mode: rshare_core::BackgroundRunMode::BackgroundProcess,
            tray_owner: rshare_core::BackgroundProcessOwner::Daemon,
            tray_state: rshare_core::TrayRuntimeState::Unavailable,
            started_by_desktop: false,
        }
    }

    fn sample_layout(local_id: DeviceId) -> LayoutGraph {
        let mut layout = LayoutGraph::new(local_id);
        layout.add_node(rshare_core::LayoutNode::new(local_id, 0, 0, 1920, 1080));
        layout
    }

    #[tokio::test]
    async fn successful_probe_does_not_trigger_spawn() {
        let spawn_attempts = Arc::new(AtomicUsize::new(0));
        let expected = sample_status();

        let result = ensure_daemon_status_with(
            {
                let expected = expected.clone();
                move || {
                    Box::pin({
                        let expected = expected.clone();
                        async move { Ok(expected) }
                    })
                }
            },
            {
                let spawn_attempts = Arc::clone(&spawn_attempts);
                move || {
                    Box::pin({
                        let spawn_attempts = Arc::clone(&spawn_attempts);
                        async move {
                            spawn_attempts.fetch_add(1, Ordering::SeqCst);
                            Ok(sample_status())
                        }
                    })
                }
            },
        )
        .await
        .expect("probe should succeed");

        assert_eq!(result.status.device_id, expected.device_id);
        assert!(!result.auto_started);
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
                move || {
                    Box::pin({
                        let spawn_attempts = Arc::clone(&spawn_attempts);
                        async move {
                            spawn_attempts.fetch_add(1, Ordering::SeqCst);
                            Ok(sample_status())
                        }
                    })
                }
            },
        )
        .await
        .expect("spawn should recover IPC-unavailable startup");

        assert!(result.status.healthy);
        assert!(result.auto_started);
        assert_eq!(spawn_attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dashboard_state_marks_status_when_desktop_auto_started_daemon() {
        let result = dashboard_state_with(
            || {
                Box::pin(async {
                    let mut status = sample_status();
                    status.input_mode = Some(rshare_core::ResolvedInputMode::Portable);
                    status.backend_health = Some(rshare_core::BackendHealth::Healthy);
                    Ok(DesktopDaemonStatus {
                        status,
                        auto_started: true,
                    })
                })
            },
            || Box::pin(async { Ok(Vec::new()) }),
            || Box::pin(async { Ok(sample_layout(DeviceId::nil())) }),
            |_| Box::pin(async { Ok(()) }),
        )
        .await
        .expect("dashboard should annotate desktop auto-start");

        let status = result.status.expect("status should be online");
        assert!(status.started_by_desktop);
        assert!(result.auto_started);
        assert!(result.acceptance.daemon_online);
        assert!(result.acceptance.background_ready);
        assert!(result.acceptance.tray_owned_by_daemon);
        assert_eq!(result.acceptance.tray_state, "Unavailable");
    }

    #[tokio::test]
    async fn dashboard_state_reports_dual_machine_acceptance_readiness() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();

        let result = dashboard_state_with(
            move || {
                Box::pin({
                    let mut status = sample_status();
                    status.device_id = local_id;
                    status.input_mode = Some(rshare_core::ResolvedInputMode::Portable);
                    status.backend_health = Some(rshare_core::BackendHealth::Healthy);
                    async move {
                        Ok(DesktopDaemonStatus {
                            status,
                            auto_started: false,
                        })
                    }
                })
            },
            move || {
                Box::pin(async move {
                    Ok(vec![DaemonDeviceSnapshot {
                        id: remote_id,
                        name: "Remote".to_string(),
                        hostname: "remote-host".to_string(),
                        addresses: vec!["192.168.1.20".to_string()],
                        connected: false,
                        last_seen_secs: Some(1),
                    }])
                })
            },
            move || Box::pin(async move { Ok(sample_layout(local_id)) }),
            |_| Box::pin(async { Ok(()) }),
        )
        .await
        .expect("dashboard should expose acceptance readiness");

        assert!(result.acceptance.input_ready);
        assert_eq!(result.acceptance.discovered_devices, 1);
        assert_eq!(result.acceptance.visible_layout_devices, 2);
        assert!(result.acceptance.dual_machine_ready);
        assert_eq!(result.acceptance.next_step, "打开另一台机器并连接设备，开始边缘切换验收");
    }

    #[tokio::test]
    async fn non_ipc_failures_do_not_trigger_spawn() {
        let spawn_attempts = Arc::new(AtomicUsize::new(0));

        let result = ensure_daemon_status_with(
            || Box::pin(async { Err(anyhow!("daemon rejected status probe")) }),
            {
                let spawn_attempts = Arc::clone(&spawn_attempts);
                move || {
                    Box::pin({
                        let spawn_attempts = Arc::clone(&spawn_attempts);
                        async move {
                            spawn_attempts.fetch_add(1, Ordering::SeqCst);
                            Ok(sample_status())
                        }
                    })
                }
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
            || Box::pin(async { Ok(sample_layout(DeviceId::nil())) }),
            |_| Box::pin(async { Ok(()) }),
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
            || Box::pin(async { Ok(sample_layout(DeviceId::nil())) }),
            |_| Box::pin(async { Ok(()) }),
        )
        .await;

        let err = result.expect_err("spawn failure should be surfaced");
        assert!(err.contains("spawn failed"));
    }

    #[tokio::test]
    async fn dashboard_state_merges_discovered_devices_into_remembered_layout() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let saved_layout = Arc::new(Mutex::new(None::<LayoutGraph>));

        let result = dashboard_state_with(
            move || {
                Box::pin({
                    let mut status = sample_status();
                    status.device_id = local_id;
                    async move {
                        Ok(DesktopDaemonStatus {
                            status,
                            auto_started: false,
                        })
                    }
                })
            },
            move || {
                Box::pin(async move {
                    Ok(vec![DaemonDeviceSnapshot {
                        id: remote_id,
                        name: "Remote".to_string(),
                        hostname: "remote-host".to_string(),
                        addresses: vec!["192.168.1.20".to_string()],
                        connected: false,
                        last_seen_secs: Some(1),
                    }])
                })
            },
            move || Box::pin(async move { Ok(sample_layout(local_id)) }),
            {
                let saved_layout = Arc::clone(&saved_layout);
                move |layout| {
                    let saved_layout = Arc::clone(&saved_layout);
                    Box::pin(async move {
                        *saved_layout.lock().unwrap() = Some(layout);
                        Ok(())
                    })
                }
            },
        )
        .await
        .expect("dashboard state should merge layout");

        assert!(result
            .layout
            .as_ref()
            .unwrap()
            .get_node(remote_id)
            .is_some());
        assert!(result
            .visible_layout
            .as_ref()
            .unwrap()
            .get_node(remote_id)
            .is_some());
        assert!(saved_layout.lock().unwrap().is_some());
    }

    #[tokio::test]
    async fn dashboard_state_visible_layout_hides_offline_remembered_nodes() {
        let local_id = DeviceId::new_v4();
        let offline_id = DeviceId::new_v4();
        let online_id = DeviceId::new_v4();
        let mut remembered = sample_layout(local_id);
        remembered.add_node(rshare_core::LayoutNode::new(
            offline_id, 1920, 0, 1920, 1080,
        ));
        remembered.add_node(rshare_core::LayoutNode::new(online_id, 3840, 0, 1920, 1080));

        let result = dashboard_state_with(
            move || {
                Box::pin({
                    let mut status = sample_status();
                    status.device_id = local_id;
                    async move {
                        Ok(DesktopDaemonStatus {
                            status,
                            auto_started: false,
                        })
                    }
                })
            },
            move || {
                Box::pin(async move {
                    Ok(vec![DaemonDeviceSnapshot {
                        id: online_id,
                        name: "Online".to_string(),
                        hostname: "online-host".to_string(),
                        addresses: vec!["192.168.1.21".to_string()],
                        connected: false,
                        last_seen_secs: Some(1),
                    }])
                })
            },
            move || {
                let remembered = remembered.clone();
                Box::pin(async move { Ok(remembered) })
            },
            |_| Box::pin(async { Ok(()) }),
        )
        .await
        .expect("dashboard state should build visible layout");

        assert!(result
            .layout
            .as_ref()
            .unwrap()
            .get_node(offline_id)
            .is_some());
        assert!(result
            .visible_layout
            .as_ref()
            .unwrap()
            .get_node(offline_id)
            .is_none());
        assert_eq!(
            result
                .visible_layout
                .as_ref()
                .unwrap()
                .get_node(online_id)
                .unwrap()
                .primary_display()
                .unwrap()
                .x,
            1920
        );
    }

    #[tokio::test]
    async fn dashboard_state_reports_layout_save_failure() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();

        let result = dashboard_state_with(
            move || {
                Box::pin({
                    let mut status = sample_status();
                    status.device_id = local_id;
                    async move {
                        Ok(DesktopDaemonStatus {
                            status,
                            auto_started: false,
                        })
                    }
                })
            },
            move || {
                Box::pin(async move {
                    Ok(vec![DaemonDeviceSnapshot {
                        id: remote_id,
                        name: "Remote".to_string(),
                        hostname: "remote-host".to_string(),
                        addresses: vec!["192.168.1.20".to_string()],
                        connected: false,
                        last_seen_secs: Some(1),
                    }])
                })
            },
            move || Box::pin(async move { Ok(sample_layout(local_id)) }),
            |_| Box::pin(async { Err(anyhow!("layout save failed")) }),
        )
        .await
        .expect("dashboard should still return status");

        assert_eq!(result.layout_error.as_deref(), Some("layout save failed"));
        assert!(result.layout.as_ref().unwrap().get_node(local_id).is_some());
        assert!(result
            .layout
            .as_ref()
            .unwrap()
            .get_node(remote_id)
            .is_none());
        assert!(result
            .visible_layout
            .as_ref()
            .unwrap()
            .get_node(remote_id)
            .is_none());
    }
}
