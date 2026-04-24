//! Linux kernel-level input capture and injection
//!
//! This module provides driver-level input handling using:
//! - evdev: Direct reading from /dev/input/event* devices
//! - uinput: Creating virtual input devices for injection

use anyhow::{Context, Result};
use evdev::{AttributeSet, Device, EventType, Key, RelativeAxisType};
use std::collections::HashMap;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::linux::DisplayServer;

// ============================================================================
// Driver-level Input Capture
// ============================================================================

/// Driver-level input listener using evdev
pub struct EvdevInputListener {
    running: Arc<AtomicBool>,
    event_count: Arc<AtomicUsize>,
    thread_handles: Vec<JoinHandle<()>>,
    callback: Option<Arc<dyn Fn(EvdevDriverEvent) + Send + Sync>>,
}

/// Raw input event from kernel
#[derive(Debug, Clone)]
pub enum EvdevDriverEvent {
    MouseMove { x: i32, y: i32 },
    MouseButton { button: u32, pressed: bool },
    MouseWheel { delta_x: i32, delta_y: i32 },
    Key { keycode: u32, pressed: bool },
}

impl EvdevInputListener {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            event_count: Arc::new(AtomicUsize::new(0)),
            thread_handles: Vec::new(),
            callback: None,
        }
    }

    /// Start capturing input from all keyboard and mouse devices
    pub fn start<F>(&mut self, callback: F) -> Result<()>
    where
        F: Fn(EvdevDriverEvent) + Send + Sync + 'static,
    {
        if self.running.load(Ordering::Relaxed) {
            return Ok(());
        }

        // Find all input devices
        let devices = Self::find_input_devices()?;

        if devices.is_empty() {
            anyhow::bail!("No input devices found in /dev/input/");
        }

        tracing::info!(
            "Found {} input devices: {:?}",
            devices.len(),
            devices.iter().map(|d| d.display()).collect::<Vec<_>>()
        );

        self.running.store(true, Ordering::Relaxed);
        self.callback = Some(Arc::new(callback));

        let running = self.running.clone();
        let event_count = self.event_count.clone();
        let callback = self.callback.clone().unwrap();

        // Spawn a thread for each device
        for device_path in devices {
            let running = running.clone();
            let event_count = event_count.clone();
            let callback = callback.clone();

            let handle = thread::Builder::new()
                .name(format!("rshare-evdev-{}", device_path.display()))
                .spawn(move || {
                    tracing::debug!("Thread started for device: {:?}", device_path);
                    let result = Self::capture_device(&device_path, running, event_count, callback);
                    match &result {
                        Ok(_) => {
                            tracing::debug!("Thread exiting normally for device: {:?}", device_path)
                        }
                        Err(e) => tracing::error!("Error capturing {:?}: {:?}", device_path, e),
                    }
                })?;

            self.thread_handles.push(handle);
        }

        Ok(())
    }

    /// Find all keyboard and mouse devices
    fn find_input_devices() -> Result<Vec<PathBuf>> {
        let mut devices = Vec::new();

        let input_dir = Path::new("/dev/input");
        if !input_dir.exists() {
            anyhow::bail!("/dev/input directory not found");
        }

        // Iterate over event* files
        for entry in input_dir.read_dir().context("Failed to read /dev/input")? {
            let entry = entry?;
            let path = entry.path();

            // Only process event* files
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("event"))
                .unwrap_or(false)
            {
                // Try to open and identify the device
                if let Ok(device) = Device::open(&path) {
                    // Check if it's a keyboard or mouse
                    let is_keyboard = device
                        .supported_keys()
                        .map(|keys| keys.iter().count() > 50)
                        .unwrap_or(false);

                    let is_mouse = device
                        .supported_relative_axes()
                        .map(|_| true) // If it has relative axes support, it's likely a mouse
                        .unwrap_or(false);

                    if is_keyboard || is_mouse {
                        let name = device.name().unwrap_or("unknown");
                        tracing::debug!("Found input device: {} ({:?})", name, path);
                        devices.push(path);
                    }
                }
            }
        }

        Ok(devices)
    }

    /// Capture events from a single device
    fn capture_device(
        path: &Path,
        running: Arc<AtomicBool>,
        event_count: Arc<AtomicUsize>,
        callback: Arc<dyn Fn(EvdevDriverEvent) + Send + Sync>,
    ) -> Result<()> {
        let mut device = Device::open(path).context("Failed to open input device")?;

        tracing::info!(
            "Opened device: {} ({:?})",
            device.name().unwrap_or("unknown"),
            path
        );

        // Grab the device exclusively to prevent events from being processed by other handlers
        device.grab()?;

        tracing::info!(
            "Grabbed device: {}, now capturing events...",
            device.name().unwrap_or("unknown")
        );

        while running.load(Ordering::Relaxed) {
            // Fetch events (no timeout in newer evdev API)
            match device.fetch_events() {
                Ok(events) => {
                    for event in events {
                        let _ = event_count.fetch_add(1, Ordering::Relaxed);
                        tracing::trace!(
                            "Raw evdev event: type={:?}, code={}, value={}",
                            event.event_type(),
                            event.code(),
                            event.value()
                        );

                        match event.event_type() {
                            EventType::KEY => {
                                let keycode = event.code() as u32;
                                let pressed = event.value() > 0;

                                callback(EvdevDriverEvent::Key { keycode, pressed });
                            }
                            EventType::RELATIVE => {
                                let code = event.code();
                                let value = event.value() as i32;

                                // Match by code value for relative axes
                                match code {
                                    0x00 => {
                                        // REL_X
                                        callback(EvdevDriverEvent::MouseMove { x: value, y: 0 });
                                    }
                                    0x01 => {
                                        // REL_Y
                                        callback(EvdevDriverEvent::MouseMove { x: 0, y: value });
                                    }
                                    0x08 => {
                                        // REL_WHEEL
                                        callback(EvdevDriverEvent::MouseWheel {
                                            delta_x: 0,
                                            delta_y: value,
                                        });
                                    }
                                    0x09 => {
                                        // REL_HWHEEL
                                        callback(EvdevDriverEvent::MouseWheel {
                                            delta_x: value,
                                            delta_y: 0,
                                        });
                                    }
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        continue;
                    }
                    tracing::debug!("Error fetching events: {:?}", e);
                    break;
                }
            }
        }

        // Ungrab the device
        device.ungrab()?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::Relaxed);

        for handle in self.thread_handles.drain(..) {
            let _ = handle.join();
        }

        tracing::info!("Evdev input listener stopped");
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn event_count(&self) -> usize {
        self.event_count.load(Ordering::Relaxed)
    }
}

impl Drop for EvdevInputListener {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// ============================================================================
// Driver-level Input Injection (UInput)
// ============================================================================

/// UInput virtual device for driver-level input injection
pub struct UInputInjector {
    keyboard_device: Option<VirtualDevice>,
    mouse_device: Option<VirtualDevice>,
    active: bool,
}

/// Wrapper around evdev::uinput::VirtualDevice
struct VirtualDevice {
    device: evdev::uinput::VirtualDevice,
    active: bool,
}

unsafe impl Send for VirtualDevice {}

impl UInputInjector {
    pub fn new() -> Result<Self> {
        Ok(Self {
            keyboard_device: None,
            mouse_device: None,
            active: false,
        })
    }

    pub fn activate(&mut self) -> Result<()> {
        // Create virtual keyboard
        self.keyboard_device = Some(Self::create_virtual_keyboard()?);

        // Create virtual mouse
        self.mouse_device = Some(Self::create_virtual_mouse()?);

        self.active = true;
        tracing::info!("UInput injector activated");
        Ok(())
    }

    pub fn deactivate(&mut self) -> Result<()> {
        self.keyboard_device = None;
        self.mouse_device = None;
        self.active = false;
        tracing::info!("UInput injector deactivated");
        Ok(())
    }

    fn create_virtual_keyboard() -> Result<VirtualDevice> {
        use evdev::uinput::VirtualDeviceBuilder;
        use evdev::{AttributeSet, EventType, Key};

        let mut keys = AttributeSet::new();
        // Enable all standard keys
        for key in 0..300u16 {
            keys.insert(Key(key));
        }

        let device = VirtualDeviceBuilder::new()?
            .name(&"R-ShareMouse Virtual Keyboard")
            .with_keys(&keys)?
            .build()
            .context("Failed to create virtual keyboard")?;

        tracing::debug!("Created virtual keyboard");
        Ok(VirtualDevice {
            device,
            active: true,
        })
    }

    fn create_virtual_mouse() -> Result<VirtualDevice> {
        use evdev::uinput::VirtualDeviceBuilder;
        use evdev::{EventType, RelativeAxisType};

        let mut rel_axes = AttributeSet::new();
        // In evdev 0.12, RelativeAxisType values are specific enum variants
        // We'll use the from_bits or similar method if available, otherwise skip detailed config
        rel_axes.insert(RelativeAxisType::REL_X);
        rel_axes.insert(RelativeAxisType::REL_Y);
        rel_axes.insert(RelativeAxisType::REL_WHEEL);
        rel_axes.insert(RelativeAxisType::REL_HWHEEL);

        let device = VirtualDeviceBuilder::new()?
            .name(&"R-ShareMouse Virtual Mouse")
            .with_relative_axes(&rel_axes)?
            .build()
            .context("Failed to create virtual mouse")?;

        tracing::debug!("Created virtual mouse");
        Ok(VirtualDevice {
            device,
            active: true,
        })
    }

    /// Send mouse move event (relative)
    pub fn send_mouse_move(&mut self, dx: i32, dy: i32) -> Result<()> {
        if !self.active {
            return Ok(());
        }

        if let Some(mouse) = &mut self.mouse_device {
            if dx != 0 {
                mouse.emit_relative(RelativeAxisType::REL_X, dx)?;
            }
            if dy != 0 {
                mouse.emit_relative(RelativeAxisType::REL_Y, dy)?;
            }
        }

        Ok(())
    }

    /// Send mouse move event (absolute)
    pub fn send_mouse_move_absolute(&mut self, x: u32, y: u32) -> Result<()> {
        if !self.active {
            return Ok(());
        }

        if let Some(mouse) = &self.mouse_device {
            // For absolute positioning, we need to configure the device with absolute axes
            // This is a simplified implementation
            tracing::debug!("Absolute mouse move to ({}, {})", x, y);
        }

        Ok(())
    }

    /// Send mouse button event
    pub fn send_mouse_button(&mut self, button: u32, press: bool) -> Result<()> {
        if !self.active {
            return Ok(());
        }

        if let Some(mouse) = &mut self.mouse_device {
            let key = match button {
                1 => Key(0x110), // BTN_LEFT
                2 => Key(0x111), // BTN_RIGHT (actually BTN_RIGHT is 0x111)
                3 => Key(0x112), // BTN_MIDDLE
                4 => Key(0x113), // BTN_SIDE
                5 => Key(0x114), // BTN_EXTRA
                _ => Key(button as u16),
            };

            mouse.emit_key(key, press)?;
        }

        Ok(())
    }

    /// Send mouse wheel event
    pub fn send_mouse_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
        if !self.active {
            return Ok(());
        }

        if let Some(mouse) = &mut self.mouse_device {
            if delta_y != 0 {
                mouse.emit_relative(RelativeAxisType::REL_WHEEL, delta_y)?;
            }
            if delta_x != 0 {
                mouse.emit_relative(RelativeAxisType::REL_HWHEEL, delta_x)?;
            }
        }

        Ok(())
    }

    /// Send key event
    pub fn send_key(&mut self, keycode: u32, press: bool) -> Result<()> {
        if !self.active {
            return Ok(());
        }

        if let Some(keyboard) = &mut self.keyboard_device {
            let key = Key(keycode as u16);
            keyboard.emit_key(key, press)?;
        }

        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.active
    }
}

impl VirtualDevice {
    fn emit_relative(&mut self, axis: RelativeAxisType, value: i32) -> Result<()> {
        use evdev::InputEvent;

        // Map RelativeAxisType variants to their code values
        let code = match axis {
            RelativeAxisType::REL_X => 0x00,
            RelativeAxisType::REL_Y => 0x01,
            RelativeAxisType::REL_WHEEL => 0x08,
            RelativeAxisType::REL_HWHEEL => 0x09,
            _ => return Ok(()),
        };

        let event = InputEvent::new(EventType::RELATIVE, code, value);
        self.device
            .emit(&[event])
            .context("Failed to emit relative event")?;
        Ok(())
    }

    fn emit_key(&mut self, key: Key, press: bool) -> Result<()> {
        use evdev::InputEvent;

        let value = if press { 1 } else { 0 };
        // Key is a tuple struct, access the inner value
        let code = key.0;
        let event = InputEvent::new(EventType::KEY, code, value);
        self.device
            .emit(&[event])
            .context("Failed to emit key event")?;
        Ok(())
    }
}

impl Drop for VirtualDevice {
    fn drop(&mut self) {
        // The VirtualDevice will be automatically destroyed when dropped
    }
}

impl Drop for UInputInjector {
    fn drop(&mut self) {
        let _ = self.deactivate();
    }
}

// ============================================================================
// Unified Driver-level Input Handler
// ============================================================================

/// Unified driver-level input handler
pub struct DriverInputHandler {
    display_server: DisplayServer,
    listener: Option<EvdevInputListener>,
    injector: Option<UInputInjector>,
    active: bool,
}

impl DriverInputHandler {
    pub fn new() -> Result<Self> {
        let display_server = DisplayServer::X11; // Assume X11 for now

        Ok(Self {
            display_server,
            listener: None,
            injector: None,
            active: false,
        })
    }

    /// Start both capture and injection
    pub fn start<F>(&mut self, callback: F) -> Result<()>
    where
        F: Fn(EvdevDriverEvent) + Send + Sync + 'static,
    {
        // Start injector
        let mut injector = UInputInjector::new()?;
        injector.activate()?;
        self.injector = Some(injector);

        // Start listener
        let mut listener = EvdevInputListener::new();
        listener.start(callback)?;
        self.listener = Some(listener);

        self.active = true;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(listener) = &mut self.listener {
            listener.stop()?;
        }
        if let Some(injector) = &mut self.injector {
            injector.deactivate()?;
        }
        self.active = false;
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get the injector for sending events
    pub fn injector(&mut self) -> Option<&mut UInputInjector> {
        self.injector.as_mut()
    }

    /// Get the event count from listener
    pub fn event_count(&self) -> usize {
        self.listener.as_ref().map(|l| l.event_count()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_event_creation() {
        let event = EvdevDriverEvent::MouseMove { x: 100, y: 200 };
        match event {
            EvdevDriverEvent::MouseMove { x, y } => {
                assert_eq!(x, 100);
                assert_eq!(y, 200);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_uinput_injector_creation() {
        let injector = UInputInjector::new();
        assert!(injector.is_ok());
        assert!(!injector.unwrap().is_active());
    }
}
