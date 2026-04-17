//! Input listener - captures local input events using rdev

use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::sync::Mutex;

use crate::events::{ButtonState, InputEvent, KeyCode, MouseButton};

/// Callback for input events
pub type InputCallback = Box<dyn Fn(InputEvent) + Send>;

/// Input listener trait
pub trait InputListener {
    /// Start listening for input events
    fn start(&mut self, callback: InputCallback) -> Result<()>;

    /// Stop listening for input events
    fn stop(&mut self) -> Result<()>;

    /// Check if listener is running
    fn is_running(&self) -> bool;
}

/// Configuration for input listener
#[derive(Debug, Clone)]
pub struct ListenerConfig {
    /// Whether to capture mouse input
    pub capture_mouse: bool,
    /// Whether to capture keyboard input
    pub capture_keyboard: bool,
    /// Debounce delay for mouse events (reduces event frequency)
    pub mouse_debounce: Duration,
    /// Edge detection threshold in pixels
    pub edge_threshold: u32,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            capture_mouse: true,
            capture_keyboard: true,
            mouse_debounce: Duration::from_millis(5),
            edge_threshold: 5, // pixels
        }
    }
}

/// Input event channel for async processing
#[derive(Debug, Clone)]
pub struct InputEventChannel {
    tx: mpsc::UnboundedSender<InputEvent>,
}

