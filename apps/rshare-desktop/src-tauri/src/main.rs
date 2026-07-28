#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result as AnyhowResult;
use rshare_core::{
    daemon_client, BackendHealth, BackgroundProcessOwner, BackgroundRunMode,
    CapabilityRegistrySnapshot, Config, DaemonDeviceSnapshot, DaemonResponse, DeviceId,
    DisplayCaptureRequest, DisplayCaptureResult, DisplayIdentifyRequest, DisplayIdentifyResult,
    DisplaySettingsUpdateRequest, DisplaySettingsUpdateResult, EndpointEvent, EndpointEventFilter,
    EndpointInjectRequest, EndpointInjectResult, EndpointInjectTarget, LayoutGraph,
    LocalControlDeviceSnapshot, LocalDisplayState, LocalInputTestKind, LocalInputTestRequest,
    LocalInputTestResult, MobileAccessSnapshot, ServiceStatusSnapshot, VirtualDisplayCreateRequest,
    VirtualDisplayOperationResult, VirtualDisplayRemoveRequest, VirtualDisplaySnapshot,
};
use serde::Serialize;
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WebviewWindow, Wry,
};
use tauri_plugin_single_instance::init;

mod hardware_assets;

const UNKNOWN_BUILD_VALUE: &str = "unknown";
const RSHARE_BUILD_INFO_PREFIX: &str = "rshare-build:";

fn build_metadata() -> String {
    let git_hash = option_env!("RSHARE_BUILD_GIT_HASH").unwrap_or(UNKNOWN_BUILD_VALUE);
    let build_timestamp = option_env!("RSHARE_BUILD_TIMESTAMP").unwrap_or(UNKNOWN_BUILD_VALUE);
    let dirty = option_env!("RSHARE_BUILD_DIRTY").unwrap_or("");
    let dirty_suffix = if dirty.is_empty() {
        String::new()
    } else {
        format!(" ({dirty})")
    };

    format!(
        "{RSHARE_BUILD_INFO_PREFIX} version={} commit={} time={}{}",
        env!("CARGO_PKG_VERSION"),
        git_hash,
        build_timestamp,
        dirty_suffix
    )
}

type BoxFutureResult<'a, T> = Pin<Box<dyn Future<Output = AnyhowResult<T>> + Send + 'a>>;
const TRAY_ICON_ID: &str = "main-tray";
const TRAY_STATUS_REFRESH_MS: u64 = 2_000;
const TRAY_MENU_STATUS_ID: &str = "tray-status";
const TRAY_MENU_SHOW_ID: &str = "tray-show";
const TRAY_MENU_HIDE_ID: &str = "tray-hide";
const TRAY_MENU_START_SERVICE_ID: &str = "tray-start-service";
const TRAY_MENU_STOP_SERVICE_ID: &str = "tray-stop-service";
const TRAY_MENU_DISPLAY_SETTINGS_ID: &str = "tray-display-settings";
const TRAY_MENU_OPEN_CONFIG_DIR_ID: &str = "tray-open-config-dir";
const TRAY_MENU_OPEN_LOG_ID: &str = "tray-open-log";
const TRAY_MENU_QUIT_ID: &str = "tray-quit";

#[derive(Debug, Clone, Serialize)]
struct DashboardStatePayload {
    status: Option<ServiceStatusSnapshot>,
    devices: Vec<DaemonDeviceSnapshot>,
    capabilities: Option<CapabilityRegistrySnapshot>,
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
    local_display_count: usize,
    local_ready: bool,
    input_ready: bool,
    dual_machine_ready: bool,
    next_step: String,
}

