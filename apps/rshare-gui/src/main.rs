//! R-ShareMouse GUI application
//!
//! Main entry point for the egui-based GUI.

use eframe::egui;

mod app;
mod dashboard;
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
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([920.0, 620.0])
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
    let width = 32;
    let height = 32;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);

    for y in 0..height {
        for x in 0..width {
            let inside = (4..28).contains(&x) && (4..28).contains(&y);
            let edge = x < 5 || x > 26 || y < 5 || y > 26;
            let (r, g, b, a) = if edge {
                (34, 48, 78, 255)
            } else if inside {
                (105, 170, 255, 255)
            } else {
                (10, 14, 22, 255)
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }

    egui::IconData {
        rgba,
        width,
        height,
    }
}

/// Setup custom fonts
fn setup_fonts(ctx: &egui::Context) {
    let fonts = egui::FontDefinitions::default();
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (egui::TextStyle::Heading, egui::FontId::proportional(28.0)),
        (egui::TextStyle::Body, egui::FontId::proportional(16.0)),
        (egui::TextStyle::Button, egui::FontId::proportional(15.0)),
        (egui::TextStyle::Small, egui::FontId::proportional(13.0)),
        (egui::TextStyle::Monospace, egui::FontId::monospace(14.0)),
    ]
    .into();
    ctx.set_style(style);
}

/// Setup application style
fn setup_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 12.0);
    style.spacing.button_padding = egui::vec2(14.0, 10.0);
    style.spacing.window_margin = egui::Margin::same(16.0);
    style.spacing.menu_margin = egui::Margin::same(10.0);

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(egui::Color32::from_rgb(232, 238, 249));
    visuals.panel_fill = egui::Color32::from_rgb(12, 16, 24);
    visuals.window_fill = egui::Color32::from_rgb(16, 20, 30);
    visuals.faint_bg_color = egui::Color32::from_rgb(23, 30, 44);
    visuals.extreme_bg_color = egui::Color32::from_rgb(8, 11, 18);
    visuals.code_bg_color = egui::Color32::from_rgb(19, 24, 36);
    visuals.selection.bg_fill = egui::Color32::from_rgb(67, 115, 201);
    visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    visuals.hyperlink_color = egui::Color32::from_rgb(112, 175, 255);

    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(17, 22, 32);
    visuals.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 54, 77));

    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(28, 36, 52);
    visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(56, 70, 96));

    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(40, 52, 76);
    visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(104, 142, 205));

    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(50, 73, 112);
    visuals.widgets.active.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(132, 176, 245));

    visuals.widgets.open.bg_fill = egui::Color32::from_rgb(34, 44, 64);
    visuals.widgets.open.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 102, 142));

    style.visuals = visuals;

    ctx.set_style(style);
}
