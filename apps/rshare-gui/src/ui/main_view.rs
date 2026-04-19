//! Main dashboard view

use eframe::egui;

use crate::{app::UiDevice, dashboard::DashboardSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardAction {
    OpenDevices,
    OpenLayout,
    OpenSettings,
    StartService,
    StopService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DashboardContentLayout {
    Stacked,
    Split,
}

impl DashboardContentLayout {
    fn for_width(width: f32) -> Self {
        if width >= 980.0 {
            Self::Split
        } else {
            Self::Stacked
        }
    }
}

fn primary_action(summary: &DashboardSummary) -> DashboardAction {
    if summary.service_running {
        DashboardAction::StopService
    } else {
        DashboardAction::StartService
    }
}

fn primary_action_label(summary: &DashboardSummary) -> &'static str {
    match primary_action(summary) {
        DashboardAction::StartService => "Start Service",
        DashboardAction::StopService => "Stop Service",
        DashboardAction::OpenDevices
        | DashboardAction::OpenLayout
        | DashboardAction::OpenSettings => unreachable!("not a primary service action"),
    }
}

fn quick_actions() -> [DashboardAction; 3] {
    [
        DashboardAction::OpenDevices,
        DashboardAction::OpenLayout,
        DashboardAction::OpenSettings,
    ]
}

fn quick_action_label(action: DashboardAction) -> &'static str {
    match action {
        DashboardAction::OpenDevices => "Open Devices",
        DashboardAction::OpenLayout => "Edit Layout",
        DashboardAction::OpenSettings => "Adjust Settings",
        DashboardAction::StartService | DashboardAction::StopService => {
            unreachable!("service actions are not quick links")
        }
    }
}

fn quick_action_description(action: DashboardAction) -> &'static str {
    match action {
        DashboardAction::OpenDevices => "Review discovered computers and connection state.",
        DashboardAction::OpenLayout => "Arrange screen edges for mouse handoff.",
        DashboardAction::OpenSettings => "Tune ports, behavior, and tray preferences.",
        DashboardAction::StartService | DashboardAction::StopService => {
            unreachable!("service actions are not quick links")
        }
    }
}

/// Main dashboard view state
pub struct MainView;

impl MainView {
    /// Create a new main view
    pub fn new() -> Self {
        Self
    }

