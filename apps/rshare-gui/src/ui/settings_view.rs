//! Settings view

use eframe::egui;
use rshare_core::config::Config;

/// Settings view state
pub struct SettingsView {
    // Network settings
    port: String,
    bind_address: String,

    // Service settings
    auto_start: bool,
    minimize_to_tray: bool,
    show_notifications: bool,
    start_minimized: bool,

    // Clipboard settings
    clipboard_sync: bool,
    clipboard_format: ClipboardFormat,

    // Security settings
    require_password: bool,
    password: String,

    // Edge threshold
    edge_threshold: u32,

    // Track if settings were modified
    modified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardFormat {
    Text,
    TextAndImages,
    TextAndHtml,
}

impl SettingsView {
    /// Create a new settings view from config
    pub fn from_config(config: &Config) -> Self {
        Self {
            port: config.network.port.to_string(),
            bind_address: config.network.bind_address.clone(),
            auto_start: false, // OS-specific, not stored in config
            minimize_to_tray: config.gui.minimize_to_tray,
            show_notifications: config.gui.show_notifications,
            start_minimized: config.gui.start_minimized,
            clipboard_sync: config.input.clipboard_sync,
            clipboard_format: ClipboardFormat::Text,
            require_password: config.security.password_required,
            password: String::new(), // Don't load actual password for security
            edge_threshold: config.input.edge_threshold,
            modified: false,
        }
    }

    /// Create a new settings view with defaults
    pub fn new() -> Self {
        Self::from_config(&Config::default())
    }

    /// Convert settings to Config
    pub fn to_config(&self) -> Config {
        let mut config = Config::default();

        // Network settings
        config.network.port = self.port.parse().unwrap_or(27431);
        config.network.bind_address = self.bind_address.clone();

        // GUI settings
        config.gui.minimize_to_tray = self.minimize_to_tray;
        config.gui.show_notifications = self.show_notifications;
        config.gui.start_minimized = self.start_minimized;

        // Input settings
        config.input.clipboard_sync = self.clipboard_sync;
        config.input.edge_threshold = self.edge_threshold;

        // Security settings
        config.security.password_required = self.require_password;

        config
    }

    /// Check if settings were modified
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Mark settings as saved
    pub fn mark_saved(&mut self) {
        self.modified = false;
    }

    /// Show the settings view
    pub fn show(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        ui.heading("Settings");
        ui.separator();
        ui.add_space(10.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                // Network section
                self.show_network_section(ui);
                ui.add_space(20.0);

                // Service section
                self.show_service_section(ui);
                ui.add_space(20.0);

                // Clipboard section
                self.show_clipboard_section(ui);
                ui.add_space(20.0);

                // Security section
                self.show_security_section(ui);
                ui.add_space(20.0);
            });

        ui.separator();
        ui.add_space(10.0);

        // Status indicator
        if self.modified {
            ui.colored_label(egui::Color32::YELLOW, "⚠ Settings have been modified");
        } else {
            ui.colored_label(egui::Color32::GRAY, "No unsaved changes");
        }
    }

    fn show_network_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Network");
        ui.separator();

        let prev_port = self.port.clone();
        let prev_bind = self.bind_address.clone();

        egui::Grid::new("network_settings")
            .spacing([10.0, 5.0])
            .show(ui, |ui| {
                ui.label("Listen Port:");
                ui.add(egui::TextEdit::singleline(&mut self.port).desired_width(100.0));
                ui.label("UDP port for device discovery and communication");
                ui.end_row();

                ui.label("Bind Address:");
                ui.add(egui::TextEdit::singleline(&mut self.bind_address).desired_width(150.0));
                ui.label("Local address to bind to");
                ui.end_row();
            });

        if prev_port != self.port || prev_bind != self.bind_address {
            self.modified = true;
        }
    }

    fn show_service_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Service");
        ui.separator();

        let prev_auto_start = self.auto_start;
        let prev_minimize = self.minimize_to_tray;
        let prev_notifications = self.show_notifications;
        let prev_minimized = self.start_minimized;

        ui.checkbox(&mut self.auto_start, "Start service automatically on login");
        ui.checkbox(&mut self.minimize_to_tray, "Minimize to system tray");
        ui.checkbox(&mut self.show_notifications, "Show notifications");
        ui.checkbox(&mut self.start_minimized, "Start minimized");

        // Track modifications
        if prev_auto_start != self.auto_start
            || prev_minimize != self.minimize_to_tray
            || prev_notifications != self.show_notifications
            || prev_minimized != self.start_minimized
        {
            self.modified = true;
        }
    }

    fn show_clipboard_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Clipboard");
        ui.separator();

        let prev_sync = self.clipboard_sync;
        let prev_threshold = self.edge_threshold;

        ui.checkbox(&mut self.clipboard_sync, "Enable clipboard synchronization");

        ui.add_space(5.0);
        ui.label("Screen edge threshold:");
        ui.add(egui::Slider::new(&mut self.edge_threshold, 1..=100).text("pixels"));

        ui.horizontal(|ui| {
            ui.label("Sync format:");
            egui::ComboBox::new("clipboard_format", "Format")
                .selected_text(format!("{:?}", self.clipboard_format))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.clipboard_format,
                        ClipboardFormat::Text,
                        "Text only",
                    );
                    ui.selectable_value(
                        &mut self.clipboard_format,
                        ClipboardFormat::TextAndImages,
                        "Text + Images",
                    );
                    ui.selectable_value(
                        &mut self.clipboard_format,
                        ClipboardFormat::TextAndHtml,
                        "Text + HTML",
                    );
                });
        });

        if prev_sync != self.clipboard_sync || prev_threshold != self.edge_threshold {
            self.modified = true;
        }
    }

    fn show_security_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Security");
        ui.separator();

        let prev_require = self.require_password;
        let prev_password = self.password.clone();

        ui.checkbox(
            &mut self.require_password,
            "Require password for connections",
        );

        if self.require_password {
            ui.horizontal(|ui| {
                ui.label("Password:");
                ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
            });
        }

        if prev_require != self.require_password || prev_password != self.password {
            self.modified = true;
        }
    }
}