#[derive(Debug, Clone)]
struct DesktopDaemonStatus {
    status: ServiceStatusSnapshot,
    auto_started: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseRequestAction {
    PreventAndHide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    None,
    ShowWindow,
    HideWindow,
    StartService,
    StopService,
    OpenDisplaySettings,
    OpenConfigDir,
    OpenLogFile,
    QuitApp,
}

struct TrayRuntimeHandles {
    tray: TrayIcon<Wry>,
    _menu: Menu<Wry>,
    status_item: MenuItem<Wry>,
    start_service_item: MenuItem<Wry>,
    stop_service_item: MenuItem<Wry>,
}

#[derive(Default)]
struct LocalControlStreamState {
    task: std::sync::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

#[derive(Default)]
struct EndpointEventStreamState {
    task: std::sync::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayStatusView {
    status_text: String,
    tooltip: String,
    service_running: bool,
}

#[tauri::command]
async fn dashboard_state() -> Result<DashboardStatePayload, String> {
    dashboard_state_with(
        || Box::pin(async { ensure_daemon_status().await }),
        || Box::pin(async { daemon_client::request_devices().await }),
        || Box::pin(async { daemon_client::request_capabilities(None).await }),
        || Box::pin(async { daemon_client::request_layout().await }),
        |layout| Box::pin(async move { daemon_client::request_set_layout(layout).await }),
    )
    .await
}

#[tauri::command]
async fn start_service(app: AppHandle) -> Result<ServiceStatusSnapshot, String> {
    let config = rshare_core::Config::load().unwrap_or_default();
    let status = daemon_client::spawn_daemon(
        Some(config.network.port),
        Some(&config.network.bind_address),
    )
    .await
    .map_err(|err| err.to_string())?;
    refresh_tray_status_once(&app).await;
    Ok(status)
}

#[tauri::command]
async fn stop_service(app: AppHandle) -> Result<(), String> {
    daemon_client::request_shutdown()
        .await
        .map_err(|err| err.to_string())?;
    refresh_tray_status_once(&app).await;
    Ok(())
}

fn parse_device_id(device_id: &str) -> Result<DeviceId, String> {
    device_id
        .parse()
        .map_err(|err| format!("Invalid device id: {err}"))
}

fn parse_optional_device_id(device_id: Option<String>) -> Result<Option<DeviceId>, String> {
    match device_id {
        Some(value) if !value.trim().is_empty() => parse_device_id(value.trim()).map(Some),
        _ => Ok(None),
    }
}

#[tauri::command]
async fn capabilities_state(
    device_id: Option<String>,
) -> Result<CapabilityRegistrySnapshot, String> {
    let device_id = parse_optional_device_id(device_id)?;
    daemon_client::request_capabilities(device_id)
        .await
        .map_err(|err| err.to_string())
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
async fn local_controls_state() -> Result<LocalControlDeviceSnapshot, String> {
    local_controls_state_with(
        || Box::pin(async { daemon_client::request_local_controls().await }),
        || rshare_platform::display::query_display_state(),
    )
    .await
}

#[tauri::command]
async fn mobile_access() -> Result<MobileAccessSnapshot, String> {
    daemon_client::request_mobile_access()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn capture_display(
    display_id: String,
    max_width: Option<u32>,
) -> Result<DisplayCaptureResult, String> {
    capture_display_with(
        DisplayCaptureRequest {
            display_id,
            max_width,
        },
        |request| Box::pin(async { daemon_client::request_display_capture(request).await }),
        |request| rshare_platform::display::capture_display(&request),
    )
    .await
}

#[tauri::command]
async fn identify_displays(duration_ms: Option<u32>) -> Result<DisplayIdentifyResult, String> {
    identify_displays_with(
        DisplayIdentifyRequest { duration_ms },
        |request| Box::pin(async { daemon_client::request_identify_displays(request).await }),
        |request| rshare_platform::display::identify_displays(&request),
    )
    .await
}

#[tauri::command]
async fn update_display_settings(
    request: DisplaySettingsUpdateRequest,
) -> Result<DisplaySettingsUpdateResult, String> {
    update_display_settings_with(
        request,
        |request| Box::pin(async { daemon_client::request_update_display_settings(request).await }),
        |request| rshare_platform::display::update_display_settings(&request),
    )
    .await
}

#[tauri::command]
async fn open_display_settings() -> Result<(), String> {
    match daemon_client::request_open_display_settings().await {
        Ok(()) => Ok(()),
        Err(err) if is_ipc_unavailable(&err) => rshare_platform::display::open_display_settings()
            .map_err(|fallback_err| fallback_err.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

#[tauri::command]
async fn list_virtual_displays() -> Result<Vec<VirtualDisplaySnapshot>, String> {
    list_virtual_displays_with(
        || Box::pin(async { daemon_client::request_virtual_displays().await }),
        || rshare_platform::virtual_display::list_virtual_displays(),
    )
    .await
}

#[tauri::command]
async fn create_virtual_display(
    request: VirtualDisplayCreateRequest,
) -> Result<VirtualDisplayOperationResult, String> {
    create_virtual_display_with(
        request,
        |request| Box::pin(async { daemon_client::request_create_virtual_display(request).await }),
        |request| rshare_platform::virtual_display::create_virtual_display(&request),
    )
    .await
}

#[tauri::command]
async fn remove_virtual_display(
    request: VirtualDisplayRemoveRequest,
) -> Result<VirtualDisplayOperationResult, String> {
    remove_virtual_display_with(
        request,
        |request| Box::pin(async { daemon_client::request_remove_virtual_display(request).await }),
        |request| rshare_platform::virtual_display::remove_virtual_display(&request),
    )
    .await
}

#[tauri::command]
async fn endpoint_events_state(
    filter: EndpointEventFilter,
    after_sequence: Option<u64>,
    limit: Option<u16>,
) -> Result<Vec<EndpointEvent>, String> {
    daemon_client::request_endpoint_events(filter, after_sequence, limit)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn inject_endpoint_event(
    target: EndpointInjectTarget,
    request: EndpointInjectRequest,
) -> Result<EndpointInjectResult, String> {
    daemon_client::request_endpoint_inject(target, request)
        .await
        .map_err(|err| err.to_string())
}

fn local_input_test_kind_from_str(kind: &str) -> Result<LocalInputTestKind, String> {
    match kind {
        "keyboard" | "keyboard_shift" | "KeyboardShift" => Ok(LocalInputTestKind::KeyboardShift),
        "mouse" | "mouse_move" | "MouseMove" => Ok(LocalInputTestKind::MouseMove),
        "virtual_gamepad_status" | "VirtualGamepadStatus" => {
            Ok(LocalInputTestKind::VirtualGamepadStatus)
        }
        other => Err(format!("Unknown local input test kind: {other}")),
    }
}

#[tauri::command]
async fn run_local_input_test(kind: String) -> Result<LocalInputTestResult, String> {
    let kind = local_input_test_kind_from_str(&kind)?;
    daemon_client::request_local_input_test(LocalInputTestRequest { kind })
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn run_remote_latency_test(device_id: String) -> Result<LocalInputTestResult, String> {
    let device_id = parse_device_id(&device_id)?;
    daemon_client::request_remote_latency_test(device_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn start_local_controls_stream(app: AppHandle) -> Result<(), String> {
    let state = app.state::<LocalControlStreamState>();
    if let Some(task) = state.task.lock().map_err(|err| err.to_string())?.take() {
        task.abort();
    }

    let app_for_task = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        if let Ok(mut websocket) = daemon_client::subscribe_local_controls_ws().await {
            loop {
                match daemon_client::read_local_control_ws_event(&mut websocket).await {
                    Ok(response @ DaemonResponse::LocalControls(_))
                    | Ok(response @ DaemonResponse::LocalControlEvent(_)) => {
                        let _ = app_for_task.emit("local-control-event", response);
                    }
                    Ok(_) => {}
                    Err(err) => {
                        let _ = app_for_task.emit("local-control-event", format!("error:{err}"));
                        break;
                    }
                }
            }
            return;
        }

        let mut stream = match daemon_client::subscribe_local_controls().await {
            Ok(stream) => stream,
            Err(err) => {
                let _ = app_for_task.emit("local-control-event", format!("error:{err}"));
                return;
            }
        };

        loop {
            match daemon_client::read_local_control_event(&mut stream).await {
                Ok(response @ DaemonResponse::LocalControls(_))
                | Ok(response @ DaemonResponse::LocalControlEvent(_)) => {
                    let _ = app_for_task.emit("local-control-event", response);
                }
                Ok(_) => {}
                Err(err) => {
                    let _ = app_for_task.emit("local-control-event", format!("error:{err}"));
                    break;
                }
            }
        }
    });
    *state.task.lock().map_err(|err| err.to_string())? = Some(task);
    Ok(())
}

#[tauri::command]
async fn stop_local_controls_stream(app: AppHandle) -> Result<(), String> {
    let state = app.state::<LocalControlStreamState>();
    if let Some(task) = state.task.lock().map_err(|err| err.to_string())?.take() {
        task.abort();
    }
    Ok(())
}

#[tauri::command]
async fn start_endpoint_events_stream(
    app: AppHandle,
    filter: EndpointEventFilter,
) -> Result<(), String> {
    let state = app.state::<EndpointEventStreamState>();
    if let Some(task) = state.task.lock().map_err(|err| err.to_string())?.take() {
        task.abort();
    }

    let app_for_task = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let mut stream = match daemon_client::subscribe_endpoint_events(filter).await {
            Ok(stream) => stream,
            Err(err) => {
                let _ = app_for_task.emit("endpoint-event", format!("error:{err}"));
                return;
            }
        };

        loop {
            match daemon_client::read_local_control_event(&mut stream).await {
                Ok(response @ DaemonResponse::EndpointEvents(_))
                | Ok(response @ DaemonResponse::EndpointEvent(_))
                | Ok(response @ DaemonResponse::EndpointInjectResult(_)) => {
                    let _ = app_for_task.emit("endpoint-event", response);
                }
                Ok(_) => {}
                Err(err) => {
                    let _ = app_for_task.emit("endpoint-event", format!("error:{err}"));
                    break;
                }
            }
        }
    });
    *state.task.lock().map_err(|err| err.to_string())? = Some(task);
    Ok(())
}

#[tauri::command]
async fn stop_endpoint_events_stream(app: AppHandle) -> Result<(), String> {
    let state = app.state::<EndpointEventStreamState>();
    if let Some(task) = state.task.lock().map_err(|err| err.to_string())?.take() {
        task.abort();
    }
    Ok(())
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
    match close_request_action() {
        CloseRequestAction::PreventAndHide => window.hide().map_err(|err| err.to_string()),
    }
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
async fn list_hardware_assets(
    app: AppHandle,
) -> Result<Vec<hardware_assets::InstalledHardwareAsset>, String> {
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    hardware_assets::list_installed_hardware_assets(&dir).map_err(|err| err.to_string())
}

#[tauri::command]
async fn import_hardware_asset(
    app: AppHandle,
    bytes: Vec<u8>,
) -> Result<hardware_assets::InstalledHardwareAsset, String> {
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    hardware_assets::import_hardware_asset_package(&dir, &bytes).map_err(|err| err.to_string())
}

#[tauri::command]
async fn export_hardware_asset(app: AppHandle, asset_id: String) -> Result<Vec<u8>, String> {
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    hardware_assets::export_hardware_asset_package(&dir, &asset_id).map_err(|err| err.to_string())
}

#[tauri::command]
async fn show_tray(app: AppHandle) -> Result<(), String> {
    show_main_window(&app)
}

fn is_ipc_unavailable(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(|io_err| io_err.kind() == std::io::ErrorKind::ConnectionRefused)
            .unwrap_or(false)
    })
}

async fn local_controls_state_with<DaemonLocalControls, PlatformDisplay>(
    mut request_daemon: DaemonLocalControls,
    mut query_platform_display: PlatformDisplay,
) -> Result<LocalControlDeviceSnapshot, String>
where
    DaemonLocalControls: FnMut() -> BoxFutureResult<'static, LocalControlDeviceSnapshot>,
    PlatformDisplay: FnMut() -> AnyhowResult<LocalDisplayState>,
{
    match request_daemon().await {
        Ok(snapshot) => Ok(snapshot),
        Err(err) if is_ipc_unavailable(&err) => {
            let mut snapshot = LocalControlDeviceSnapshot::default();
            snapshot.display =
                query_platform_display().map_err(|fallback_err| fallback_err.to_string())?;
            Ok(snapshot)
        }
        Err(err) => Err(err.to_string()),
    }
}

async fn capture_display_with<DaemonCapture, PlatformCapture>(
    request: DisplayCaptureRequest,
    mut request_daemon: DaemonCapture,
    mut capture_platform: PlatformCapture,
) -> Result<DisplayCaptureResult, String>
where
    DaemonCapture: FnMut(DisplayCaptureRequest) -> BoxFutureResult<'static, DisplayCaptureResult>,
    PlatformCapture: FnMut(DisplayCaptureRequest) -> AnyhowResult<DisplayCaptureResult>,
{
    match request_daemon(request.clone()).await {
        Ok(result) => Ok(result),
        Err(err) if is_ipc_unavailable(&err) => {
            capture_platform(request).map_err(|fallback_err| fallback_err.to_string())
        }
        Err(err) => Err(err.to_string()),
    }
}

async fn identify_displays_with<DaemonIdentify, PlatformIdentify>(
    request: DisplayIdentifyRequest,
    mut request_daemon: DaemonIdentify,
    mut identify_platform: PlatformIdentify,
) -> Result<DisplayIdentifyResult, String>
where
    DaemonIdentify:
        FnMut(DisplayIdentifyRequest) -> BoxFutureResult<'static, DisplayIdentifyResult>,
    PlatformIdentify: FnMut(DisplayIdentifyRequest) -> AnyhowResult<DisplayIdentifyResult>,
{
    match request_daemon(request.clone()).await {
        Ok(result) => Ok(result),
        Err(err) if is_ipc_unavailable(&err) => {
            identify_platform(request).map_err(|fallback_err| fallback_err.to_string())
        }
        Err(err) => Err(err.to_string()),
    }
}

async fn update_display_settings_with<DaemonUpdate, PlatformUpdate>(
    request: DisplaySettingsUpdateRequest,
    mut request_daemon: DaemonUpdate,
    mut update_platform: PlatformUpdate,
) -> Result<DisplaySettingsUpdateResult, String>
where
    DaemonUpdate: FnMut(
        DisplaySettingsUpdateRequest,
    ) -> BoxFutureResult<'static, DisplaySettingsUpdateResult>,
    PlatformUpdate:
        FnMut(DisplaySettingsUpdateRequest) -> AnyhowResult<DisplaySettingsUpdateResult>,
{
    match request_daemon(request.clone()).await {
        Ok(result) => Ok(result),
        Err(err) if is_ipc_unavailable(&err) => {
            update_platform(request).map_err(|fallback_err| fallback_err.to_string())
        }
        Err(err) => Err(err.to_string()),
    }
}

async fn list_virtual_displays_with<DaemonList, PlatformList>(
    mut request_daemon: DaemonList,
    mut list_platform: PlatformList,
) -> Result<Vec<VirtualDisplaySnapshot>, String>
where
    DaemonList: FnMut() -> BoxFutureResult<'static, Vec<VirtualDisplaySnapshot>>,
    PlatformList: FnMut() -> AnyhowResult<Vec<VirtualDisplaySnapshot>>,
{
    match request_daemon().await {
        Ok(displays) => Ok(displays),
        Err(err) if is_ipc_unavailable(&err) => {
            list_platform().map_err(|fallback_err| fallback_err.to_string())
        }
        Err(err) => Err(err.to_string()),
    }
}

async fn create_virtual_display_with<DaemonCreate, PlatformCreate>(
    request: VirtualDisplayCreateRequest,
    mut request_daemon: DaemonCreate,
    mut create_platform: PlatformCreate,
) -> Result<VirtualDisplayOperationResult, String>
where
    DaemonCreate: FnMut(
        VirtualDisplayCreateRequest,
    ) -> BoxFutureResult<'static, VirtualDisplayOperationResult>,
    PlatformCreate:
        FnMut(VirtualDisplayCreateRequest) -> AnyhowResult<VirtualDisplayOperationResult>,
{
    match request_daemon(request.clone()).await {
        Ok(result) => Ok(result),
        Err(err) if is_ipc_unavailable(&err) => {
            create_platform(request).map_err(|fallback_err| fallback_err.to_string())
        }
        Err(err) => Err(err.to_string()),
    }
}

async fn remove_virtual_display_with<DaemonRemove, PlatformRemove>(
    request: VirtualDisplayRemoveRequest,
    mut request_daemon: DaemonRemove,
    mut remove_platform: PlatformRemove,
) -> Result<VirtualDisplayOperationResult, String>
where
    DaemonRemove: FnMut(
        VirtualDisplayRemoveRequest,
    ) -> BoxFutureResult<'static, VirtualDisplayOperationResult>,
    PlatformRemove:
        FnMut(VirtualDisplayRemoveRequest) -> AnyhowResult<VirtualDisplayOperationResult>,
{
    match request_daemon(request.clone()).await {
        Ok(result) => Ok(result),
        Err(err) if is_ipc_unavailable(&err) => {
            remove_platform(request).map_err(|fallback_err| fallback_err.to_string())
        }
        Err(err) => Err(err.to_string()),
    }
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

async fn dashboard_state_with<Ensure, Devices, Capabilities, Layout, SaveLayout>(
    mut ensure_status: Ensure,
    mut request_devices: Devices,
    mut request_capabilities: Capabilities,
    mut request_layout: Layout,
    mut save_layout: SaveLayout,
) -> Result<DashboardStatePayload, String>
where
    Ensure: FnMut() -> BoxFutureResult<'static, DesktopDaemonStatus>,
    Devices: FnMut() -> BoxFutureResult<'static, Vec<DaemonDeviceSnapshot>>,
    Capabilities: FnMut() -> BoxFutureResult<'static, CapabilityRegistrySnapshot>,
    Layout: FnMut() -> BoxFutureResult<'static, LayoutGraph>,
    SaveLayout: FnMut(LayoutGraph) -> BoxFutureResult<'static, ()>,
{
    let daemon = ensure_status().await.map_err(|err| err.to_string())?;
    let mut status = daemon.status;
    status.started_by_desktop = daemon.auto_started;
    let devices = request_devices().await.unwrap_or_default();
    let capabilities = request_capabilities().await.ok();
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
        remembered.online_display_projection(
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
        capabilities,
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
    let local_display_count = visible_layout
        .and_then(|layout| layout.get_node(status.device_id))
        .map(|node| node.displays.len())
        .unwrap_or_default();
    let input_ready = status.input_mode.is_some()
        && matches!(status.backend_health, Some(BackendHealth::Healthy));
    let background_ready = status.background_owner == BackgroundProcessOwner::Daemon
        && status.background_mode == BackgroundRunMode::BackgroundProcess;
    let tray_owned_by_daemon = status.tray_owner == BackgroundProcessOwner::Daemon;
    let local_ready =
        background_ready && input_ready && layout_error.is_none() && local_display_count > 0;
    let dual_machine_ready = background_ready
        && input_ready
        && layout_error.is_none()
        && !devices.is_empty()
        && visible_layout_devices > 1;
    let next_step = if !background_ready {
        "后台服务未就绪，先启动守护进程"
    } else if !input_ready {
        "输入后端未就绪，先检查权限或后端降级"
    } else if local_ready && devices.is_empty() {
        "本机能力已就绪，可以进行本机设备监控；双机验收等待局域网发现"
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
        local_display_count,
        local_ready,
        input_ready,
        dual_machine_ready,
        next_step: next_step.to_string(),
    }
}

fn main() {
    eprintln!("{}", build_metadata());
    tauri::Builder::default()
        .plugin(init(|app, _args, _cwd| {
            // Focus the existing window when a second instance is launched
            let _ = show_main_window(app);
        }))
        .manage(LocalControlStreamState::default())
        .manage(EndpointEventStreamState::default())
        .on_menu_event(|app, event| {
            if let Some(action) = tray_action_from_menu_event(&event) {
                if let Err(err) = apply_tray_action(app, action) {
                    eprintln!("tray menu action failed: {err}");
                }
            }
        })
        .on_tray_icon_event(|app, event| {
            if event.id().as_ref() != TRAY_ICON_ID {
                return;
            }
            let action = tray_action_from_icon_event(&event);
            if let Err(err) = apply_tray_action(app, action) {
                eprintln!("tray icon action failed: {err}");
            }
        })
        .invoke_handler(tauri::generate_handler![
            dashboard_state,
            start_service,
            stop_service,
            capabilities_state,
            connect_device,
            disconnect_device,
            local_controls_state,
            mobile_access,
            capture_display,
            identify_displays,
            update_display_settings,
            open_display_settings,
            list_virtual_displays,
            create_virtual_display,
            remove_virtual_display,
            endpoint_events_state,
            inject_endpoint_event,
            start_local_controls_stream,
            stop_local_controls_stream,
            start_endpoint_events_stream,
            stop_endpoint_events_stream,
            run_local_input_test,
            run_remote_latency_test,
            minimize_window,
            toggle_maximize_window,
            close_window,
            start_drag_window,
            get_config,
            set_config,
            get_layout,
            set_layout,
            list_hardware_assets,
            import_hardware_asset,
            export_hardware_asset,
            show_tray,
            hide_to_tray,
            get_logs,
            clear_logs
        ])
        .setup(|app| {
            // Setup system tray
            setup_system_tray(app.handle())?;
            start_tray_status_refresh(app.handle().clone());

            // Handle window close event to minimize to tray
            let app_handle = app.handle().clone();
            app.get_webview_window("main")
                .unwrap()
                .on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        match close_request_action() {
                            CloseRequestAction::PreventAndHide => {
                                api.prevent_close();
                                let _ = hide_main_window(&app_handle);
                            }
                        }
                    }
                });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run Tauri desktop app");
}

fn main_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    app.get_webview_window("main")
        .ok_or_else(|| "Main window not found".to_string())
}

fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let window = main_window(app)?;
    let _ = window.unminimize();
    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())?;
    Ok(())
}

fn hide_main_window(app: &AppHandle) -> Result<(), String> {
    let window = main_window(app)?;
    window.hide().map_err(|err| err.to_string())
}

fn close_request_action() -> CloseRequestAction {
    CloseRequestAction::PreventAndHide
}

fn tray_action_from_menu_event(event: &tauri::menu::MenuEvent) -> Option<TrayAction> {
    let action = tray_action_from_menu_id(event.id().as_ref());
    (action != TrayAction::None).then_some(action)
}

fn tray_action_from_menu_id(id: &str) -> TrayAction {
    match id {
        TRAY_MENU_STATUS_ID => TrayAction::None,
        TRAY_MENU_SHOW_ID => TrayAction::ShowWindow,
        TRAY_MENU_HIDE_ID => TrayAction::HideWindow,
        TRAY_MENU_START_SERVICE_ID => TrayAction::StartService,
        TRAY_MENU_STOP_SERVICE_ID => TrayAction::StopService,
        TRAY_MENU_DISPLAY_SETTINGS_ID => TrayAction::OpenDisplaySettings,
        TRAY_MENU_OPEN_CONFIG_DIR_ID => TrayAction::OpenConfigDir,
        TRAY_MENU_OPEN_LOG_ID => TrayAction::OpenLogFile,
        TRAY_MENU_QUIT_ID => TrayAction::QuitApp,
        _ => TrayAction::None,
    }
}

fn tray_action_from_icon_event(event: &TrayIconEvent) -> TrayAction {
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
        | TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } => TrayAction::ShowWindow,
        _ => TrayAction::None,
    }
}

fn apply_tray_action(app: &AppHandle, action: TrayAction) -> Result<(), String> {
    match action {
        TrayAction::None => Ok(()),
        TrayAction::ShowWindow => show_main_window(app),
        TrayAction::HideWindow => hide_main_window(app),
        TrayAction::StartService => {
            start_daemon_from_tray(app);
            Ok(())
        }
        TrayAction::StopService => {
            stop_daemon_from_tray(app);
            Ok(())
        }
        TrayAction::OpenDisplaySettings => {
            if let Err(e) = rshare_platform::display::open_display_settings() {
                Err(format!("Failed to open display settings: {}", e))
            } else {
                Ok(())
            }
        }
        TrayAction::OpenConfigDir => rshare_platform::system::open_config_dir()
            .map_err(|err| format!("Failed to open config directory: {err}")),
        TrayAction::OpenLogFile => rshare_platform::system::open_log_file()
            .map_err(|err| format!("Failed to open log file: {err}")),
        TrayAction::QuitApp => {
            shutdown_daemon_and_exit(app);
            Ok(())
        }
    }
}

fn start_daemon_from_tray(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let config = Config::load().unwrap_or_default();
        if let Err(err) = daemon_client::spawn_daemon(
            Some(config.network.port),
            Some(&config.network.bind_address),
        )
        .await
        {
            eprintln!("tray failed to start daemon: {err}");
        }
        refresh_tray_status_once(&app).await;
    });
}

fn stop_daemon_from_tray(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = daemon_client::request_shutdown().await {
            eprintln!("tray failed to stop daemon: {err}");
        }
        refresh_tray_status_once(&app).await;
    });
}

