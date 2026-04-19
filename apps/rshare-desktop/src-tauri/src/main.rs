#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rshare_core::{
    daemon_client, Config, DaemonDeviceSnapshot, DeviceId, LayoutGraph, ServiceStatusSnapshot,
};
use serde::Serialize;
use tauri::WebviewWindow;

#[derive(Debug, Clone, Serialize)]
struct DashboardStatePayload {
    status: Option<ServiceStatusSnapshot>,
    devices: Vec<DaemonDeviceSnapshot>,
}

#[tauri::command]
async fn dashboard_state() -> Result<DashboardStatePayload, String> {
    let status = daemon_client::request_status().await.ok();
    let devices = if status.is_some() {
        daemon_client::request_devices().await.unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(DashboardStatePayload { status, devices })
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
