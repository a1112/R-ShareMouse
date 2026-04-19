//! Main application state

use eframe::egui;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

use rshare_core::{config::Config, DaemonDeviceSnapshot, ServiceStatusSnapshot};

use crate::dashboard::DashboardSummary;
use crate::tray;
use crate::ui::{
    layout_view::LayoutView,
    main_view::{DashboardAction, MainView},
    settings_view::SettingsView,
};

/// Device information for UI display
#[derive(Debug, Clone)]
pub struct UiDevice {
    pub id: Uuid,
    pub name: String,
    pub address: String,
    pub connected: bool,
    pub last_seen_secs: Option<u64>,
}

impl From<DaemonDeviceSnapshot> for UiDevice {
    fn from(value: DaemonDeviceSnapshot) -> Self {
        Self {
            id: value.id,
            name: value.name,
            address: value
                .addresses
                .first()
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
            connected: value.connected,
            last_seen_secs: value.last_seen_secs,
        }
    }
}

/// Active tab/view in the application
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Main,
    Devices,
    Layout,
    Settings,
    About,
}

/// Main application state
pub struct RShareApp {
    active_tab: ActiveTab,
    devices: Vec<UiDevice>,
    device_snapshots: Vec<DaemonDeviceSnapshot>,
    status_snapshot: Option<ServiceStatusSnapshot>,
    main_view: MainView,
    settings_view: SettingsView,
    layout_view: LayoutView,
    show_confirmation: bool,
    confirmation_message: String,
    confirmation_action: Option<Box<dyn FnOnce(&mut Self) + Send>>,
    runtime: tokio::runtime::Runtime,
    tray_event_rx: Option<std::sync::mpsc::Receiver<tray::TrayEvent>>,
    window_visible: bool,
    config: Config,
    tray_manager: Option<tray::TrayManager>,
    activity_log: Vec<String>,
    last_refresh: Instant,
}

impl RShareApp {
    /// Create a new application instance
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let config = Config::load().unwrap_or_default();
        let settings_view = SettingsView::from_config(&config);
        let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

