//! Common frameless behavior; native decorations and mobile windows keep their own chrome.
#[cfg(desktop)]
#[path = "project_window_corners.rs"]
mod project_window_corners;

pub fn init<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let builder = tauri::plugin::Builder::new("project-window-chrome");
    #[cfg(desktop)]
    let builder = {
        use std::{
            collections::HashMap,
            sync::{Arc, Mutex},
        };
        use tauri::Manager;
        // Corners and chrome depend on window mode, not each intermediate size.
        // Never hold this lock while calling a native API or evaluating JavaScript.
        let modes = Arc::new(Mutex::new(HashMap::new()));
        let ready_modes = modes.clone();
        builder
            .on_window_ready(move |window| {
                if let Some(mode) = mode(&window) {
                    if let Ok(mut modes) = ready_modes.lock() {
                        modes.insert(window.label().to_owned(), mode);
                    }
                }
                project_window_corners::apply(&window);
            })
            .js_init_script(include_str!("project-window-chrome.js").replace(
                "__PROJECT_CHROME_MACOS__",
                if cfg!(target_os = "macos") {
                    "true"
                } else {
                    "false"
                },
            ))
            .on_event(move |app, event| {
                if let tauri::RunEvent::WindowEvent { label, event, .. } = event {
                    if matches!(event, tauri::WindowEvent::Destroyed) {
                        if let Ok(mut modes) = modes.lock() {
                            modes.remove(label);
                        }
                        return;
                    }
                    if !matches!(
                        event,
                        tauri::WindowEvent::Resized(_)
                            | tauri::WindowEvent::Focused(_)
                            | tauri::WindowEvent::ScaleFactorChanged { .. }
                    ) {
                        return;
                    }
                    let Some(window) = app.get_webview_window(label) else {
                        return;
                    };
                    let native = window.as_ref().window();
                    let Some(next) = mode(&native) else {
                        return;
                    };
                    let previous = modes.lock().ok().and_then(|mut modes| {
                        let previous = modes.get(label).copied();
                        if previous != Some(next) {
                            modes.insert(label.to_owned(), next);
                        }
                        previous
                    });
                    let scale_changed =
                        matches!(event, tauri::WindowEvent::ScaleFactorChanged { .. });
                    if previous == Some(next) && !scale_changed {
                        return;
                    }
                    if previous.map(|p| (p.0, p.1, p.2)) != Some((next.0, next.1, next.2))
                        || scale_changed
                    {
                        project_window_corners::apply(&native);
                    }
                    let _ = window
                        .eval("window.dispatchEvent(new Event('project-native-window-state'))");
                }
            })
    };
    builder.build()
}

#[cfg(desktop)]
fn mode<R: tauri::Runtime>(window: &tauri::Window<R>) -> Option<(bool, bool, bool, bool)> {
    Some((
        window.is_decorated().ok()?,
        window.is_maximized().ok()?,
        window.is_fullscreen().ok()?,
        window.is_focused().ok()?,
    ))
}
