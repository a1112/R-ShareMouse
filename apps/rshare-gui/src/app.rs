//! Main application state

use eframe::egui;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use rshare_core::config::Config;

use crate::ui::{layout_view::LayoutView, main_view::MainView, settings_view::SettingsView};

use crate::tray;

/// Device information for UI display
#[derive(Debug, Clone)]
pub struct UiDevice {
    pub id: Uuid,
    pub name: String,
    pub address: String,
    pub online: bool,
    pub connected: bool,
    pub latency: u32,
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

#[derive(Debug)]
enum NetworkCommand {
    Connect(Uuid),
    Disconnect(Uuid),
    Stop,
}

/// Main application state
pub struct RShareApp {
    /// Currently active tab
    active_tab: ActiveTab,

    /// Service state
    service_running: Arc<Mutex<bool>>,

    /// Discovered and connected devices
    devices: Arc<Mutex<Vec<UiDevice>>>,

    /// Main view state
    main_view: MainView,

    /// Settings view state
    settings_view: SettingsView,

    /// Layout view state
    layout_view: LayoutView,

    /// Show confirmation dialog
    show_confirmation: bool,

    /// Confirmation message
    confirmation_message: String,

    /// Confirmation callback
    confirmation_action: Option<Box<dyn FnOnce(&mut Self) + Send>>,

    /// Network event receiver
    network_event_rx: mpsc::Receiver<rshare_net::NetworkEvent>,

    /// Network event sender (for spawning NetworkManager)
    network_event_tx: mpsc::Sender<rshare_net::NetworkEvent>,

    /// Network command sender (for controlling the running NetworkManager)
    network_command_tx: Option<mpsc::Sender<NetworkCommand>>,

    /// Tokio runtime for async operations
    runtime: tokio::runtime::Runtime,

    /// Local device ID
    local_device_id: Uuid,

    /// System tray event receiver
    tray_event_rx: Option<std::sync::mpsc::Receiver<tray::TrayEvent>>,

    /// Window visibility
    window_visible: bool,

    /// Application configuration
    config: Config,

    /// Tray manager (for updating tray state)
    tray_manager: Option<tray::TrayManager>,
}

impl RShareApp {
    /// Create a new application instance
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        // Load configuration
        let config = Config::load().unwrap_or_default();

        // Create channel for network events
        let (network_event_tx, network_event_rx) = mpsc::channel(100);

        // Create tokio runtime
        let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

        // Generate local device ID
        let local_device_id = Uuid::new_v4();

        // Start with empty device list
        let devices = vec![];

        // Create settings view from loaded config
        let settings_view = SettingsView::from_config(&config);

        // Create system tray (optional - may fail on some platforms)
        let mut tray_manager = tray::TrayManager::new().ok();
        let tray_event_rx = tray_manager.as_mut().map(|tm| tm.events());

        let hostname = hostname::get()
            .unwrap_or_else(|_| "unknown".into())
            .to_string_lossy()
            .to_string();
        let mut layout_view = LayoutView::new();
        layout_view.set_local_device(format!("{} Mac", hostname), hostname.clone());

        Self {
            active_tab: ActiveTab::Layout,
            service_running: Arc::new(Mutex::new(false)),
            devices: Arc::new(Mutex::new(devices)),
            main_view: MainView::new(),
            settings_view,
            layout_view,
            show_confirmation: false,
            confirmation_message: String::new(),
            confirmation_action: None,
            network_event_rx,
            network_event_tx,
            network_command_tx: None,
            runtime,
            local_device_id,
            tray_event_rx,
            window_visible: true,
            config,
            tray_manager,
        }
    }