        let wake_ctx = cc.egui_ctx.clone();
        let wake_ui: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            wake_ctx.request_repaint();
        });
        let mut tray_manager = tray::TrayManager::new_with_waker(Some(wake_ui)).ok();
        let tray_event_rx = tray_manager.as_mut().map(|tm| tm.events());

        let mut app = Self {
            active_tab: ActiveTab::Main,
            devices: Vec::new(),
            device_snapshots: Vec::new(),
            status_snapshot: None,
            main_view: MainView::new(),
            settings_view,
            layout_view: LayoutView::new(),
            show_confirmation: false,
            confirmation_message: String::new(),
            confirmation_action: None,
            runtime,
            tray_event_rx,
            window_visible: true,
            config,
            tray_manager,
            activity_log: vec!["Application started".to_string()],
            last_refresh: Instant::now() - Duration::from_secs(5),
        };

        app.refresh_daemon_state();
        app
    }

    fn service_running(&self) -> bool {
        self.status_snapshot.is_some()
    }

    fn push_activity(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.activity_log.insert(0, message);
        self.activity_log.truncate(20);
    }

    fn update_tray(&mut self) {
        let running = self.service_running();
        if let Some(ref mut tm) = self.tray_manager {
            tm.set_service_running(running);
            tm.set_tooltip(if running {
                "R-ShareMouse - Service Running"
            } else {
                "R-ShareMouse - Service Stopped"
            });
        }
    }

    fn sync_layout_devices(&mut self) {
        self.layout_view.sync_devices(&self.devices);
    }

    fn refresh_daemon_state(&mut self) {
        let previous_running = self.service_running();
        let status = self.runtime.block_on(rshare_core::daemon_client::request_status()).ok();
        let devices = if status.is_some() {
            self.runtime
                .block_on(rshare_core::daemon_client::request_devices())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        self.status_snapshot = status;
        self.device_snapshots = devices;
        self.devices = self
            .device_snapshots
            .clone()
            .into_iter()
            .map(UiDevice::from)
            .collect();
        self.sync_layout_devices();
        self.update_tray();

        let running = self.service_running();
        if running && !previous_running {
            self.push_activity("Daemon connected");
        } else if !running && previous_running {
            self.push_activity("Daemon stopped");
        }
    }

    fn start_service(&mut self) {
        match self.runtime.block_on(rshare_core::daemon_client::spawn_daemon(
            Some(self.config.network.port),
            Some(&self.config.network.bind_address),
        )) {
            Ok(status) => {
                self.status_snapshot = Some(status);
                self.push_activity("Service started");
                self.refresh_daemon_state();
            }
            Err(err) => {
                log::error!("Failed to start service: {}", err);
                self.push_activity(format!("Failed to start service: {}", err));
            }
        }
    }

    fn stop_service(&mut self) {
        match self
            .runtime
            .block_on(rshare_core::daemon_client::request_shutdown())
        {
            Ok(_) => {
                self.push_activity("Service stopped");
            }
            Err(err) => {
                log::error!("Failed to stop service: {}", err);
                self.push_activity(format!("Failed to stop service: {}", err));
            }
        }
        self.refresh_daemon_state();
    }

    fn connect_device(&mut self, id: Uuid) {
        match self
            .runtime
            .block_on(rshare_core::daemon_client::request_connect(id))
        {
            Ok(_) => self.push_activity(format!("Connect requested for {}", id)),
            Err(err) => {
                log::error!("Failed to connect {}: {}", id, err);
                self.push_activity(format!("Connect failed for {}: {}", id, err));
            }
        }
        self.refresh_daemon_state();
    }

    fn disconnect_device(&mut self, id: Uuid) {
        match self
            .runtime
            .block_on(rshare_core::daemon_client::request_disconnect(id))
        {
            Ok(_) => self.push_activity(format!("Disconnect requested for {}", id)),
            Err(err) => {
                log::error!("Failed to disconnect {}: {}", id, err);
                self.push_activity(format!("Disconnect failed for {}: {}", id, err));
            }
        }
        self.refresh_daemon_state();
    }

    fn confirm(
        &mut self,
        message: impl Into<String>,
        action: impl FnOnce(&mut Self) + Send + 'static,
    ) {
        self.confirmation_message = message.into();
        self.confirmation_action = Some(Box::new(action));
        self.show_confirmation = true;
    }

    fn handle_dashboard_action(&mut self, action: DashboardAction) {
        match action {
            DashboardAction::OpenDevices => self.active_tab = ActiveTab::Devices,
            DashboardAction::OpenLayout => self.active_tab = ActiveTab::Layout,
            DashboardAction::OpenSettings => self.active_tab = ActiveTab::Settings,
            DashboardAction::StartService => self.start_service(),
            DashboardAction::StopService => {
                self.confirm("Stop the R-ShareMouse service?", |app| app.stop_service());
            }
        }
    }
}

impl eframe::App for RShareApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.config.gui.minimize_to_tray {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.window_visible = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        if self.last_refresh.elapsed() >= Duration::from_secs(1) {
            self.refresh_daemon_state();
            self.last_refresh = Instant::now();
        }

        let tray_events: Vec<tray::TrayEvent> = if let Some(ref rx) = self.tray_event_rx {
            let mut events = Vec::new();
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
            events
        } else {
            Vec::new()
        };