fn shutdown_daemon_and_exit(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = shutdown_daemon_for_exit().await {
            eprintln!("daemon shutdown before desktop exit failed: {err}");
        }
        app.exit(0);
    });
}

async fn shutdown_daemon_for_exit() -> AnyhowResult<()> {
    let manager = Arc::new(rshare_core::service::ServiceManager::new()?);
    shutdown_daemon_for_exit_with(
        {
            let manager = Arc::clone(&manager);
            move || manager.is_running()
        },
        || Box::pin(async { daemon_client::request_shutdown().await }),
        {
            let manager = Arc::clone(&manager);
            move || {
                let manager = Arc::clone(&manager);
                Box::pin(async move { manager.stop().await })
            }
        },
        20,
        Duration::from_millis(200),
    )
    .await
}

async fn shutdown_daemon_for_exit_with<IsRunning, RequestShutdown, ForceStop>(
    mut is_running: IsRunning,
    mut request_shutdown: RequestShutdown,
    mut force_stop: ForceStop,
    wait_polls: usize,
    wait_interval: Duration,
) -> AnyhowResult<()>
where
    IsRunning: FnMut() -> bool,
    RequestShutdown: FnMut() -> BoxFutureResult<'static, ()>,
    ForceStop: FnMut() -> BoxFutureResult<'static, ()>,
{
    if !is_running() {
        return Ok(());
    }

    if let Err(err) = request_shutdown().await {
        eprintln!("graceful daemon shutdown request failed, forcing stop: {err}");
        force_stop().await?;
        return Ok(());
    }

    for _ in 0..wait_polls {
        tokio::time::sleep(wait_interval).await;
        if !is_running() {
            return Ok(());
        }
    }

    force_stop().await?;
    Ok(())
}