    /// Start the service
    fn start_service(&mut self) {
        log::info!("Starting service...");
        if *self.service_running.blocking_lock() {
            return;
        }

        #[cfg(target_os = "macos")]
        {
            if !rshare_platform::permissions::can_listen_events() {
                log::warn!(
                    "macOS Input Monitoring permission is missing; input capture will not work"
                );
            }
            if !rshare_platform::permissions::can_post_events() {
                log::warn!(
                    "macOS Accessibility permission is missing; remote input posting will not work"
                );
            }
        }

        // Mark as running
        *self.service_running.blocking_lock() = true;

        // Update tray icon if available
        if let Some(ref mut tm) = self.tray_manager {
            tm.set_service_running(true);
            tm.set_tooltip("R-ShareMouse - Service Running");
        }

        // Get hostname
        let hostname = hostname::get()
            .unwrap_or_else(|_| "unknown".into())
            .to_string_lossy()
            .to_string();

        let device_name = format!("{}-R-ShareMouse", hostname);
        let local_device_id = self.local_device_id;

        // Clone sender for the spawned task
        let event_tx = self.network_event_tx.clone();
        let service_running = self.service_running.clone();
        let (command_tx, mut command_rx) = mpsc::channel(32);
        self.network_command_tx = Some(command_tx);

        let network_config = rshare_net::NetworkManagerConfig {
            bind_address: format!(
                "{}:{}",
                self.config.network.bind_address, self.config.network.port
            ),
            auto_connect: false,
            broadcast_interval: std::time::Duration::from_secs(2),
            ..Default::default()
        };

        // Spawn network manager in background
        self.runtime.spawn(async move {
            let mut network_manager = rshare_net::NetworkManager::new(
                local_device_id,
                device_name.clone(),
                hostname.clone(),
            )
            .with_config(network_config);

            // Start the network manager
            if let Err(e) = network_manager.start().await {
                log::error!("Failed to start network manager: {}", e);
                *service_running.lock().await = false;
                return;
            }

            log::info!("Network manager started");

            // Forward events to GUI
            let mut events = network_manager.events();
            loop {
                tokio::select! {
                    command = command_rx.recv() => {
                        match command {
                            Some(NetworkCommand::Connect(id)) => {
                                if let Err(e) = network_manager.connect_to_discovered(id).await {
                                    log::warn!("Failed to connect to {}: {}", id, e);
                                    let _ = event_tx
                                        .send(rshare_net::NetworkEvent::ConnectionError {
                                            device_id: id,
                                            error: e.to_string(),
                                        })
                                        .await;
                                }
                            }
                            Some(NetworkCommand::Disconnect(id)) => {
                                if let Err(e) = network_manager.disconnect_from(&id).await {
                                    log::warn!("Failed to disconnect from {}: {}", id, e);
                                }
                            }
                            Some(NetworkCommand::Stop) | None => {
                                break;
                            }
                        }
                    }
                    event = events.recv() => {
                        match event {
                            Some(event) => {
                                if event_tx.send(event).await.is_err() {
                                    log::error!("Failed to send event to GUI");
                                    break;
                                }
                            }
                            None => {
                                log::warn!("Network event channel closed");
                                break;
                            }
                        }
                    }
                }
            }

            // Stop network manager
            let _ = network_manager.stop().await;
            *service_running.lock().await = false;
            log::info!("Network manager stopped");
        });
    }

    /// Stop the service
    fn stop_service(&mut self) {
        log::info!("Stopping service...");
        *self.service_running.blocking_lock() = false;
        if let Some(tx) = self.network_command_tx.take() {
            let _ = tx.try_send(NetworkCommand::Stop);
        }

        // Update tray icon if available
        if let Some(ref mut tm) = self.tray_manager {
            tm.set_service_running(false);
            tm.set_tooltip("R-ShareMouse - Service Stopped");
        }
    }

    /// Add a device to the list
    pub fn add_device(&self, device: UiDevice) {
        let mut devices = self.devices.blocking_lock();
        if let Some(existing) = devices.iter_mut().find(|d| d.id == device.id) {
            existing.name = device.name;
            existing.address = device.address;
            existing.online = device.online;
            existing.latency = device.latency;
        } else {
            devices.push(device);
        }
    }

    /// Update device status
    pub fn update_device_connection(&self, id: Uuid, connected: bool, latency: u32) {
        let mut devices = self.devices.blocking_lock();
        if let Some(device) = devices.iter_mut().find(|d| d.id == id) {
            device.connected = connected;
            device.online = device.online || connected;
            device.latency = latency;
        }
    }

    /// Remove a device from the list
    pub fn remove_device(&self, id: Uuid) {
        let mut devices = self.devices.blocking_lock();
        devices.retain(|d| d.id != id);
    }

    /// Get all devices
    pub fn get_devices(&self) -> Vec<UiDevice> {
        self.devices.blocking_lock().clone()
    }

