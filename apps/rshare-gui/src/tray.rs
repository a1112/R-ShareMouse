//! System tray icon management

use std::sync::mpsc;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder,
};

/// Tray event
#[derive(Debug, Clone)]
pub enum TrayEvent {
    Show,
    Hide,
    Quit,
    ToggleService,
}

/// Tray icon manager
pub struct TrayManager {
    menu_tx: mpsc::Sender<TrayEvent>,
    menu_rx: Option<mpsc::Receiver<TrayEvent>>,
    _tray_icon: Option<TrayIcon>,
    show_item: MenuItem,
    hide_item: MenuItem,
    toggle_item: MenuItem,
    quit_item: MenuItem,
}

impl TrayManager {
    /// Create a new tray manager
    pub fn new() -> anyhow::Result<Self> {
        let (menu_tx, menu_rx) = mpsc::channel();

        // Create menu items
        let show_item = MenuItem::new("Show", true, None);
        let hide_item = MenuItem::new("Hide", true, None);
        let toggle_item = MenuItem::new("Start Service", true, None);
        let quit_item = MenuItem::new("Quit", true, None);

        // Create the tray menu
        let menu = Menu::new();
        let _ = menu.append(&show_item);
        let _ = menu.append(&hide_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&toggle_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&quit_item);

        let icon = Self::create_tray_icon()?;

        // Try to create the tray icon
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_icon(icon)
            .with_tooltip("R-ShareMouse")
            .with_title("R-ShareMouse")
            .build()
            .ok();

        // Set up menu event handler
        let tx = menu_tx.clone();
        let show_item_id = show_item.id().clone();
        let hide_item_id = hide_item.id().clone();
        let toggle_item_id = toggle_item.id().clone();
        let quit_item_id = quit_item.id().clone();

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id {
            id if id == show_item_id => {
                let _ = tx.send(TrayEvent::Show);
            }
            id if id == hide_item_id => {
                let _ = tx.send(TrayEvent::Hide);
            }
            id if id == toggle_item_id => {
                let _ = tx.send(TrayEvent::ToggleService);
            }
            id if id == quit_item_id => {
                let _ = tx.send(TrayEvent::Quit);
            }
            _ => {}
        }));

        Ok(Self {
            menu_tx,
            menu_rx: Some(menu_rx),
            _tray_icon: tray_icon,
            show_item,
            hide_item,
            toggle_item,
            quit_item,
        })
    }

    /// Create a display-shaped tray icon from RGBA data.
    fn create_tray_icon() -> anyhow::Result<tray_icon::Icon> {
        let size = 32;
        let rgba = monitor_icon_rgba(size);
        tray_icon::Icon::from_rgba(rgba, size, size)
            .map_err(|e| anyhow::anyhow!("Failed to create icon: {:?}", e))
    }

    /// Get the event receiver
    pub fn events(&mut self) -> mpsc::Receiver<TrayEvent> {
        self.menu_rx.take().expect("Event receiver already taken")
    }

    /// Update the toggle service menu item text
    pub fn set_service_running(&mut self, running: bool) {
        if running {
            let _ = self.toggle_item.set_text("Stop Service");
        } else {
            let _ = self.toggle_item.set_text("Start Service");
        }
    }

    /// Set the application title/tooltip
    pub fn set_tooltip(&mut self, text: &str) {
        if let Some(ref icon) = self._tray_icon {
            let _ = icon.set_tooltip(Some(text));
        }
    }
}

pub fn monitor_icon_rgba(size: u32) -> Vec<u8> {
    let mut rgba = vec![0; (size * size * 4) as usize];
    let scale = size as f32 / 32.0;
    let sx = |value: u32| ((value as f32 * scale).round() as u32).min(size.saturating_sub(1));

    let screen = (sx(5), sx(6), sx(27).max(sx(5) + 1), sx(21).max(sx(6) + 1));
    let stand = (
        sx(14),
        sx(22),
        sx(18).max(sx(14) + 1),
        sx(25).max(sx(22) + 1),
    );
    let base = (
        sx(10),
        sx(26),
        sx(22).max(sx(10) + 1),
        sx(28).max(sx(26) + 1),
    );

    for y in 0..size {
        for x in 0..size {
            let in_screen_border = x >= screen.0
                && x <= screen.2
                && y >= screen.1
                && y <= screen.3
                && (x <= screen.0 + 1
                    || x >= screen.2 - 1
                    || y <= screen.1 + 1
                    || y >= screen.3 - 1);
            let in_stand = x >= stand.0 && x <= stand.2 && y >= stand.1 && y <= stand.3;
            let in_base = x >= base.0 && x <= base.2 && y >= base.1 && y <= base.3;
            let in_accent =
                x >= screen.0 + 3 && x <= screen.0 + 8 && y >= screen.1 + 3 && y <= screen.1 + 5;

            let color = if in_accent {
                Some((130, 151, 229, 255))
            } else if in_screen_border || in_stand || in_base {
                Some((238, 241, 255, 255))
            } else {
                None
            };

            if let Some((r, g, b, a)) = color {
                let idx = ((y * size + x) * 4) as usize;
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = a;
            }
        }
    }

    rgba
}

impl Default for TrayManager {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| {
            let (menu_tx, menu_rx) = mpsc::channel();

            // Fallback stub when tray creation fails
            Self {
                menu_tx,
                menu_rx: Some(menu_rx),
                _tray_icon: None,
                show_item: MenuItem::new("Show", true, None),
                hide_item: MenuItem::new("Hide", true, None),
                toggle_item: MenuItem::new("Start Service", true, None),
                quit_item: MenuItem::new("Quit", true, None),
            }
        })
    }
}
