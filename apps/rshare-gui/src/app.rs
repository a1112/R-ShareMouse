//! Main application state

use eframe::egui;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use rshare_core::config::Config;

use crate::ui::{
    main_view::MainView,
    settings_view::SettingsView,
    layout_view::LayoutView,
};

use crate::tray;

/// Device information for UI display
#[derive(Debug, Clone)]
pub struct UiDevice {
    pub id: Uuid,
    pub name: String,
    pub address: String,
    pub online: bool,
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
        let runtime = tokio::runtime::Runtime::new()
            .expect("Failed to create tokio runtime");

        // Generate local device ID
        let local_device_id = Uuid::new_v4();

        // Start with empty device list
        let devices = vec![];

        // Create settings view from loaded config
        let settings_view = SettingsView::from_config(&config);

        // Create system tray (optional - may fail on some platforms)
        let mut tray_manager = tray::TrayManager::new().ok();
        let tray_event_rx = tray_manager.as_mut()
            .map(|tm| tm.events());

        Self {
            active_tab: ActiveTab::Main,
            service_running: Arc::new(Mutex::new(false)),
            devices: Arc::new(Mutex::new(devices)),
            main_view: MainView::new(),
            settings_view,
            layout_view: LayoutView::new(),
            show_confirmation: false,
            confirmation_message: String::new(),
            confirmation_action: None,
            network_event_rx,
            network_event_tx,
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

        // Spawn network manager in background
        self.runtime.spawn(async move {
            let mut network_manager = rshare_net::NetworkManager::new(
                local_device_id,
                device_name.clone(),
                hostname.clone(),
            );

            // Start the network manager
            if let Err(e) = network_manager.start().await {
                log::error!("Failed to start network manager: {}", e);
                let _ = service_running.lock().await;
                return;
            }

            log::info!("Network manager started");

            // Forward events to GUI
            let mut events = network_manager.events();
            while *service_running.lock().await {
                match events.recv().await {
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

            // Stop network manager
            let _ = network_manager.stop().await;
            log::info!("Network manager stopped");
        });
    }

    /// Stop the service
    fn stop_service(&mut self) {
        log::info!("Stopping service...");
        *self.service_running.blocking_lock() = false;

        // Update tray icon if available
        if let Some(ref mut tm) = self.tray_manager {
            tm.set_service_running(false);
            tm.set_tooltip("R-ShareMouse - Service Stopped");
        }
    }

    /// Add a device to the list
    pub fn add_device(&self, device: UiDevice) {
        let mut devices = self.devices.blocking_lock();
        // Don't add duplicates
        if !devices.iter().any(|d| d.id == device.id) {
            devices.push(device);
        }
    }

    /// Update device status
    pub fn update_device(&self, id: Uuid, online: bool, latency: u32) {
        let mut devices = self.devices.blocking_lock();
        if let Some(device) = devices.iter_mut().find(|d| d.id == id) {
            device.online = online;
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
    fn confirm(&mut self, message: impl Into<String>, action: impl FnOnce(&mut Self) + Send + 'static) {
        self.confirmation_message = message.into();
        self.confirmation_action = Some(Box::new(action));
        self.show_confirmation = true;
    }
}

impl eframe::App for RShareApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
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
                        name: device.name,
                        address: device.addresses.first()
                            .map(|a| a.to_string())
                            .unwrap_or_else(|| "unknown".to_string()),
                        online: true,
                        latency: 0,
                    });
                }
                rshare_net::NetworkEvent::DeviceConnected(id) => {
                    self.update_device(id, true, 0);
                }
                rshare_net::NetworkEvent::DeviceDisconnected(id) => {
                    self.update_device(id, false, 0);
                }
                rshare_net::NetworkEvent::ConnectionError { device_id, .. } => {
                    self.update_device(device_id, false, 0);
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

        // Top menu bar
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
                    // Service status indicator
                    let is_running = self.service_running.blocking_lock().clone();
                    let status_text = if is_running { "● Running" } else { "○ Stopped" };
                    let status_color = if is_running {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::GRAY
                    };
                    ui.colored_label(status_color, status_text);

                    if ui.button(if is_running { "Stop" } else { "Start" }).clicked() {
                        if is_running {
                            self.confirm("Stop the R-ShareMouse service?", |app| app.stop_service());
                        } else {
                            self.start_service();
                        }
                    }
                });
            });
        });

        // Tab content
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.active_tab {
                ActiveTab::Main => {
                    self.main_view.show(ui, ctx);
                }
                ActiveTab::Devices => {
                    self.show_devices_tab(ui);
                }
                ActiveTab::Layout => {
                    self.layout_view.show(ui, ctx);
                }
                ActiveTab::Settings => {
                    self.settings_view.show(ui, ctx);
                }
                ActiveTab::About => {
                    self.show_about_tab(ui);
                }
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
        // Save settings if they were modified
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
                    if device.online {
                        ui.colored_label(egui::Color32::GREEN, "● Connected");
                        if ui.button(format!("Disconnect {}", device.id)).clicked() {
                            // TODO: Implement disconnect
                            log::info!("Disconnect requested for {}", device.id);
                        }
                    } else {
                        ui.colored_label(egui::Color32::GRAY, "○ Discovered");
                        if ui.button(format!("Connect {}", device.id)).clicked() {
                            // TODO: Implement manual connect
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
