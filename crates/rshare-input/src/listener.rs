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
                            let mouse_button = rdev_button_to_mouse_button(button);
                            let input_event =
                                InputEvent::mouse_button(mouse_button, ButtonState::Pressed);
                            let _ = channel.send(input_event);
                        }
                    }
                    EventType::ButtonRelease(button) => {
                        if config.capture_mouse {
                            let mouse_button = rdev_button_to_mouse_button(button);
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

    /// Start listening on a dedicated OS thread.
    ///
    /// This is used by synchronous backend adapters that cannot await the
    /// async `start` method. The rdev listener itself is blocking, so keeping it
    /// on a dedicated thread also avoids blocking a Tokio worker.
    pub fn start_background_thread(&self) -> Result<std::thread::JoinHandle<()>> {
        {
            let mut running = self.running.blocking_lock();
            if *running {
                return Ok(std::thread::spawn(|| {}));
            }
            *running = true;
        }

        tracing::info!("RDev input listener starting on background thread");

        let running = self.running.clone();
        let channel = self.channel.clone();
        let config = self.config.clone();
        let last_mouse_time = self.last_mouse_time.clone();
        let last_mouse_pos = self.last_mouse_pos.clone();

        let handle = std::thread::Builder::new()
            .name("rshare-rdev-input-listener".to_string())
            .spawn(move || {
                use rdev::{listen, Event, EventType};

                let callback = move |event: Event| {
                    if !*running.blocking_lock() {
                        return;
                    }

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
                                    let _ =
                                        channel.send(InputEvent::mouse_move(x as i32, y as i32));
                                }

                                *last_mouse_pos.blocking_lock() = Some((x as i32, y as i32));
                            }
                        }
                        EventType::ButtonPress(button) => {
                            if config.capture_mouse {
                                let mouse_button = rdev_button_to_mouse_button(button);
                                let _ = channel.send(InputEvent::mouse_button(
                                    mouse_button,
                                    ButtonState::Pressed,
                                ));
                            }
                        }
                        EventType::ButtonRelease(button) => {
                            if config.capture_mouse {
                                let mouse_button = rdev_button_to_mouse_button(button);
                                let _ = channel.send(InputEvent::mouse_button(
                                    mouse_button,
                                    ButtonState::Released,
                                ));
                            }
                        }
                        EventType::Wheel { delta_x, delta_y } => {
                            if config.capture_mouse {
                                let _ = channel
                                    .send(InputEvent::mouse_wheel(delta_x as i32, delta_y as i32));
                            }
                        }
                        EventType::KeyPress(key) => {
                            if config.capture_keyboard {
                                if let Some(key_code) = rdev_key_to_key_code(key) {
                                    let _ = channel
                                        .send(InputEvent::key(key_code, ButtonState::Pressed));
                                }
                            }
                        }
                        EventType::KeyRelease(key) => {
                            if config.capture_keyboard {
                                if let Some(key_code) = rdev_key_to_key_code(key) {
                                    let _ = channel
                                        .send(InputEvent::key(key_code, ButtonState::Released));
                                }
                            }
                        }
                    }
                };

                if let Err(error) = listen(callback) {
                    tracing::error!("RDev listen error: {:?}", error);
                }
            })
            .map_err(|error| anyhow::anyhow!("Failed to spawn rdev listener thread: {error}"))?;

        Ok(handle)
    }

    /// Stop listening
    pub async fn stop(&self) -> Result<()> {
        let mut running = self.running.lock().await;
        *running = false;
        tracing::info!("RDev input listener stopped");
        Ok(())
    }

    /// Stop listening from synchronous adapter code.
    pub fn stop_blocking(&self) -> Result<()> {
        let mut running = self.running.blocking_lock();
        *running = false;
        tracing::info!("RDev input listener stopped");
        Ok(())
    }

    /// Check if running
    pub async fn is_running(&self) -> bool {
        *self.running.lock().await
    }

    /// Check running state from synchronous adapter code.
    pub fn is_running_blocking(&self) -> bool {
        *self.running.blocking_lock()
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

fn rdev_button_to_mouse_button(button: rdev::Button) -> MouseButton {
    match button {
        rdev::Button::Left => MouseButton::Left,
        rdev::Button::Right => MouseButton::Right,
        rdev::Button::Middle => MouseButton::Middle,
        // rdev reports Windows XBUTTON1/XBUTTON2 as Unknown(1/2). Some Linux
        // stacks surface browser side buttons as 8/9.
        rdev::Button::Unknown(1) | rdev::Button::Unknown(8) => MouseButton::Back,
        rdev::Button::Unknown(2) | rdev::Button::Unknown(9) => MouseButton::Forward,
        rdev::Button::Unknown(code) => MouseButton::Other(code),
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
        Key::PrintScreen => KeyCode::Raw(0x2C),
        Key::ScrollLock => KeyCode::Raw(0x91),
        Key::Pause => KeyCode::Raw(0x13),
        Key::BackQuote => KeyCode::Raw(0xC0),
        Key::Num1 => KeyCode::Char(b'1'),
        Key::Num2 => KeyCode::Char(b'2'),
        Key::Num3 => KeyCode::Char(b'3'),
        Key::Num4 => KeyCode::Char(b'4'),
        Key::Num5 => KeyCode::Char(b'5'),
        Key::Num6 => KeyCode::Char(b'6'),
        Key::Num7 => KeyCode::Char(b'7'),
        Key::Num8 => KeyCode::Char(b'8'),
        Key::Num9 => KeyCode::Char(b'9'),
        Key::Num0 => KeyCode::Char(b'0'),
        Key::Minus => KeyCode::Raw(0xBD),
        Key::Equal => KeyCode::Raw(0xBB),
        Key::KeyQ => KeyCode::Char(b'Q'),
        Key::KeyW => KeyCode::Char(b'W'),
        Key::KeyE => KeyCode::Char(b'E'),
        Key::KeyR => KeyCode::Char(b'R'),
        Key::KeyT => KeyCode::Char(b'T'),
        Key::KeyY => KeyCode::Char(b'Y'),
        Key::KeyU => KeyCode::Char(b'U'),
        Key::KeyI => KeyCode::Char(b'I'),
        Key::KeyO => KeyCode::Char(b'O'),
        Key::KeyP => KeyCode::Char(b'P'),
        Key::LeftBracket => KeyCode::Raw(0xDB),
        Key::RightBracket => KeyCode::Raw(0xDD),
        Key::KeyA => KeyCode::Char(b'A'),
        Key::KeyS => KeyCode::Char(b'S'),
        Key::KeyD => KeyCode::Char(b'D'),
        Key::KeyF => KeyCode::Char(b'F'),
        Key::KeyG => KeyCode::Char(b'G'),
        Key::KeyH => KeyCode::Char(b'H'),
        Key::KeyJ => KeyCode::Char(b'J'),
        Key::KeyK => KeyCode::Char(b'K'),
        Key::KeyL => KeyCode::Char(b'L'),
        Key::SemiColon => KeyCode::Raw(0xBA),
        Key::Quote => KeyCode::Raw(0xDE),
        Key::BackSlash | Key::IntlBackslash => KeyCode::Raw(0xDC),
        Key::KeyZ => KeyCode::Char(b'Z'),
        Key::KeyX => KeyCode::Char(b'X'),
        Key::KeyC => KeyCode::Char(b'C'),
        Key::KeyV => KeyCode::Char(b'V'),
        Key::KeyB => KeyCode::Char(b'B'),
        Key::KeyN => KeyCode::Char(b'N'),
        Key::KeyM => KeyCode::Char(b'M'),
        Key::Comma => KeyCode::Raw(0xBC),
        Key::Dot => KeyCode::Raw(0xBE),
        Key::Slash => KeyCode::Raw(0xBF),
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
        Key::KpReturn => KeyCode::KeypadEnter,
        Key::KpMinus => KeyCode::KeypadSubtract,
        Key::KpPlus => KeyCode::KeypadAdd,
        Key::KpDivide => KeyCode::KeypadDivide,
        Key::KpDelete => KeyCode::KeypadDecimal,
        Key::Unknown(code) => KeyCode::Raw(code),
        _ => return None,
    })
}

