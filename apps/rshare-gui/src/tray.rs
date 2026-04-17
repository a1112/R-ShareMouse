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

        // Create a simple icon
        let icon = Self::create_simple_icon()?;

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

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            match event.id {
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
            }
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

    /// Create a simple icon from RGBA data
    fn create_simple_icon() -> anyhow::Result<tray_icon::Icon> {
        // Simple 2x2 white icon (R, G, B, A)
        let rgba = vec![
            255, 255, 255, 255,
            255, 255, 255, 255,
            255, 255, 255, 255,
            255, 255, 255, 255,
        ];
        tray_icon::Icon::from_rgba(rgba, 2, 2)
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