/// Setup system tray with icon and restore actions.
fn setup_system_tray(app: &AppHandle) -> tauri::Result<()> {
    let status_item = MenuItem::with_id(
        app,
        TRAY_MENU_STATUS_ID,
        "状态：后台检测中",
        false,
        None::<&str>,
    )?;
    let show_item = MenuItem::with_id(app, TRAY_MENU_SHOW_ID, "显示主窗口", true, None::<&str>)?;
    let hide_item = MenuItem::with_id(app, TRAY_MENU_HIDE_ID, "隐藏到托盘", true, None::<&str>)?;
    let service_separator = PredefinedMenuItem::separator(app)?;
    let start_service_item = MenuItem::with_id(
        app,
        TRAY_MENU_START_SERVICE_ID,
        "启动后台服务",
        true,
        None::<&str>,
    )?;
    let stop_service_item = MenuItem::with_id(
        app,
        TRAY_MENU_STOP_SERVICE_ID,
        "停止后台服务",
        false,
        None::<&str>,
    )?;
    let settings_separator = PredefinedMenuItem::separator(app)?;
    let display_settings_item = MenuItem::with_id(
        app,
        TRAY_MENU_DISPLAY_SETTINGS_ID,
        "显示设置...",
        true,
        None::<&str>,
    )?;
    let config_dir_item = MenuItem::with_id(
        app,
        TRAY_MENU_OPEN_CONFIG_DIR_ID,
        "打开配置目录",
        true,
        None::<&str>,
    )?;
    let log_item = MenuItem::with_id(app, TRAY_MENU_OPEN_LOG_ID, "打开日志", true, None::<&str>)?;
    let quit_separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_QUIT_ID, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status_item,
            &show_item,
            &hide_item,
            &service_separator,
            &start_service_item,
            &stop_service_item,
            &settings_separator,
            &display_settings_item,
            &config_dir_item,
            &log_item,
            &quit_separator,
            &quit_item,
        ],
    )?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".to_string()))?;

    let tray = TrayIconBuilder::with_id(TRAY_ICON_ID)
        .icon(icon)
        .tooltip("R-ShareMouse")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .build(app)?;

    app.manage(TrayRuntimeHandles {
        tray,
        _menu: menu,
        status_item,
        start_service_item,
        stop_service_item,
    });

    Ok(())
}