    /// Show the main view and return a requested user action.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        summary: &DashboardSummary,
        devices: &[UiDevice],
        activity_log: &[String],
    ) -> Option<DashboardAction> {
        let mut action = None;
        let layout = DashboardContentLayout::for_width(ui.available_width());

        self.show_hero(ui, summary, &mut action);
        ui.add_space(18.0);
        self.show_metric_cards(ui, summary);
        ui.add_space(18.0);

        match layout {
            DashboardContentLayout::Stacked => {
                self.show_device_panel(ui, summary, devices, &mut action);
                ui.add_space(16.0);
                self.show_activity_panel(ui, activity_log);
            }
            DashboardContentLayout::Split => {
                ui.columns(2, |columns| {
                    self.show_activity_panel(&mut columns[0], activity_log);
                    self.show_device_panel(&mut columns[1], summary, devices, &mut action);
                });
            }
        }

        action
    }

    fn show_hero(
        &self,
        ui: &mut egui::Ui,
        summary: &DashboardSummary,
        action: &mut Option<DashboardAction>,
    ) {
        self.panel_frame(egui::Color32::from_rgb(26, 41, 68))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading("Control Console");
                        ui.add_space(6.0);
                        ui.label("Monitor daemon health, active devices, and the next action at a glance.");
                        ui.add_space(10.0);

                        let tone = if summary.service_running {
                            egui::Color32::from_rgb(108, 214, 152)
                        } else {
                            egui::Color32::from_rgb(241, 183, 78)
                        };

                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(tone, &summary.service_label);
                            ui.label(format!(
                                "{} connected · {} discovered",
                                summary.connected_count, summary.device_count
                            ));
                            ui.label(format!("Clipboard {}", summary.clipboard_label));
                        });
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let button = egui::Button::new(primary_action_label(summary))
                            .min_size(egui::vec2(140.0, 42.0));
                        if ui.add(button).clicked() {
                            *action = Some(primary_action(summary));
                        }
                    });
                });
            });
    }

    fn show_metric_cards(&self, ui: &mut egui::Ui, summary: &DashboardSummary) {
        let width = ui.available_width();
        if width >= 840.0 {
            ui.columns(3, |columns| {
                self.metric_card(
                    &mut columns[0],
                    "Service",
                    &summary.service_label,
                    if summary.service_running {
                        "Daemon reachable and responding."
                    } else {
                        "Launch the daemon to start discovery."
                    },
                    if summary.service_running {
                        egui::Color32::from_rgb(78, 201, 140)
                    } else {
                        egui::Color32::from_rgb(214, 138, 85)
                    },
                );
                self.metric_card(
                    &mut columns[1],
                    "Network",
                    &summary.network_label,
                    &format!(
                        "{} device(s) visible on the network.",
                        summary.device_count
                    ),
                    egui::Color32::from_rgb(100, 170, 255),
                );
                self.metric_card(
                    &mut columns[2],
                    "Clipboard",
                    &summary.clipboard_label,
                    if summary.service_running {
                        "Clipboard watcher is linked to the daemon."
                    } else {
                        "Clipboard sync becomes active after startup."
                    },
                    egui::Color32::from_rgb(174, 142, 255),
                );
            });
        } else {
            self.metric_card(
                ui,
                "Service",
                &summary.service_label,
                if summary.service_running {
                    "Daemon reachable and responding."
                } else {
                    "Launch the daemon to start discovery."
                },
                if summary.service_running {
                    egui::Color32::from_rgb(78, 201, 140)
                } else {
                    egui::Color32::from_rgb(214, 138, 85)
                },
            );
            ui.add_space(12.0);
            self.metric_card(
                ui,
                "Network",
                &summary.network_label,
                &format!("{} device(s) visible on the network.", summary.device_count),
                egui::Color32::from_rgb(100, 170, 255),
            );
            ui.add_space(12.0);
            self.metric_card(
                ui,
                "Clipboard",
                &summary.clipboard_label,
                if summary.service_running {
                    "Clipboard watcher is linked to the daemon."
                } else {
                    "Clipboard sync becomes active after startup."
                },
                egui::Color32::from_rgb(174, 142, 255),
            );
        }
    }

    fn metric_card(
        &self,
        ui: &mut egui::Ui,
        title: &str,
        value: &str,
        detail: &str,
        accent: egui::Color32,
    ) {
        self.panel_frame(egui::Color32::from_rgb(22, 28, 42))
            .show(ui, |ui| {
                ui.set_min_height(120.0);
                ui.vertical(|ui| {
                    ui.colored_label(accent, title);
                    ui.add_space(6.0);
                    ui.heading(value);
                    ui.add_space(6.0);
                    ui.label(detail);
                });
            });
    }

    fn show_activity_panel(&self, ui: &mut egui::Ui, activity_log: &[String]) {
        self.panel_frame(egui::Color32::from_rgb(19, 24, 36))
            .show(ui, |ui| {
                ui.heading("Recent Activity");
                ui.add_space(8.0);

                if activity_log.is_empty() {
                    ui.label("No activity yet. Start the daemon to populate this timeline.");
                    return;
                }

                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for (index, entry) in activity_log.iter().take(8).enumerate() {
                            if index > 0 {
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(8.0);
                            }

                            ui.horizontal(|ui| {
                                ui.colored_label(
                                    egui::Color32::from_rgb(100, 170, 255),
                                    "●",
                                );
                                ui.label(entry);
                            });
                        }
                    });
            });
    }

    fn show_device_panel(
        &self,
        ui: &mut egui::Ui,
        summary: &DashboardSummary,
        devices: &[UiDevice],
        action: &mut Option<DashboardAction>,
    ) {
        self.panel_frame(egui::Color32::from_rgb(19, 24, 36))
            .show(ui, |ui| {
                ui.heading("Operations");
                ui.add_space(4.0);
                ui.label("Shortcuts for the tasks you perform most often.");
                ui.add_space(12.0);

                for quick_action in quick_actions() {
                    let button = egui::Button::new(quick_action_label(quick_action))
                        .min_size(egui::vec2(ui.available_width(), 36.0));
                    if ui.add(button).clicked() {
                        *action = Some(quick_action);
                    }
                    ui.add_space(4.0);
                    ui.small(quick_action_description(quick_action));
                    ui.add_space(10.0);
                }

                ui.separator();
                ui.add_space(12.0);

                ui.heading("Device Snapshot");
                ui.label(format!(
                    "{} connected / {} discovered",
                    summary.connected_count, summary.device_count
                ));
                ui.add_space(10.0);

                if devices.is_empty() {
                    ui.label("No devices discovered yet. Keep the service running and wait for peers.");
                } else {
                    for device in devices.iter().take(4) {
                        let status_color = if device.connected {
                            egui::Color32::from_rgb(108, 214, 152)
                        } else {
                            egui::Color32::from_gray(150)
                        };
                        let status_text = if device.connected {
                            "Connected"
                        } else {
                            "Discovered"
                        };

                        ui.horizontal(|ui| {
                            ui.colored_label(status_color, "●");
                            ui.vertical(|ui| {
                                ui.strong(&device.name);
                                ui.small(format!("{} · {}", status_text, device.address));
                            });
                        });
                        ui.add_space(8.0);
                    }
                }
            });
    }

    fn panel_frame(&self, fill: egui::Color32) -> egui::Frame {
        egui::Frame::none()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 61, 84)))
            .inner_margin(egui::Margin::same(18.0))
            .rounding(egui::Rounding::same(14.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::DashboardSummary;

    fn stopped_summary() -> DashboardSummary {
        DashboardSummary {
            service_label: "○ Stopped".to_string(),
            service_running: false,
            device_count: 0,
            connected_count: 0,
            network_label: "Daemon offline".to_string(),
            clipboard_label: "Unavailable".to_string(),
        }
    }

    #[test]
    fn narrow_width_uses_stacked_dashboard_layout() {
        assert_eq!(DashboardContentLayout::for_width(720.0), DashboardContentLayout::Stacked);
    }

    #[test]
    fn wide_width_uses_split_dashboard_layout() {
        assert_eq!(DashboardContentLayout::for_width(1180.0), DashboardContentLayout::Split);
    }

    #[test]
    fn offline_summary_uses_start_service_primary_action() {
        let summary = stopped_summary();

        assert_eq!(primary_action(&summary), DashboardAction::StartService);
        assert_eq!(primary_action_label(&summary), "Start Service");
    }

    #[test]
    fn online_summary_uses_stop_service_primary_action() {
        let mut summary = stopped_summary();
        summary.service_running = true;
        summary.service_label = "● Running".to_string();

        assert_eq!(primary_action(&summary), DashboardAction::StopService);
        assert_eq!(primary_action_label(&summary), "Stop Service");
    }

    #[test]
    fn quick_actions_map_to_navigation_targets() {
        assert_eq!(
            quick_actions(),
            [
                DashboardAction::OpenDevices,
                DashboardAction::OpenLayout,
                DashboardAction::OpenSettings,
            ]
        );
    }
}