    /// Show a confirmation dialog
    fn confirm(
        &mut self,
        message: impl Into<String>,
        action: impl FnOnce(&mut Self) + Send + 'static,
    ) {
        self.confirmation_message = message.into();
        self.confirmation_action = Some(Box::new(action));
        self.show_confirmation = true;
    }
}

impl eframe::App for RShareApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle window close event - minimize to tray if configured
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.config.gui.minimize_to_tray {
                // Consume the close event and hide instead
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.window_visible = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            // If not minimizing to tray, let the close proceed
        }

        // Process network events (non-blocking try_recv)
        while let Ok(event) = self.network_event_rx.try_recv() {
            match event {
                rshare_net::NetworkEvent::DeviceFound(device) => {
                    self.add_device(UiDevice {
                        id: device.id,
                        name: device.name.clone(),
                        address: device
                            .addresses
                            .first()
                            .map(|a| a.to_string())
                            .unwrap_or_else(|| "unknown".to_string()),
                        online: true,
                        connected: false,
                        latency: 0,
                    });
                    self.layout_view.add_device(
                        device.id.to_string(),
                        device.name,
                        device.hostname,
                        true,
                    );
                    self.layout_view
                        .apply_local_screen_layout(&self.config.gui.screen_layout);
                }
                rshare_net::NetworkEvent::DeviceConnected(id) => {
                    self.update_device_connection(id, true, 0);
                }
                rshare_net::NetworkEvent::DeviceDisconnected(id) => {
                    self.update_device_connection(id, false, 0);
                }
                rshare_net::NetworkEvent::ConnectionError { device_id, .. } => {
                    self.update_device_connection(device_id, false, 0);
                }
                _ => {}
            }
        }

        // Process tray events (non-blocking try_recv)
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
                    let is_running = {
                        let lock = self.service_running.blocking_lock();
                        *lock
                    };
                    if is_running {
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

        egui::TopBottomPanel::top("display_manager_header")
            .exact_height(64.0)
            .show(ctx, |ui| {
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, egui::Color32::from_rgb(30, 28, 38));
                ui.add_space(8.0);
                ui.horizontal_centered(|ui| {
                    ui.add_space(8.0);
                    ui.strong("R-ShareMouse v0.1.0 - Display Manager");
                    ui.add_space(18.0);

                    self.header_tab(ui, ActiveTab::Layout, "Displays");
                    self.header_tab(ui, ActiveTab::Devices, "Devices");
                    self.header_tab(ui, ActiveTab::Main, "Activity");

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("?").clicked() {
                            self.active_tab = ActiveTab::About;
                        }
                        if ui.button("Settings").clicked() {
                            self.active_tab = ActiveTab::Settings;
                        }

                        let is_running = *self.service_running.blocking_lock();
                        if ui
                            .button(if is_running { "Stop" } else { "Start" })
                            .clicked()
                        {
                            if is_running {
                                self.confirm("Stop the R-ShareMouse service?", |app| {
                                    app.stop_service()
                                });
                            } else {
                                self.start_service();
                            }
                        }

                        let status_color = if is_running {
                            egui::Color32::from_rgb(82, 190, 112)
                        } else {
                            egui::Color32::from_gray(150)
                        };
                        ui.colored_label(
                            status_color,
                            if is_running { "Running" } else { "Stopped" },
                        );
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(35, 37, 37))
                    .inner_margin(0.0),
            )
            .show(ctx, |ui| match self.active_tab {
                ActiveTab::Main => {
                    self.main_view.show(ui, ctx);
                }
                ActiveTab::Devices => {
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        ui.vertical(|ui| self.show_devices_tab(ui));
                    });
                }
                ActiveTab::Layout => {
                    self.layout_view.show(ui, ctx);
                }
                ActiveTab::Settings => {
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        self.settings_view.show(ui, ctx);
                    });
                }
                ActiveTab::About => {
                    self.show_about_tab(ui);
                }
            });

        // Confirmation dialog
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

        // Request continuous repaint for smooth animations
        ctx.request_repaint();
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        let mut next_config = if self.settings_view.is_modified() {
            self.settings_view.to_config()
        } else {
            self.config.clone()
        };
        next_config.gui.screen_layout = self.layout_view.local_screen_layout_entries();

        if next_config != self.config || self.settings_view.is_modified() {
            self.config = next_config;
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
    fn header_tab(&mut self, ui: &mut egui::Ui, tab: ActiveTab, label: &str) {
        let selected = self.active_tab == tab;
        let button = egui::Button::new(label)
            .fill(if selected {
                egui::Color32::from_rgb(76, 74, 91)
            } else {
                egui::Color32::from_rgb(38, 38, 45)
            })
            .stroke(egui::Stroke::new(
                1.0,
                if selected {
                    egui::Color32::from_rgb(126, 132, 168)
                } else {
                    egui::Color32::from_rgb(54, 54, 64)
                },
            ));
        if ui.add(button).clicked() {
            self.active_tab = tab;
        }
    }

    /// Show the devices tab
    fn show_devices_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Discovered Devices");
        ui.add_space(10.0);

        let devices = self.get_devices();

        if devices.is_empty() {
            ui.label("No devices found. Make sure R-ShareMouse is running on other devices.");
            ui.add_space(10.0);
            ui.label("Device discovery is active when the service is running.");
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

                for device in &devices {
                    ui.label(&device.name);
                    ui.label(&device.address);
                    if device.connected {
                        ui.colored_label(egui::Color32::GREEN, "● Connected");
                        if ui.button(format!("Disconnect {}", device.id)).clicked() {
                            if let Some(tx) = &self.network_command_tx {
                                let _ = tx.try_send(NetworkCommand::Disconnect(device.id));
                            }
                            log::info!("Disconnect requested for {}", device.id);
                        }
                    } else if device.online {
                        ui.colored_label(egui::Color32::YELLOW, "● Discovered");
                        if ui.button(format!("Connect {}", device.id)).clicked() {
                            if let Some(tx) = &self.network_command_tx {
                                let _ = tx.try_send(NetworkCommand::Connect(device.id));
                            }
                            log::info!("Connect requested for {}", device.id);
                        }
                    } else {
                        ui.colored_label(egui::Color32::GRAY, "○ Discovered");
                        if ui.button(format!("Connect {}", device.id)).clicked() {
                            if let Some(tx) = &self.network_command_tx {
                                let _ = tx.try_send(NetworkCommand::Connect(device.id));
                            }
                            log::info!("Connect requested for {}", device.id);
                        }
                    }
                    ui.end_row();
                }
            });

        ui.add_space(20.0);
        ui.heading("Network Status");
        ui.add_space(10.0);

        let is_running = *self.service_running.blocking_lock();
        if is_running {
            ui.colored_label(egui::Color32::GREEN, "● Network service is running");
            ui.label("• Device discovery: Active (UDP port 27432)");
            ui.label("• Listening for connections: 0.0.0.0:27431");
            ui.label(format!("• Discovered {} device(s)", devices.len()));
        } else {
            ui.colored_label(egui::Color32::GRAY, "○ Network service is stopped");
            ui.label("Start the service to enable device discovery and connections.");
        }

        self.show_platform_permissions(ui);
    }

    fn show_platform_permissions(&self, ui: &mut egui::Ui) {
        #[cfg(target_os = "macos")]
        {
            ui.add_space(20.0);
            ui.heading("macOS Permissions");
            ui.add_space(10.0);

            let can_listen = rshare_platform::permissions::can_listen_events();
            let can_post = rshare_platform::permissions::can_post_events();

            ui.horizontal(|ui| {
                if can_listen {
                    ui.colored_label(egui::Color32::GREEN, "● Input Monitoring granted");
                } else {
                    ui.colored_label(egui::Color32::RED, "● Input Monitoring missing");
                    if ui.button("Request Input Monitoring").clicked() {
                        let _ = rshare_platform::permissions::request_listen_events();
                    }
                }
            });

            ui.horizontal(|ui| {
                if can_post {
                    ui.colored_label(egui::Color32::GREEN, "● Accessibility granted");
                } else {
                    ui.colored_label(egui::Color32::RED, "● Accessibility missing");
                    if ui.button("Request Accessibility").clicked() {
                        let _ = rshare_platform::permissions::request_post_events();
                    }
                }
            });
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = ui;
        }
    }

    /// Show the about tab
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
