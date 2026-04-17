//! Main dashboard view

use eframe::egui;

/// Main dashboard view state
pub struct MainView {
    show_welcome: bool,
}

impl MainView {
    /// Create a new main view
    pub fn new() -> Self {
        Self {
            show_welcome: true,
        }
    }

    /// Show the main view
    pub fn show(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        ui.heading("Dashboard");
        ui.separator();
        ui.add_space(10.0);

        // Quick stats
        egui::Grid::new("main_stats")
            .spacing([20.0, 10.0])
            .show(ui, |ui| {
                // Status card
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.label("Service Status");
                        ui.strong("● Running");
                        ui.label("Connected: 2 devices");
                    });
                });

                // Network card
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.label("Network");
                        ui.strong("0.0.0.0:4242");
                        ui.label("Latency: ~10ms");
                    });
                });

                // Clipboard card
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.label("Clipboard");
                        ui.strong("Synced");
                        ui.label("Last sync: Just now");
                    });
                });
            });

        ui.add_space(20.0);

        // Quick actions
        ui.heading("Quick Actions");
        ui.add_space(5.0);

        ui.horizontal(|ui| {
            if ui.button("⚙ Settings").clicked() {
                // Navigate to settings
            }
            if ui.button("🖥 Layout").clicked() {
                // Navigate to layout
            }
            if ui.button("📋 Devices").clicked() {
                // Navigate to devices
            }
        });

        ui.add_space(20.0);

        // Activity log
        ui.heading("Recent Activity");
        ui.add_space(5.0);

        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                ui.label("10:30:45 - Desktop-PC connected");
                ui.label("10:30:42 - MacBook-Pro connected");
                ui.label("10:30:15 - Service started");
                ui.label("10:29:58 - Configuration loaded");
            });
    }
}