fn start_tray_status_refresh(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            refresh_tray_status_once(&app).await;
            tokio::time::sleep(Duration::from_millis(TRAY_STATUS_REFRESH_MS)).await;
        }
    });
}

async fn refresh_tray_status_once(app: &AppHandle) {
    let status = daemon_client::request_status().await.ok();
    apply_tray_status_view(app, tray_status_view(status.as_ref()));
}

fn apply_tray_status_view(app: &AppHandle, view: TrayStatusView) {
    let handles = app.state::<TrayRuntimeHandles>();
    let _ = handles.status_item.set_text(&view.status_text);
    let _ = handles
        .start_service_item
        .set_enabled(!view.service_running);
    let _ = handles.stop_service_item.set_enabled(view.service_running);
    let _ = handles.tray.set_tooltip(Some(&view.tooltip));
}

fn tray_status_view(status: Option<&ServiceStatusSnapshot>) -> TrayStatusView {
    match status {
        Some(status) => {
            let backend = match &status.backend_health {
                Some(BackendHealth::Healthy) => "输入正常",
                Some(BackendHealth::Degraded { .. }) => "输入受限",
                None => "输入未初始化",
            };
            let status_text = format!(
                "状态：后台运行中 · 发现 {} · 已连接 {}",
                status.discovered_devices, status.connected_devices
            );
            let tooltip = format!(
                "R-ShareMouse ({})\n{}\n{}",
                rshare_platform::system::platform_name(),
                status.bind_address,
                backend
            );

            TrayStatusView {
                status_text,
                tooltip,
                service_running: true,
            }
        }
        None => TrayStatusView {
            status_text: "状态：后台未运行".to_string(),
            tooltip: format!(
                "R-ShareMouse ({})\n后台服务未运行",
                rshare_platform::system::platform_name()
            ),
            service_running: false,
        },
    }
}