/// Default input listener implementation (synchronous callback-based)
pub struct DefaultInputListener {
    config: ListenerConfig,
    running: bool,
    #[cfg(all(target_os = "windows", not(test)))]
    windows_listener: Option<rshare_platform::WindowsInputListener>,
    #[cfg(all(target_os = "macos", not(test)))]
    macos_listener: Option<rshare_platform::MacosInputListener>,
}

impl DefaultInputListener {
    pub fn new() -> Self {
        Self {
            config: ListenerConfig::default(),
            running: false,
            #[cfg(all(target_os = "windows", not(test)))]
            windows_listener: None,
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

        #[cfg(all(target_os = "windows", not(test)))]
        {
            use std::sync::{Arc, Mutex as StdMutex};

            tracing::info!("Input listener starting (using native Windows low-level hooks)");
            let callback = Arc::new(StdMutex::new(_callback));
            let mut listener = rshare_platform::WindowsInputListener::new();
            listener.start_with_callback(move |event| {
                let input_event = InputEvent::from_windows_event(event);
                if let Ok(callback) = callback.lock() {
                    callback(input_event);
                }
            })?;
            self.windows_listener = Some(listener);
            self.running = true;
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

        #[cfg(not(any(
            all(target_os = "windows", not(test)),
            all(target_os = "macos", not(test))
        )))]
        {
            tracing::info!("Input listener starting (using RDev)");
            self.running = true;

            // TODO: Register platform-specific input hooks
            // For now, recommend using RDevInputListener instead

            Ok(())
        }
    }

    fn stop(&mut self) -> Result<()> {
        #[cfg(all(target_os = "windows", not(test)))]
        {
            if let Some(listener) = self.windows_listener.as_mut() {
                listener.stop()?;
            }
            self.windows_listener = None;
        }

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
    fn rdev_listener_maps_alphanumeric_keys() {
        assert_eq!(
            rdev_key_to_key_code(rdev::Key::KeyA),
            Some(KeyCode::Char(b'A'))
        );
        assert_eq!(
            rdev_key_to_key_code(rdev::Key::KeyZ),
            Some(KeyCode::Char(b'Z'))
        );
        assert_eq!(
            rdev_key_to_key_code(rdev::Key::Num1),
            Some(KeyCode::Char(b'1'))
        );
        assert_eq!(
            rdev_key_to_key_code(rdev::Key::Num0),
            Some(KeyCode::Char(b'0'))
        );
    }

    #[test]
    fn rdev_listener_maps_keyboard_punctuation_and_keypad() {
        assert_eq!(
            rdev_key_to_key_code(rdev::Key::BackQuote),
            Some(KeyCode::Raw(0xC0))
        );
        assert_eq!(
            rdev_key_to_key_code(rdev::Key::Minus),
            Some(KeyCode::Raw(0xBD))
        );
        assert_eq!(
            rdev_key_to_key_code(rdev::Key::Equal),
            Some(KeyCode::Raw(0xBB))
        );
        assert_eq!(
            rdev_key_to_key_code(rdev::Key::KpReturn),
            Some(KeyCode::KeypadEnter)
        );
        assert_eq!(
            rdev_key_to_key_code(rdev::Key::KpDelete),
            Some(KeyCode::KeypadDecimal)
        );
    }

    #[test]
    fn rdev_listener_maps_mouse_side_buttons() {
        assert_eq!(
            rdev_button_to_mouse_button(rdev::Button::Left),
            MouseButton::Left
        );
        assert_eq!(
            rdev_button_to_mouse_button(rdev::Button::Middle),
            MouseButton::Middle
        );
        assert_eq!(
            rdev_button_to_mouse_button(rdev::Button::Unknown(1)),
            MouseButton::Back
        );
        assert_eq!(
            rdev_button_to_mouse_button(rdev::Button::Unknown(2)),
            MouseButton::Forward
        );
        assert_eq!(
            rdev_button_to_mouse_button(rdev::Button::Unknown(42)),
            MouseButton::Other(42)
        );
    }

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

    #[cfg(target_os = "windows")]
    #[test]
    fn capture_falls_back_to_portable_when_windows_hook_setup_fails() {
        use crate::selection::{BackendCandidate, BackendSelector};

        // When Windows-native is unhealthy, selector should pick portable
        let candidates = vec![
            BackendCandidate::unhealthy(
                rshare_core::BackendKind::WindowsNative,
                rshare_core::BackendFailureReason::InitializationFailed,
            ),
            BackendCandidate::healthy(rshare_core::BackendKind::Portable),
        ];

        let selector = BackendSelector::new();
        let result = selector.select(&candidates);

        assert!(result.is_some());
        assert_eq!(result.unwrap().kind, rshare_core::BackendKind::Portable);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_native_capture_reports_correct_kind() {
        use crate::backend::{CaptureBackend, WindowsNativeCaptureBackend};

        let backend = WindowsNativeCaptureBackend::new();
        assert_eq!(backend.kind(), rshare_core::BackendKind::WindowsNative);
    }
}
