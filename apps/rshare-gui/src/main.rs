//! R-ShareMouse GUI application
//!
//! Main entry point for the egui-based GUI.

use eframe::egui;

mod app;
mod ui;
mod tray;

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
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([600.0, 400.0])
            .with_icon(load_icon()),
        ..Default::default()
    };

    // Run the application
    eframe::run_native(
        "R-ShareMouse",
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
    // TODO: Load actual icon from assets
    // For now, return a simple 1x1 transparent icon
    egui::IconData {
        rgba: vec![0, 0, 0, 0],
        width: 1,
        height: 1,
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
    style.visuals.panel_fill = egui::Color32::from_gray(30);
    style.visuals.faint_bg_color = egui::Color32::from_gray(40);

    ctx.set_style(style);
}