/// Hide window to tray
#[tauri::command]
async fn hide_to_tray(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}

/// Log entry structure
#[derive(Debug, Clone, Serialize)]
struct LogEntry {
    timestamp: String,
    level: String,
    target: String,
    message: String,
}

/// Get the log file path
fn get_log_file_path() -> PathBuf {
    rshare_platform::system::log_file_path()
        .unwrap_or_else(|_| PathBuf::from(".").join("rshare-daemon.log"))
}

/// Parse a single log line
fn parse_log_line(line: &str) -> Option<LogEntry> {
    // Parse tracing default format: TIMESTAMP LEVEL target: message
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    // Skip non-log lines
    if !line.contains(' ') {
        return None;
    }

    // Find the timestamp (ends with 'Z' or contains timezone)
    let parts: Vec<&str> = line.splitn(4, ' ').collect();
    if parts.len() < 4 {
        return None;
    }

    // Format: "2024-04-20T10:30:45.123456Z INFO rshare_core: message"
    let timestamp = parts[0].to_string();
    let level = parts[1].to_string();
    let rest = &line[parts[0].len() + parts[1].len() + 2..];

    // Find the colon that separates target from message
    if let Some(colon_pos) = rest.find(':') {
        let target = rest[..colon_pos].trim().to_string();
        let message = rest[colon_pos + 1..].trim().to_string();
        return Some(LogEntry {
            timestamp,
            level,
            target,
            message,
        });
    }

    None
}

/// Get logs from the daemon log file
#[tauri::command]
async fn get_logs(limit: Option<usize>) -> Result<Vec<LogEntry>, String> {
    let log_file = get_log_file_path();
    let limit = limit.unwrap_or(1000);

    if !log_file.exists() {
        return Ok(Vec::new());
    }

    let content =
        std::fs::read_to_string(&log_file).map_err(|e| format!("读取日志文件失败: {}", e))?;

    let entries: Vec<LogEntry> = content
        .lines()
        .rev()
        .filter_map(|line| parse_log_line(line))
        .take(limit)
        .collect();

    Ok(entries)
}

