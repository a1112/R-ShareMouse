//! R-ShareMouse GUI application
//!
//! Main entry point for the egui-based GUI.

use eframe::egui;

mod app;
mod tray;
mod ui;

use app::RShareApp;

fn main() -> eframe::Result<()> {
    // Initialize logging
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    log::info!("R-ShareMouse GUI starting...");

    // Set up egui options
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("R-ShareMouse v0.1.0 - Display Manager")
            .with_inner_size([1180.0, 665.0])
            .with_min_inner_size([840.0, 520.0])
            .with_icon(load_icon()),
        ..Default::default()
    };

    // Run the application
    eframe::run_native(
        "R-ShareMouse v0.1.0 - Display Manager",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            setup_style(&cc.egui_ctx);
            Ok(Box::new(RShareApp::new(cc)))
        }),
    )
}

/// Load application icon from embedded data
fn load_icon() -> egui::IconData {
    let size = 64;
    egui::IconData {
        rgba: tray::monitor_icon_rgba(size),
        width: size,
        height: size,
    }
}

/// Setup custom fonts
fn setup_fonts(ctx: &egui::Context) {
    let fonts = egui::FontDefinitions::default();
    ctx.set_fonts(fonts);
}

/// Setup application style
fn setup_style(ctx: &egui::Context) {
    let mut style = egui::Style::default();

    // Custom colors
    style.visuals.dark_mode = true;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.visuals.panel_fill = egui::Color32::from_rgb(31, 30, 38);
    style.visuals.window_fill = egui::Color32::from_rgb(32, 34, 34);
    style.visuals.faint_bg_color = egui::Color32::from_rgb(42, 43, 43);
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(40, 40, 48);
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(57, 56, 68);
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(95, 92, 115);

    ctx.set_style(style);
}