impl InputEventChannel {
    /// Create a new channel
    pub fn new() -> (Self, mpsc::UnboundedReceiver<InputEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Send an event
    pub fn send(&self, event: InputEvent) -> Result<()> {
        self.tx
            .send(event)
            .map_err(|e| anyhow::anyhow!("Failed to send event: {}", e))
    }

    /// Check if channel is closed
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

impl Default for InputEventChannel {
    fn default() -> Self {
        let (tx, _) = mpsc::unbounded_channel();
        Self { tx }
    }
}

/// RDev-based input listener (cross-platform)
pub struct RDevInputListener {
    config: ListenerConfig,
    running: Arc<Mutex<bool>>,
    channel: InputEventChannel,
    _rx: Option<mpsc::UnboundedReceiver<InputEvent>>,
    last_mouse_time: Arc<Mutex<Instant>>,
    last_mouse_pos: Arc<Mutex<Option<(i32, i32)>>>,
}

impl RDevInputListener {
    /// Create a new RDev-based input listener
    pub fn new() -> Self {
        let (channel, rx) = InputEventChannel::new();
        Self {
            config: ListenerConfig::default(),
            running: Arc::new(Mutex::new(false)),
            channel,
            _rx: Some(rx),
            last_mouse_time: Arc::new(Mutex::new(Instant::now())),
            last_mouse_pos: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the configuration
    pub fn with_config(mut self, config: ListenerConfig) -> Self {
        self.config = config;
        self
    }

    /// Get the event receiver
    pub fn receiver(&mut self) -> mpsc::UnboundedReceiver<InputEvent> {
        self._rx.take().expect("Receiver already taken")
    }

    /// Get a clone of the event channel
    pub fn channel(&self) -> InputEventChannel {
        self.channel.clone()
    }

    /// Start listening
    pub async fn start(&self) -> Result<()> {
        let mut running = self.running.lock().await;
        if *running {
            return Ok(());
        }
        *running = true;
        drop(running);

        tracing::info!("RDev input listener starting");

        let running = self.running.clone();
        let channel = self.channel.clone();
        let config = self.config.clone();
        let last_mouse_time = self.last_mouse_time.clone();
        let last_mouse_pos = self.last_mouse_pos.clone();

        // Spawn the rdev listener in a blocking task
        tokio::task::spawn_blocking(move || {
            use rdev::{listen, Event, EventType};

            let callback = move |event: Event| {
                // Check if we should stop
                if !*running.blocking_lock() {
                    return;
                }

                // Process the event based on config
                match event.event_type {
                    EventType::MouseMove { x, y } => {
                        if config.capture_mouse {
                            let now = Instant::now();
                            let should_send = {
                                let mut last_time = last_mouse_time.blocking_lock();
                                let elapsed = now.saturating_duration_since(*last_time);
                                if elapsed >= config.mouse_debounce {
                                    *last_time = now;
                                    true
                                } else {
                                    false
                                }
                            };

                            if should_send {
                                let input_event = InputEvent::mouse_move(x as i32, y as i32);
                                let _ = channel.send(input_event);
                            }

                            *last_mouse_pos.blocking_lock() = Some((x as i32, y as i32));
                        }
                    }
                    EventType::ButtonPress(button) => {
                        if config.capture_mouse {
                            let mouse_button = match button {
                                rdev::Button::Left => MouseButton::Left,
                                rdev::Button::Right => MouseButton::Right,
                                rdev::Button::Middle => MouseButton::Middle,
                                _ => MouseButton::Other(0),
                            };
                            let input_event =
                                InputEvent::mouse_button(mouse_button, ButtonState::Pressed);
                            let _ = channel.send(input_event);
                        }
                    }
                    EventType::ButtonRelease(button) => {
                        if config.capture_mouse {
                            let mouse_button = match button {
                                rdev::Button::Left => MouseButton::Left,
                                rdev::Button::Right => MouseButton::Right,
                                rdev::Button::Middle => MouseButton::Middle,
                                _ => MouseButton::Other(0),
                            };
                            let input_event =
                                InputEvent::mouse_button(mouse_button, ButtonState::Released);
                            let _ = channel.send(input_event);
                        }
                    }
                    EventType::Wheel { delta_x, delta_y } => {
                        if config.capture_mouse {
                            let input_event =
                                InputEvent::mouse_wheel(delta_x as i32, delta_y as i32);
                            let _ = channel.send(input_event);
                        }
                    }
                    EventType::KeyPress(key) => {
                        if config.capture_keyboard {
                            if let Some(key_code) = rdev_key_to_key_code(key) {
                                let input_event = InputEvent::key(key_code, ButtonState::Pressed);
                                let _ = channel.send(input_event);
                            }
                        }
                    }
                    EventType::KeyRelease(key) => {
                        if config.capture_keyboard {
                            if let Some(key_code) = rdev_key_to_key_code(key) {
                                let input_event = InputEvent::key(key_code, ButtonState::Released);
                                let _ = channel.send(input_event);
                            }
                        }
                    }
                }
            };

            if let Err(e) = listen(callback) {
                tracing::error!("RDev listen error: {:?}", e);
            }
        });

        Ok(())
    }

    /// Stop listening
    pub async fn stop(&self) -> Result<()> {
        let mut running = self.running.lock().await;
        *running = false;
        tracing::info!("RDev input listener stopped");
        Ok(())
    }

    /// Check if running
    pub async fn is_running(&self) -> bool {
        *self.running.lock().await
    }

    /// Get the last mouse position
    pub async fn last_mouse_position(&self) -> Option<(i32, i32)> {
        *self.last_mouse_pos.lock().await
    }
}

impl Default for RDevInputListener {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert rdev Key to KeyCode
fn rdev_key_to_key_code(key: rdev::Key) -> Option<KeyCode> {
    use rdev::Key;

    Some(match key {
        Key::Alt => KeyCode::AltLeft,
        Key::Backspace => KeyCode::Backspace,
        Key::CapsLock => KeyCode::CapsLock,
        Key::ControlLeft => KeyCode::ControlLeft,
        Key::ControlRight => KeyCode::ControlRight,
        Key::DownArrow => KeyCode::Down,
        Key::Escape => KeyCode::Escape,
        Key::F1 => KeyCode::F1,
        Key::F2 => KeyCode::F2,
        Key::F3 => KeyCode::F3,
        Key::F4 => KeyCode::F4,
        Key::F5 => KeyCode::F5,
        Key::F6 => KeyCode::F6,
        Key::F7 => KeyCode::F7,
        Key::F8 => KeyCode::F8,
        Key::F9 => KeyCode::F9,
        Key::F10 => KeyCode::F10,
        Key::F11 => KeyCode::F11,
        Key::F12 => KeyCode::F12,
        Key::LeftArrow => KeyCode::Left,
        Key::RightArrow => KeyCode::Right,
        Key::ShiftLeft => KeyCode::ShiftLeft,
        Key::ShiftRight => KeyCode::ShiftRight,
        Key::UpArrow => KeyCode::Up,
        Key::MetaLeft => KeyCode::SuperLeft,
        Key::MetaRight => KeyCode::SuperRight,
        Key::Tab => KeyCode::Tab,
        Key::Return => KeyCode::Enter,
        Key::Space => KeyCode::Space,
        Key::NumLock => KeyCode::NumLock,
        Key::Home => KeyCode::Home,
        Key::End => KeyCode::End,
        Key::PageUp => KeyCode::PageUp,
        Key::PageDown => KeyCode::PageDown,
        Key::Insert => KeyCode::Insert,
        Key::Delete => KeyCode::Delete,
        Key::Kp0 => KeyCode::Keypad0,
        Key::Kp1 => KeyCode::Keypad1,
        Key::Kp2 => KeyCode::Keypad2,
        Key::Kp3 => KeyCode::Keypad3,
        Key::Kp4 => KeyCode::Keypad4,
        Key::Kp5 => KeyCode::Keypad5,
        Key::Kp6 => KeyCode::Keypad6,
        Key::Kp7 => KeyCode::Keypad7,
        Key::Kp8 => KeyCode::Keypad8,
        Key::Kp9 => KeyCode::Keypad9,
        Key::KpMultiply => KeyCode::KeypadMultiply,
        Key::Unknown(code) => KeyCode::Raw(code),
        _ => return None,
    })
}

/// Default input listener implementation (synchronous callback-based)
pub struct DefaultInputListener {
    config: ListenerConfig,
    running: bool,
    #[cfg(all(target_os = "macos", not(test)))]
    macos_listener: Option<rshare_platform::MacosInputListener>,
}

impl DefaultInputListener {
    pub fn new() -> Self {
        Self {
            config: ListenerConfig::default(),
            running: false,
            #[cfg(all(target_os = "macos", not(test)))]
            macos_listener: None,
        }
    }

    pub fn with_config(mut self, config: ListenerConfig) -> Self {
        self.config = config;
        self
    }
}

impl Default for DefaultInputListener {
    fn default() -> Self {
        Self::new()
    }
}

impl InputListener for DefaultInputListener {
    fn start(&mut self, _callback: InputCallback) -> Result<()> {
        if self.running {
            return Ok(());
        }

        #[cfg(all(target_os = "macos", not(test)))]
        {
            use std::sync::{Arc, Mutex as StdMutex};

            tracing::info!("Input listener starting (using native macOS CGEventTap)");
            let callback = Arc::new(StdMutex::new(_callback));
            let mut listener = rshare_platform::MacosInputListener::new();
            listener.start_with_callback(move |event| {
                let input_event = InputEvent::from_macos_event(event);
                if let Ok(callback) = callback.lock() {
                    callback(input_event);
                }
            })?;
            self.macos_listener = Some(listener);
            self.running = true;
            return Ok(());
        }

        #[cfg(not(all(target_os = "macos", not(test))))]
        {
            tracing::info!("Input listener starting (using RDev)");
            self.running = true;

            // TODO: Register platform-specific input hooks
            // For now, recommend using RDevInputListener instead

            Ok(())
        }
    }

    fn stop(&mut self) -> Result<()> {
        #[cfg(all(target_os = "macos", not(test)))]
        {
            if let Some(listener) = self.macos_listener.as_mut() {
                listener.stop()?;
            }
            self.macos_listener = None;
        }

        self.running = false;
        tracing::info!("Input listener stopped");
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_listener_config_default() {
        let config = ListenerConfig::default();
        assert!(config.capture_mouse);
        assert!(config.capture_keyboard);
    }

    #[test]
    fn test_rdev_listener_new() {
        let listener = RDevInputListener::new();
        assert!(listener._rx.is_some());
    }

    #[tokio::test]
    async fn test_channel_send() {
        let (channel, mut rx) = InputEventChannel::new();

        let event = InputEvent::mouse_move(100, 200);
        channel.send(event.clone()).unwrap();

        let received = rx.recv().await.unwrap();
        match received {
            InputEvent::MouseMove { x, y } => {
                assert_eq!(x, 100);
                assert_eq!(y, 200);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_default_listener() {
        let mut listener = DefaultInputListener::new();
        assert!(!listener.is_running());

        let callback = Box::new(|_event| {});
        let result = listener.start(callback);
        assert!(result.is_ok());
    }
}