/// Clear the daemon log file
#[tauri::command]
async fn clear_logs() -> Result<(), String> {
    let log_file = get_log_file_path();
    if let Some(parent) = log_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建日志目录失败: {}", e))?;
    }
    std::fs::write(&log_file, "").map_err(|e| format!("清空日志失败: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::time::Duration;

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
            network: rshare_core::NetworkTransportSnapshot::default(),
            latency_feedback: rshare_core::LatencyFeedbackSnapshot::default(),
        }
    }

    fn sample_layout(local_id: DeviceId) -> LayoutGraph {
        let mut layout = LayoutGraph::new(local_id);
        layout.add_node(rshare_core::LayoutNode::new(local_id, 0, 0, 1920, 1080));
        layout
    }

    fn sample_multi_display_layout(local_id: DeviceId) -> LayoutGraph {
        let mut layout = LayoutGraph::new(local_id);
        let mut node = rshare_core::LayoutNode::new(local_id, 0, 0, 2560, 1440);
        node.displays.push(rshare_core::DisplayNode {
            display_id: "display-2".to_string(),
            x: 2560,
            y: 0,
            width: 2560,
            height: 1440,
            primary: false,
            scale_percent: None,
            dpi_x: None,
            dpi_y: None,
        });
        layout.add_node(node);
        layout
    }

    fn empty_capabilities() -> BoxFutureResult<'static, CapabilityRegistrySnapshot> {
        Box::pin(async {
            Ok(CapabilityRegistrySnapshot {
                local_device_id: DeviceId::nil(),
                generated_at_ms: 1,
                devices: Vec::new(),
            })
        })
    }

    fn sample_tray_click(button: MouseButton, button_state: MouseButtonState) -> TrayIconEvent {
        TrayIconEvent::Click {
            id: tauri::tray::TrayIconId::new(TRAY_ICON_ID),
            position: tauri::PhysicalPosition::new(0.0, 0.0),
            rect: tauri::Rect {
                position: tauri::Position::Physical(tauri::PhysicalPosition::new(0, 0)),
                size: tauri::Size::Physical(tauri::PhysicalSize::new(16, 16)),
            },
            button,
            button_state,
        }
    }

    #[test]
    fn local_input_test_kind_parser_accepts_ui_keys() {
        assert_eq!(
            local_input_test_kind_from_str("keyboard").unwrap(),
            LocalInputTestKind::KeyboardShift
        );
        assert_eq!(
            local_input_test_kind_from_str("keyboard_shift").unwrap(),
            LocalInputTestKind::KeyboardShift
        );
        assert_eq!(
            local_input_test_kind_from_str("mouse").unwrap(),
            LocalInputTestKind::MouseMove
        );
        assert_eq!(
            local_input_test_kind_from_str("mouse_move").unwrap(),
            LocalInputTestKind::MouseMove
        );
        assert!(local_input_test_kind_from_str("unknown").is_err());
    }

    #[test]
    fn close_request_is_intercepted_for_hide_to_tray_behavior() {
        assert_eq!(close_request_action(), CloseRequestAction::PreventAndHide);
    }

    #[test]
    fn tray_menu_ids_map_to_restore_hide_and_quit_actions() {
        assert_eq!(
            tray_action_from_menu_id(TRAY_MENU_STATUS_ID),
            TrayAction::None
        );
        assert_eq!(
            tray_action_from_menu_id(TRAY_MENU_SHOW_ID),
            TrayAction::ShowWindow
        );
        assert_eq!(
            tray_action_from_menu_id(TRAY_MENU_HIDE_ID),
            TrayAction::HideWindow
        );
        assert_eq!(
            tray_action_from_menu_id(TRAY_MENU_START_SERVICE_ID),
            TrayAction::StartService
        );
        assert_eq!(
            tray_action_from_menu_id(TRAY_MENU_STOP_SERVICE_ID),
            TrayAction::StopService
        );
        assert_eq!(
            tray_action_from_menu_id(TRAY_MENU_DISPLAY_SETTINGS_ID),
            TrayAction::OpenDisplaySettings
        );
        assert_eq!(
            tray_action_from_menu_id(TRAY_MENU_OPEN_CONFIG_DIR_ID),
            TrayAction::OpenConfigDir
        );
        assert_eq!(
            tray_action_from_menu_id(TRAY_MENU_OPEN_LOG_ID),
            TrayAction::OpenLogFile
        );
        assert_eq!(
            tray_action_from_menu_id(TRAY_MENU_QUIT_ID),
            TrayAction::QuitApp
        );
        assert_eq!(tray_action_from_menu_id("unknown"), TrayAction::None);
    }

    #[test]
    fn tray_status_view_marks_service_actions_from_daemon_status() {
        let mut status = sample_status();
        status.discovered_devices = 2;
        status.connected_devices = 1;
        status.backend_health = Some(BackendHealth::Healthy);

        let running = tray_status_view(Some(&status));
        assert!(running.service_running);
        assert!(running.status_text.contains("发现 2"));
        assert!(running.status_text.contains("已连接 1"));
        assert!(running.tooltip.contains("输入正常"));

        let stopped = tray_status_view(None);
        assert!(!stopped.service_running);
        assert!(stopped.status_text.contains("后台未运行"));
    }

    #[test]
    fn left_click_release_restores_main_window_from_tray() {
        assert_eq!(
            tray_action_from_icon_event(&sample_tray_click(
                MouseButton::Left,
                MouseButtonState::Up,
            )),
            TrayAction::ShowWindow
        );
        assert_eq!(
            tray_action_from_icon_event(&sample_tray_click(
                MouseButton::Right,
                MouseButtonState::Up,
            )),
            TrayAction::None
        );
    }

    #[tokio::test]
    async fn virtual_display_list_falls_back_to_platform_when_daemon_ipc_is_unavailable() {
        let platform_calls = Arc::new(AtomicUsize::new(0));

        let result = list_virtual_displays_with(
            || {
                Box::pin(async {
                    Err(anyhow!(std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "daemon offline",
                    )))
                })
            },
            {
                let platform_calls = Arc::clone(&platform_calls);
                move || {
                    platform_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![VirtualDisplaySnapshot {
                        id: "rshare-vdisplay-1".to_string(),
                        width: 1920,
                        height: 1080,
                        refresh_rate_millihz: Some(60_000),
                        name: Some("R-ShareMouse Virtual Display".to_string()),
                        status: rshare_core::VirtualDisplayStatus::Active,
                        display_id: Some("windows-display-rshare".to_string()),
                        message: None,
                    }])
                }
            },
        )
        .await
        .expect("platform fallback should return virtual display snapshots");

        assert_eq!(platform_calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].display_id.as_deref(),
            Some("windows-display-rshare")
        );
    }

    #[tokio::test]
    async fn local_controls_state_falls_back_to_platform_display_when_daemon_ipc_is_unavailable() {
        let platform_calls = Arc::new(AtomicUsize::new(0));

        let result = local_controls_state_with(
            || {
                Box::pin(async {
                    Err(anyhow!(std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "daemon offline",
                    )))
                })
            },
            {
                let platform_calls = Arc::clone(&platform_calls);
                move || {
                    platform_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(rshare_core::LocalDisplayState {
                        display_count: 1,
                        primary_width: 1920,
                        primary_height: 1080,
                        displays: vec![rshare_core::LocalDisplayInfo {
                            display_id: "windows-display-rshare".to_string(),
                            width: 1920,
                            height: 1080,
                            active: true,
                            ..rshare_core::LocalDisplayInfo::default()
                        }],
                        ..rshare_core::LocalDisplayState::default()
                    })
                }
            },
        )
        .await
        .expect("platform display fallback should return a local controls snapshot");

        assert_eq!(platform_calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.display.display_count, 1);
        assert_eq!(
            result.display.displays[0].display_id.as_str(),
            "windows-display-rshare"
        );
    }

    #[tokio::test]
    async fn display_capture_falls_back_to_platform_when_daemon_ipc_is_unavailable() {
        let platform_calls = Arc::new(AtomicUsize::new(0));
        let request = DisplayCaptureRequest {
            display_id: "windows-display-rshare".to_string(),
            max_width: Some(640),
        };

        let result = capture_display_with(
            request.clone(),
            |_| {
                Box::pin(async {
                    Err(anyhow!(std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "daemon offline",
                    )))
                })
            },
            {
                let platform_calls = Arc::clone(&platform_calls);
                move |platform_request| {
                    platform_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(DisplayCaptureResult {
                        status: rshare_core::DisplayOperationStatus::Success,
                        display_id: platform_request.display_id,
                        mime_type: Some("image/png".to_string()),
                        width: platform_request.max_width,
                        height: Some(360),
                        bytes: vec![1, 2, 3],
                        message: None,
                    })
                }
            },
        )
        .await
        .expect("platform fallback should return display capture");

        assert_eq!(platform_calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.status, rshare_core::DisplayOperationStatus::Success);
        assert_eq!(result.display_id, request.display_id);
        assert_eq!(result.width, request.max_width);
    }

    #[tokio::test]
    async fn display_identify_falls_back_to_platform_when_daemon_ipc_is_unavailable() {
        let platform_calls = Arc::new(AtomicUsize::new(0));
        let request = DisplayIdentifyRequest {
            duration_ms: Some(750),
        };

        let result = identify_displays_with(
            request,
            |_| {
                Box::pin(async {
                    Err(anyhow!(std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "daemon offline",
                    )))
                })
            },
            {
                let platform_calls = Arc::clone(&platform_calls);
                move |platform_request| {
                    platform_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(DisplayIdentifyResult {
                        status: rshare_core::DisplayOperationStatus::Success,
                        message: platform_request
                            .duration_ms
                            .map(|duration| format!("identified for {duration}ms")),
                    })
                }
            },
        )
        .await
        .expect("platform fallback should return identify result");

        assert_eq!(platform_calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.status, rshare_core::DisplayOperationStatus::Success);
        assert_eq!(result.message.as_deref(), Some("identified for 750ms"));
    }

    #[tokio::test]
    async fn display_update_falls_back_to_platform_when_daemon_ipc_is_unavailable() {
        let platform_calls = Arc::new(AtomicUsize::new(0));
        let request = DisplaySettingsUpdateRequest {
            display_id: "windows-display-rshare".to_string(),
            width: Some(1920),
            height: Some(1080),
            refresh_rate_millihz: Some(60_000),
            orientation: None,
            primary: None,
            x: Some(0),
            y: Some(0),
            scale_percent: None,
        };

        let result = update_display_settings_with(
            request,
            |_| {
                Box::pin(async {
                    Err(anyhow!(std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "daemon offline",
                    )))
                })
            },
            {
                let platform_calls = Arc::clone(&platform_calls);
                move |platform_request| {
                    platform_calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(platform_request.display_id, "windows-display-rshare");
                    Ok(DisplaySettingsUpdateResult {
                        status: rshare_core::DisplayOperationStatus::Success,
                        message: None,
                    })
                }
            },
        )
        .await
        .expect("platform fallback should return display update result");

        assert_eq!(platform_calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.status, rshare_core::DisplayOperationStatus::Success);
    }

    #[tokio::test]
    async fn virtual_display_create_falls_back_to_platform_when_daemon_ipc_is_unavailable() {
        let platform_calls = Arc::new(AtomicUsize::new(0));
        let request = VirtualDisplayCreateRequest {
            id: None,
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        };

        let result = create_virtual_display_with(
            request.clone(),
            |_| {
                Box::pin(async {
                    Err(anyhow!(std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "daemon offline",
                    )))
                })
            },
            {
                let platform_calls = Arc::clone(&platform_calls);
                move |platform_request| {
                    platform_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(VirtualDisplayOperationResult {
                        status: rshare_core::VirtualDisplayOperationStatus::Created,
                        display: Some(VirtualDisplaySnapshot {
                            id: platform_request
                                .id
                                .unwrap_or_else(|| "rshare-vdisplay-1".to_string()),
                            width: platform_request.width,
                            height: platform_request.height,
                            refresh_rate_millihz: platform_request.refresh_rate_millihz,
                            name: platform_request.name,
                            status: rshare_core::VirtualDisplayStatus::Pending,
                            display_id: None,
                            message: Some("waiting for IddCx arrival".to_string()),
                        }),
                        message: None,
                    })
                }
            },
        )
        .await
        .expect("platform fallback should return create operation status");

        assert_eq!(platform_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            result.status,
            rshare_core::VirtualDisplayOperationStatus::Created
        );
        assert_eq!(
            result.display.as_ref().map(|display| display.width),
            Some(request.width)
        );
    }

    #[tokio::test]
    async fn virtual_display_remove_falls_back_to_platform_when_daemon_ipc_is_unavailable() {
        let platform_calls = Arc::new(AtomicUsize::new(0));
        let request = VirtualDisplayRemoveRequest {
            id: "rshare-vdisplay-1".to_string(),
        };

        let result = remove_virtual_display_with(
            request.clone(),
            |_| {
                Box::pin(async {
                    Err(anyhow!(std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "daemon offline",
                    )))
                })
            },
            {
                let platform_calls = Arc::clone(&platform_calls);
                move |platform_request| {
                    platform_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(VirtualDisplayOperationResult {
                        status: rshare_core::VirtualDisplayOperationStatus::Removed,
                        display: Some(VirtualDisplaySnapshot {
                            id: platform_request.id,
                            width: 1920,
                            height: 1080,
                            refresh_rate_millihz: Some(60_000),
                            name: None,
                            status: rshare_core::VirtualDisplayStatus::Removed,
                            display_id: None,
                            message: None,
                        }),
                        message: None,
                    })
                }
            },
        )
        .await
        .expect("platform fallback should return remove operation status");

        assert_eq!(platform_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            result.status,
            rshare_core::VirtualDisplayOperationStatus::Removed
        );
        assert_eq!(
            result.display.as_ref().map(|display| display.id.as_str()),
            Some(request.id.as_str())
        );
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
            empty_capabilities,
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
    async fn dashboard_state_includes_capability_registry_when_available() {
        let local_id = DeviceId::new_v4();
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
            || Box::pin(async { Ok(Vec::new()) }),
            move || {
                Box::pin(async move {
                    Ok(CapabilityRegistrySnapshot {
                        local_device_id: local_id,
                        generated_at_ms: 123,
                        devices: Vec::new(),
                    })
                })
            },
            move || Box::pin(async move { Ok(sample_layout(local_id)) }),
            |_| Box::pin(async { Ok(()) }),
        )
        .await
        .expect("dashboard should include capability registry");

        let capabilities = result.capabilities.expect("capability registry");
        assert_eq!(capabilities.local_device_id, local_id);
        assert_eq!(capabilities.generated_at_ms, 123);
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
            empty_capabilities,
            move || Box::pin(async move { Ok(sample_layout(local_id)) }),
            |_| Box::pin(async { Ok(()) }),
        )
        .await
        .expect("dashboard should expose acceptance readiness");

        assert!(result.acceptance.input_ready);
        assert_eq!(result.acceptance.discovered_devices, 1);
        assert_eq!(result.acceptance.visible_layout_devices, 2);
        assert!(result.acceptance.dual_machine_ready);
        assert_eq!(
            result.acceptance.next_step,
            "打开另一台机器并连接设备，开始边缘切换验收"
        );
    }

    #[tokio::test]
    async fn dashboard_state_reports_local_acceptance_without_remote_devices() {
        let local_id = DeviceId::new_v4();

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
            || Box::pin(async { Ok(Vec::new()) }),
            empty_capabilities,
            move || Box::pin(async move { Ok(sample_multi_display_layout(local_id)) }),
            |_| Box::pin(async { Ok(()) }),
        )
        .await
        .expect("dashboard should expose local acceptance");

        assert!(result.acceptance.input_ready);
        assert!(result.acceptance.local_ready);
        assert_eq!(result.acceptance.local_display_count, 2);
        assert_eq!(result.acceptance.visible_layout_devices, 1);
        assert!(!result.acceptance.dual_machine_ready);
        assert_eq!(
            result.acceptance.next_step,
            "本机能力已就绪，可以进行本机设备监控；双机验收等待局域网发现"
        );
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
            empty_capabilities,
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
            empty_capabilities,
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
            empty_capabilities,
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
            empty_capabilities,
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
            3840
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
            empty_capabilities,
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

    #[tokio::test]
    async fn desktop_exit_requests_graceful_daemon_shutdown_when_running() {
        let request_count = Arc::new(AtomicUsize::new(0));
        let force_stop_count = Arc::new(AtomicUsize::new(0));
        let running_checks = Arc::new(AtomicUsize::new(0));

        shutdown_daemon_for_exit_with(
            {
                let running_checks = Arc::clone(&running_checks);
                move || running_checks.fetch_add(1, Ordering::SeqCst) == 0
            },
            {
                let request_count = Arc::clone(&request_count);
                move || {
                    Box::pin({
                        let request_count = Arc::clone(&request_count);
                        async move {
                            request_count.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    })
                }
            },
            {
                let force_stop_count = Arc::clone(&force_stop_count);
                move || {
                    Box::pin({
                        let force_stop_count = Arc::clone(&force_stop_count);
                        async move {
                            force_stop_count.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    })
                }
            },
            1,
            Duration::from_millis(1),
        )
        .await
        .expect("desktop exit should request daemon shutdown");

        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(force_stop_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn desktop_exit_falls_back_to_force_stop_when_shutdown_request_fails() {
        let request_count = Arc::new(AtomicUsize::new(0));
        let force_stop_count = Arc::new(AtomicUsize::new(0));

        shutdown_daemon_for_exit_with(
            || true,
            {
                let request_count = Arc::clone(&request_count);
                move || {
                    Box::pin({
                        let request_count = Arc::clone(&request_count);
                        async move {
                            request_count.fetch_add(1, Ordering::SeqCst);
                            Err(anyhow!("ipc shutdown failed"))
                        }
                    })
                }
            },
            {
                let force_stop_count = Arc::clone(&force_stop_count);
                move || {
                    Box::pin({
                        let force_stop_count = Arc::clone(&force_stop_count);
                        async move {
                            force_stop_count.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    })
                }
            },
            1,
            Duration::from_millis(1),
        )
        .await
        .expect("desktop exit should fall back to force stop");

        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(force_stop_count.load(Ordering::SeqCst), 1);
    }
}