        for event in tray_events {
            match event {
                tray::TrayEvent::Show => {
                    self.window_visible = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                }
                tray::TrayEvent::Hide => {
                    self.window_visible = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                tray::TrayEvent::ToggleService => {
                    if self.service_running() {
                        self.stop_service();
                    } else {
                        self.start_service();
                    }
                }
                tray::TrayEvent::Quit => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Settings").clicked() {
                        self.active_tab = ActiveTab::Settings;
                        ui.close_menu();
                    }
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("View", |ui| {
                    if ui.button("Main").clicked() {
                        self.active_tab = ActiveTab::Main;
                        ui.close_menu();
                    }
                    if ui.button("Devices").clicked() {
                        self.active_tab = ActiveTab::Devices;
                        ui.close_menu();
                    }
                    if ui.button("Layout").clicked() {
                        self.active_tab = ActiveTab::Layout;
                        ui.close_menu();
                    }
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        self.active_tab = ActiveTab::About;
                        ui.close_menu();
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let summary =
                        DashboardSummary::from_snapshots(self.status_snapshot.as_ref(), &self.device_snapshots);
                    let status_color = if summary.service_running {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::GRAY
                    };
                    ui.colored_label(status_color, &summary.service_label);

                    if ui
                        .button(if summary.service_running { "Stop" } else { "Start" })
                        .clicked()
                    {
                        if summary.service_running {
                            self.confirm("Stop the R-ShareMouse service?", |app| app.stop_service());
                        } else {
                            self.start_service();
                        }
                    }
                });
            });
        });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(12, 16, 24))
                    .inner_margin(egui::Margin::same(20.0)),
            )
            .show(ctx, |ui| match self.active_tab {
            ActiveTab::Main => {
                let summary =
                    DashboardSummary::from_snapshots(self.status_snapshot.as_ref(), &self.device_snapshots);
                if let Some(action) =
                    self.main_view
                        .show(ui, ctx, &summary, &self.devices, &self.activity_log)
                {
                    self.handle_dashboard_action(action);
                }
            }
            ActiveTab::Devices => self.show_devices_tab(ui),
            ActiveTab::Layout => self.layout_view.show(ui, ctx),
            ActiveTab::Settings => self.settings_view.show(ui, ctx),
            ActiveTab::About => self.show_about_tab(ui),
        });

        if self.show_confirmation {
            egui::Window::new("Confirm")
                .collapsible(false)
                .resizable(false)
                .pivot(egui::Align2::CENTER_CENTER)
                .fixed_pos(egui::pos2(400.0, 300.0))
                .show(ctx, |ui| {
                    ui.label(&self.confirmation_message);
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        if ui.button("Yes").clicked() {
                            if let Some(action) = self.confirmation_action.take() {
                                action(self);
                            }
                            self.show_confirmation = false;
                        }
                        if ui.button("No").clicked() {
                            self.confirmation_action = None;
                            self.show_confirmation = false;
                        }
                    });
                });
        }

        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        if self.settings_view.is_modified() {
            self.config = self.settings_view.to_config();
            if let Err(e) = self.config.save() {
                log::error!("Failed to save config: {}", e);
            } else {
                log::info!("Configuration saved successfully");
                self.settings_view.mark_saved();
            }
        }
    }
}

impl RShareApp {
    fn show_devices_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Discovered Devices");
        ui.add_space(10.0);

        if self.devices.is_empty() {
            ui.label("No devices found. Start the service and wait for discovery.");
            return;
        }

        egui::Grid::new("devices_grid")
            .striped(true)
            .spacing([10.0, 5.0])
            .show(ui, |ui| {
                ui.strong("Name");
                ui.strong("Address");
                ui.strong("Status");
                ui.strong("Action");
                ui.end_row();

                let devices = self.devices.clone();
                for device in devices {
                    ui.label(&device.name);
                    ui.label(&device.address);
                    let last_seen = device
                        .last_seen_secs
                        .map(|secs| format!("{}s ago", secs))
                        .unwrap_or_else(|| "unknown".to_string());

                    if device.connected {
                        ui.colored_label(egui::Color32::GREEN, format!("● Connected ({})", last_seen));
                        if ui.button(format!("Disconnect {}", device.id)).clicked() {
                            self.disconnect_device(device.id);
                        }
                    } else {
                        ui.colored_label(egui::Color32::GRAY, format!("○ Discovered ({})", last_seen));
                        if ui.button(format!("Connect {}", device.id)).clicked() {
                            self.connect_device(device.id);
                        }
                    }
                    ui.end_row();
                }
            });

        ui.add_space(20.0);
        ui.heading("Network Status");
        ui.add_space(10.0);

        if let Some(status) = &self.status_snapshot {
            ui.colored_label(egui::Color32::GREEN, "● Network service is running");
            ui.label(format!("• Listening for connections: {}", status.bind_address));
            ui.label(format!("• Device discovery: UDP port {}", status.discovery_port));
            ui.label(format!("• Discovered {} device(s)", status.discovered_devices));
            ui.label(format!("• Connected {} device(s)", status.connected_devices));
        } else {
            ui.colored_label(egui::Color32::GRAY, "○ Network service is stopped");
            ui.label("Start the service to enable device discovery and connections.");
        }
    }

    fn show_about_tab(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.heading("R-ShareMouse");
            ui.add_space(10.0);
            ui.label("Version 0.1.0");
            ui.add_space(20.0);

            ui.label("A cross-platform mouse and keyboard sharing solution");
            ui.label("written in Rust, inspired by ShareMouse.");
            ui.add_space(20.0);

            ui.hyperlink_to("GitHub", "https://github.com/yourusername/R-ShareMouse");
            ui.label("License: MIT");

            ui.add_space(20.0);
            ui.label("Features:");
            ui.label("• Seamless mouse and keyboard sharing across computers");
            ui.label("• Clipboard synchronization");
            ui.label("• Automatic device discovery");
            ui.label("• Encrypted communication");
        });
    }
}
