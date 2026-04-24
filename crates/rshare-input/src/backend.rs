//! Input backend traits and adapter implementations
//!
//! This module defines the core backend abstractions for input capture
//! and injection, along with portable adapters that wrap existing implementations.

use crate::emulator::InputEmulator;
use crate::events::InputEvent;
use crate::listener::RDevInputListener;
use anyhow::Result;
use std::fmt::{self, Debug};

#[cfg(target_os = "macos")]
type PortableInputEmulator = crate::emulator::MacosNativeInputEmulator;

#[cfg(not(target_os = "macos"))]
type PortableInputEmulator = crate::emulator::EnigoInputEmulator;

/// The kind of backend.
pub type BackendKind = rshare_core::BackendKind;

/// Backend health status.
pub type BackendHealth = rshare_core::BackendHealth;

/// Reason for backend failure.
pub type BackendFailureReason = rshare_core::BackendFailureReason;

/// Trait for input event capture backends.
pub trait CaptureBackend: Debug + Send + Sync {
    /// Get the kind of this backend.
    fn kind(&self) -> BackendKind;

    /// Check if the backend is currently healthy.
    fn health(&self) -> BackendHealth;

    /// Start capturing input events.
    fn start(&mut self) -> Result<()>;

    /// Stop capturing input events.
    fn stop(&mut self) -> Result<()>;

    /// Check if currently capturing.
    fn is_running(&self) -> bool;
}

/// Trait for input event injection backends.
pub trait InjectBackend: Debug + Send + Sync {
    /// Get the kind of this backend.
    fn kind(&self) -> BackendKind;

    /// Check if the backend is currently healthy.
    fn health(&self) -> BackendHealth;

    /// Inject an input event.
    fn inject(&mut self, event: InputEvent) -> Result<()>;

    /// Check if currently active.
    fn is_active(&self) -> bool;
}

/// Trait for privilege/session state tracking backends.
pub trait PrivilegeBackend: Debug + Send + Sync {
    /// Get the current privilege/session state.
    fn current_state(&self) -> rshare_core::PrivilegeState;
}

trait CaptureDriver: Debug + Send + Sync {
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn is_running(&self) -> bool;
}

struct RDevCaptureDriver {
    listener: RDevInputListener,
    _thread: Option<std::thread::JoinHandle<()>>,
}

impl Debug for RDevCaptureDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RDevCaptureDriver")
            .field("running", &self.is_running())
            .finish()
    }
}

impl RDevCaptureDriver {
    fn new() -> Self {
        Self {
            listener: RDevInputListener::new(),
            _thread: None,
        }
    }
}

impl CaptureDriver for RDevCaptureDriver {
    fn start(&mut self) -> Result<()> {
        if self.listener.is_running_blocking() {
            return Ok(());
        }

        self._thread = Some(self.listener.start_background_thread()?);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.listener.stop_blocking()
    }

    fn is_running(&self) -> bool {
        self.listener.is_running_blocking()
    }
}

/// Capabilities that a backend may support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// Supports low-level hooking.
    pub low_level_hook: bool,
    /// Supports send/input injection.
    pub send_input: bool,
    /// Supports secure desktop injection.
    pub secure_desktop: bool,
    /// Requires elevated privileges.
    pub requires_elevation: bool,
}

impl Default for BackendCapabilities {
    fn default() -> Self {
        Self {
            low_level_hook: false,
            send_input: false,
            secure_desktop: false,
            requires_elevation: false,
        }
    }
}

/// Portable capture backend using rdev.
///
/// Uses rdev's blocking listener on a dedicated thread.
pub struct PortableCaptureBackend {
    driver: Box<dyn CaptureDriver>,
    health: BackendHealth,
}

impl PortableCaptureBackend {
    /// Create a new portable capture backend.
    pub fn new() -> Self {
        Self {
            driver: Box::new(RDevCaptureDriver::new()),
            health: BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
        }
    }

    #[cfg(test)]
    fn new_with_driver_for_test(driver: Box<dyn CaptureDriver>) -> Self {
        Self {
            driver,
            health: BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
        }
    }

    /// Create a new portable capture backend for testing.
    ///
    /// Returns a backend with Healthy status for availability detection.
    /// The backend must still be started before it will capture events.
    pub fn new_for_test() -> Result<Self> {
        let mut backend = Self::new();
        backend.health = BackendHealth::Healthy;
        Ok(backend)
    }
}

impl Debug for PortableCaptureBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PortableCaptureBackend")
            .field("running", &self.driver.is_running())
            .field("health", &self.health)
            .finish()
    }
}

impl Default for PortableCaptureBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureBackend for PortableCaptureBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Portable
    }

    fn health(&self) -> BackendHealth {
        self.health.clone()
    }

    fn start(&mut self) -> Result<()> {
        self.driver.start()?;
        if !self.driver.is_running() {
            self.health = BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            };
            anyhow::bail!("Portable capture driver did not enter running state");
        }
        self.health = BackendHealth::Healthy;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.driver.stop()?;
        self.health = BackendHealth::Degraded {
            reason: BackendFailureReason::Unavailable,
        };
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.driver.is_running()
    }
}

/// Portable injection backend.
pub struct PortableInjectBackend {
    emulator: PortableInputEmulator,
    health: BackendHealth,
}

impl PortableInjectBackend {
    /// Create a new portable injection backend.
    pub fn new() -> Result<Self> {
        let mut emulator = PortableInputEmulator::new()?;
        emulator.activate()?;

        Ok(Self {
            emulator,
            health: BackendHealth::Healthy,
        })
    }

    /// Create a new portable injection backend for testing.
    pub fn new_for_test() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            return Ok(Self {
                emulator: PortableInputEmulator::new_for_test()?,
                health: BackendHealth::Healthy,
            });
        }

        #[cfg(not(target_os = "macos"))]
        Self::new()
    }

    /// Activate the backend and underlying emulator.
    pub fn activate(&mut self) -> Result<()> {
        self.emulator.activate()?;
        self.health = BackendHealth::Healthy;
        Ok(())
    }

    /// Deactivate the backend.
    pub fn deactivate(&mut self) -> Result<()> {
        self.emulator.deactivate()?;
        Ok(())
    }
}

impl Debug for PortableInjectBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PortableInjectBackend")
            .field("active", &self.emulator.is_active())
            .field("health", &self.health)
            .finish()
    }
}

impl Default for PortableInjectBackend {
    fn default() -> Self {
        Self::new().expect("Failed to create PortableInjectBackend")
    }
}

impl InjectBackend for PortableInjectBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Portable
    }

    fn health(&self) -> BackendHealth {
        self.health.clone()
    }

    fn inject(&mut self, event: InputEvent) -> Result<()> {
        if !self.emulator.is_active() {
            anyhow::bail!("Portable inject backend is not active");
        }

        self.emulator.emulate(event)
    }

    fn is_active(&self) -> bool {
        self.emulator.is_active()
    }
}

/// No-op privilege backend for platforms without privilege tracking.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopPrivilegeBackend;

/// Windows-native injection backend adapter.
#[cfg(target_os = "windows")]
pub struct WindowsNativeInjectBackend {
    emulator: crate::emulator::WindowsNativeInputEmulator,
    health: BackendHealth,
}

#[cfg(target_os = "windows")]
impl WindowsNativeInjectBackend {
    /// Create a new Windows-native injection backend.
    pub fn new() -> Result<Self> {
        let mut emulator = crate::emulator::WindowsNativeInputEmulator::new()?;
        emulator.activate()?;

        Ok(Self {
            emulator,
            health: BackendHealth::Healthy,
        })
    }

    /// Create a new Windows-native injection backend for testing.
    pub fn new_for_test() -> Result<Self> {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
impl Debug for WindowsNativeInjectBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowsNativeInjectBackend")
            .field("health", &self.health)
            .finish()
    }
}

#[cfg(target_os = "windows")]
impl InjectBackend for WindowsNativeInjectBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::WindowsNative
    }

    fn health(&self) -> BackendHealth {
        self.health.clone()
    }

    fn inject(&mut self, event: InputEvent) -> Result<()> {
        if !InputEmulator::is_active(&self.emulator) {
            anyhow::bail!("Windows native inject backend is not active");
        }

        InputEmulator::emulate(&mut self.emulator, event)
    }

    fn is_active(&self) -> bool {
        InputEmulator::is_active(&self.emulator)
    }
}

/// Windows-native capture backend adapter.
///
/// Uses native low-level mouse and keyboard hooks through rshare-platform.
#[cfg(target_os = "windows")]
pub struct WindowsNativeCaptureBackend {
    driver: Box<dyn CaptureDriver>,
    health: BackendHealth,
}

#[cfg(target_os = "windows")]
impl WindowsNativeCaptureBackend {
    /// Create a new Windows-native capture backend.
    pub fn new() -> Self {
        Self {
            driver: Box::new(WindowsNativeCaptureDriver::new()),
            health: BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
        }
    }

    #[cfg(test)]
    fn new_with_driver_for_test(driver: Box<dyn CaptureDriver>) -> Self {
        Self {
            driver,
            health: BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
        }
    }

    /// Create a new Windows-native capture backend for testing.
    pub fn new_for_test() -> Result<Self> {
        Ok(Self::new())
    }
}

#[cfg(target_os = "windows")]
impl Debug for WindowsNativeCaptureBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowsNativeCaptureBackend")
            .field("running", &self.driver.is_running())
            .field("health", &self.health)
            .finish()
    }
}

#[cfg(target_os = "windows")]
impl Default for WindowsNativeCaptureBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
impl CaptureBackend for WindowsNativeCaptureBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::WindowsNative
    }

    fn health(&self) -> BackendHealth {
        self.health.clone()
    }

    fn start(&mut self) -> Result<()> {
        self.driver.start()?;
        if !self.driver.is_running() {
            self.health = BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            };
            anyhow::bail!("Windows native capture driver did not enter running state");
        }
        self.health = BackendHealth::Healthy;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.driver.stop()?;
        self.health = BackendHealth::Degraded {
            reason: BackendFailureReason::Unavailable,
        };
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.driver.is_running()
    }
}

#[cfg(target_os = "windows")]
struct WindowsNativeCaptureDriver {
    listener: rshare_platform::WindowsInputListener,
}

#[cfg(target_os = "windows")]
impl WindowsNativeCaptureDriver {
    fn new() -> Self {
        Self {
            listener: rshare_platform::WindowsInputListener::new(),
        }
    }
}

#[cfg(target_os = "windows")]
impl Debug for WindowsNativeCaptureDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowsNativeCaptureDriver")
            .field("running", &self.is_running())
            .finish()
    }
}

#[cfg(target_os = "windows")]
impl CaptureDriver for WindowsNativeCaptureDriver {
    fn start(&mut self) -> Result<()> {
        self.listener.start()
    }

    fn stop(&mut self) -> Result<()> {
        self.listener.stop()
    }

    fn is_running(&self) -> bool {
        self.listener.is_running()
    }
}

/// Virtual HID injection backend (Windows only).
///
/// This backend uses virtual HID devices to inject input at a lower level.
/// Currently scaffold only - returns Unsupported error.
#[cfg(target_os = "windows")]
pub struct VirtualHidInjectBackend {
    client: rshare_platform::windows::WindowsDriverClient,
    health: BackendHealth,
}

#[cfg(target_os = "windows")]
impl VirtualHidInjectBackend {
    pub fn new() -> Result<Self> {
        let client = rshare_platform::windows::WindowsDriverClient::open_vhid()?;
        let capabilities = client.query_capabilities()?;
        if !capabilities.virtual_keyboard && !capabilities.virtual_mouse {
            anyhow::bail!("RShare Virtual HID driver interface is not active");
        }

        Ok(Self {
            client,
            health: BackendHealth::Healthy,
        })
    }

    pub fn new_for_test() -> Result<Self> {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
impl Debug for VirtualHidInjectBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualHidInjectBackend")
            .field("health", &self.health)
            .finish()
    }
}

#[cfg(target_os = "windows")]
impl InjectBackend for VirtualHidInjectBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::VirtualHid
    }

    fn health(&self) -> BackendHealth {
        self.health.clone()
    }

    fn inject(&mut self, event: InputEvent) -> Result<()> {
        match event {
            InputEvent::Key { keycode, state } | InputEvent::KeyExtended { keycode, state, .. } => {
                self.client
                    .inject_keyboard(keycode.to_raw() as u16, state.is_pressed())
            }
            InputEvent::MouseMove { x, y } => self.client.inject_mouse_move(x, y),
            InputEvent::MouseButton { button, state } => self
                .client
                .inject_mouse_button(button.to_code(), state.is_pressed()),
            InputEvent::MouseWheel { delta_x, delta_y } => {
                self.client.inject_mouse_wheel(delta_x, delta_y)
            }
            _ => anyhow::bail!(
                "Virtual HID driver injection does not support {}",
                event.event_type()
            ),
        }
    }

    fn is_active(&self) -> bool {
        matches!(self.health, BackendHealth::Healthy)
    }
}

/// Virtual HID capture backend (Windows only).
///
/// This backend uses the RShare kernel filter driver to capture input
/// at a lower level than user-mode hooks, providing better reliability
/// and security desktop support.
#[cfg(target_os = "windows")]
pub struct VirtualHidCaptureBackend {
    driver: Box<dyn CaptureDriver>,
    health: BackendHealth,
}

#[cfg(target_os = "windows")]
impl VirtualHidCaptureBackend {
    /// Create a new Virtual HID capture backend.
    ///
    /// Opens the RShare filter driver and verifies event filtering is active.
    pub fn new() -> Self {
        Self {
            driver: Box::new(VirtualHidCaptureDriver::new()),
            health: BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
        }
    }

    #[cfg(test)]
    fn new_with_driver_for_test(driver: Box<dyn CaptureDriver>) -> Self {
        Self {
            driver,
            health: BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
        }
    }

    /// Create a new Virtual HID capture backend for testing.
    pub fn new_for_test() -> Result<Self> {
        Ok(Self::new())
    }
}

#[cfg(target_os = "windows")]
impl Debug for VirtualHidCaptureBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualHidCaptureBackend")
            .field("running", &self.driver.is_running())
            .field("health", &self.health)
            .finish()
    }
}

#[cfg(target_os = "windows")]
impl Default for VirtualHidCaptureBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
impl CaptureBackend for VirtualHidCaptureBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::VirtualHid
    }

    fn health(&self) -> BackendHealth {
        self.health.clone()
    }

    fn start(&mut self) -> Result<()> {
        self.driver.start()?;
        if !self.driver.is_running() {
            self.health = BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            };
            anyhow::bail!("Virtual HID capture driver did not enter running state");
        }
        self.health = BackendHealth::Healthy;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.driver.stop()?;
        self.health = BackendHealth::Degraded {
            reason: BackendFailureReason::Unavailable,
        };
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.driver.is_running()
    }
}

#[cfg(target_os = "windows")]
struct VirtualHidCaptureDriver {
    client: Option<rshare_platform::windows::WindowsDriverClient>,
    thread: Option<std::thread::JoinHandle<()>>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    callback: std::sync::Arc<std::sync::Mutex<Option<DriverEventCallback>>>,
}

#[cfg(target_os = "windows")]
type DriverEventCallback =
    Box<dyn Fn(rshare_platform::windows::WindowsDriverInputEvent) + Send + Sync + 'static>;

#[cfg(target_os = "windows")]
impl VirtualHidCaptureDriver {
    fn new() -> Self {
        Self {
            client: None,
            thread: None,
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            callback: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn start(&mut self) -> Result<()> {
        if self.running.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }

        let client = rshare_platform::windows::WindowsDriverClient::open_filter()
            .map_err(|_| anyhow::anyhow!("Failed to open RShare filter driver"))?;

        let capabilities = client
            .query_capabilities()
            .map_err(|_| anyhow::anyhow!("Failed to query driver capabilities"))?;

        if !capabilities.filter_events {
            anyhow::bail!("RShare filter driver event filtering is not active");
        }

        self.client = Some(client);

        // Start the event reading thread
        let running = self.running.clone();
        let callback = self.callback.clone();

        running.store(true, std::sync::atomic::Ordering::Release);

        let thread = std::thread::Builder::new()
            .name("rshare-vhid-capture".to_string())
            .spawn(move || {
                Self::event_loop(running, callback);
            })?;

        self.thread = Some(thread);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.running
            .store(false, std::sync::atomic::Ordering::Release);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }

        self.client = None;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::Acquire)
    }

    fn event_loop(
        running: std::sync::Arc<std::sync::atomic::AtomicBool>,
        callback: std::sync::Arc<std::sync::Mutex<Option<DriverEventCallback>>>,
    ) {
        let client = match rshare_platform::windows::WindowsDriverClient::open_filter() {
            Ok(client) => client,
            Err(error) => {
                tracing::error!(
                    "Virtual HID capture failed to open filter driver: {}",
                    error
                );
                running.store(false, std::sync::atomic::Ordering::Release);
                return;
            }
        };

        while running.load(std::sync::atomic::Ordering::Acquire) {
            match client.read_event() {
                Ok(event) => {
                    // Only forward hardware events (ignore injected loopback)
                    if matches!(
                        event.source,
                        rshare_platform::windows::WindowsDriverEventSource::Hardware
                    ) {
                        if let Ok(guard) = callback.lock() {
                            if let Some(cb) = guard.as_ref() {
                                cb(event);
                            }
                        }
                    }
                }
                Err(error) => {
                    // Check if queue is empty (normal condition when no events)
                    if rshare_platform::windows::is_driver_event_queue_empty(&error) {
                        // Sleep a bit to avoid busy-waiting
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        continue;
                    }
                    // Real error - log and stop
                    tracing::error!("Virtual HID capture error: {}", error);
                    break;
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl CaptureDriver for VirtualHidCaptureDriver {
    fn start(&mut self) -> Result<()> {
        VirtualHidCaptureDriver::start(self)
    }

    fn stop(&mut self) -> Result<()> {
        VirtualHidCaptureDriver::stop(self)
    }

    fn is_running(&self) -> bool {
        VirtualHidCaptureDriver::is_running(self)
    }
}

#[cfg(target_os = "windows")]
impl Debug for VirtualHidCaptureDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualHidCaptureDriver")
            .field("running", &self.is_running())
            .finish()
    }
}

impl PrivilegeBackend for NoopPrivilegeBackend {
    fn current_state(&self) -> rshare_core::PrivilegeState {
        rshare_core::PrivilegeState::UnlockedDesktop
    }
}

/// Re-export privilege types for convenience
#[cfg(target_os = "windows")]
pub use crate::privilege::WindowsPrivilegeBackend as PrivilegeTracker;

#[cfg(not(target_os = "windows"))]
pub use NoopPrivilegeBackend as PrivilegeTracker;

// ============================================================================
// Linux Driver-level Backends
// ============================================================================

/// Linux evdev capture backend using kernel-level input access.
///
/// This backend reads directly from /dev/input/event* devices for
/// low-latency input capture at the kernel level.
#[cfg(target_os = "linux")]
pub struct EvdevCaptureBackend {
    driver: Box<dyn CaptureDriver>,
    health: BackendHealth,
}

#[cfg(target_os = "linux")]
impl EvdevCaptureBackend {
    /// Create a new evdev capture backend.
    pub fn new() -> Self {
        Self {
            driver: Box::new(EvdevCaptureDriver::new()),
            health: BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
        }
    }

    #[cfg(test)]
    fn new_with_driver_for_test(driver: Box<dyn CaptureDriver>) -> Self {
        Self {
            driver,
            health: BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
        }
    }

    /// Create a new evdev capture backend for testing.
    pub fn new_for_test() -> Result<Self> {
        Ok(Self::new())
    }
}

#[cfg(target_os = "linux")]
impl Debug for EvdevCaptureBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvdevCaptureBackend")
            .field("running", &self.driver.is_running())
            .field("health", &self.health)
            .finish()
    }
}

#[cfg(target_os = "linux")]
impl Default for EvdevCaptureBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl CaptureBackend for EvdevCaptureBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Evdev
    }

    fn health(&self) -> BackendHealth {
        self.health.clone()
    }

    fn start(&mut self) -> Result<()> {
        self.driver.start()?;
        if !self.driver.is_running() {
            self.health = BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            };
            anyhow::bail!("Evdev capture driver did not enter running state");
        }
        self.health = BackendHealth::Healthy;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.driver.stop()?;
        self.health = BackendHealth::Degraded {
            reason: BackendFailureReason::Unavailable,
        };
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.driver.is_running()
    }
}

#[cfg(target_os = "linux")]
struct EvdevCaptureDriver {
    listener: rshare_platform::EvdevInputListener,
    running: bool,
}

#[cfg(target_os = "linux")]
impl EvdevCaptureDriver {
    fn new() -> Self {
        Self {
            listener: rshare_platform::EvdevInputListener::new(),
            running: false,
        }
    }
}

#[cfg(target_os = "linux")]
impl Debug for EvdevCaptureDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvdevCaptureDriver")
            .field("running", &self.running)
            .finish()
    }
}

#[cfg(target_os = "linux")]
impl CaptureDriver for EvdevCaptureDriver {
    fn start(&mut self) -> Result<()> {
        if self.running {
            return Ok(());
        }

        // Start with a dummy callback for now
        self.listener.start(|_event| {})?;
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if !self.running {
            return Ok(());
        }

        self.listener.stop()?;
        self.running = false;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.listener.is_running()
    }
}

/// Linux uinput injection backend using kernel-level virtual devices.
///
/// This backend creates virtual input devices in the kernel for
/// low-latency input injection.
#[cfg(target_os = "linux")]
pub struct UInputInjectBackend {
    injector: rshare_platform::UInputInjector,
    health: BackendHealth,
}

#[cfg(target_os = "linux")]
impl UInputInjectBackend {
    /// Create a new uinput injection backend.
    pub fn new() -> Result<Self> {
        let mut injector = rshare_platform::UInputInjector::new()?;
        injector.activate()?;

        Ok(Self {
            injector,
            health: BackendHealth::Healthy,
        })
    }

    /// Create a new uinput injection backend for testing.
    pub fn new_for_test() -> Result<Self> {
        Self::new()
    }

    /// Activate the backend.
    pub fn activate(&mut self) -> Result<()> {
        self.injector.activate()?;
        self.health = BackendHealth::Healthy;
        Ok(())
    }

    /// Deactivate the backend.
    pub fn deactivate(&mut self) -> Result<()> {
        self.injector.deactivate()?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Debug for UInputInjectBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UInputInjectBackend")
            .field("active", &self.injector.is_active())
            .field("health", &self.health)
            .finish()
    }
}

#[cfg(target_os = "linux")]
impl Default for UInputInjectBackend {
    fn default() -> Self {
        Self::new().expect("Failed to create UInputInjectBackend")
    }
}

#[cfg(target_os = "linux")]
impl InjectBackend for UInputInjectBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::UInput
    }

    fn health(&self) -> BackendHealth {
        self.health.clone()
    }

    fn inject(&mut self, event: InputEvent) -> Result<()> {
        if !self.injector.is_active() {
            anyhow::bail!("UInput inject backend is not active");
        }

        match event {
            InputEvent::MouseMove { x, y } => {
                self.injector.send_mouse_move(x, y)?;
            }
            InputEvent::MouseButton { button, state } => {
                self.injector
                    .send_mouse_button(button.to_code() as u32, state.is_pressed())?;
            }
            InputEvent::MouseWheel { delta_x, delta_y } => {
                self.injector.send_mouse_wheel(delta_x, delta_y)?;
            }
            InputEvent::Key { keycode, state } => {
                self.injector
                    .send_key(keycode.to_raw(), state.is_pressed())?;
            }
            InputEvent::KeyExtended { keycode, state, .. } => {
                self.injector
                    .send_key(keycode.to_raw(), state.is_pressed())?;
            }
            _ => anyhow::bail!("UInput injection does not support {}", event.event_type()),
        }
        Ok(())
    }

    fn is_active(&self) -> bool {
        self.injector.is_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_backend_reports_portable_kind() {
        let backend = PortableInjectBackend::new_for_test().unwrap();
        assert_eq!(backend.kind(), BackendKind::Portable);
    }

    #[test]
    fn portable_capture_backend_reports_portable_kind() {
        let backend = PortableCaptureBackend::new();
        assert_eq!(backend.kind(), BackendKind::Portable);
    }

    #[test]
    fn portable_backend_starts_healthy() {
        let backend = PortableInjectBackend::new_for_test().unwrap();
        assert_eq!(backend.health(), BackendHealth::Healthy);
        assert!(backend.is_active());
    }

    #[test]
    fn portable_capture_backend_starts_degraded() {
        let backend = PortableCaptureBackend::new();
        assert!(matches!(backend.health(), BackendHealth::Degraded { .. }));
    }

    #[test]
    fn portable_capture_start_surfaces_driver_errors() {
        let mut backend =
            PortableCaptureBackend::new_with_driver_for_test(Box::new(FailingCaptureDriver));
        assert!(!backend.is_running());

        let result = backend.start();
        assert!(result.is_err());
        assert!(!backend.is_running());

        backend.stop().unwrap();
        assert!(!backend.is_running());
    }

    #[test]
    fn portable_capture_start_and_stop_drive_capture_driver_health() {
        let mut backend = PortableCaptureBackend::new_with_driver_for_test(Box::new(
            FakeCaptureDriver::default(),
        ));

        assert!(!backend.is_running());
        assert!(matches!(backend.health(), BackendHealth::Degraded { .. }));

        backend.start().unwrap();

        assert!(backend.is_running());
        assert_eq!(backend.health(), BackendHealth::Healthy);

        backend.stop().unwrap();

        assert!(!backend.is_running());
        assert!(matches!(backend.health(), BackendHealth::Degraded { .. }));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_native_capture_start_and_stop_drive_capture_driver_health() {
        let mut backend = WindowsNativeCaptureBackend::new_with_driver_for_test(Box::new(
            FakeCaptureDriver::default(),
        ));

        assert!(!backend.is_running());
        assert!(matches!(backend.health(), BackendHealth::Degraded { .. }));

        backend.start().unwrap();

        assert!(backend.is_running());
        assert_eq!(backend.health(), BackendHealth::Healthy);

        backend.stop().unwrap();

        assert!(!backend.is_running());
        assert!(matches!(backend.health(), BackendHealth::Degraded { .. }));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_native_capture_start_fails_when_driver_does_not_run() {
        let mut backend = WindowsNativeCaptureBackend::new_with_driver_for_test(Box::new(
            NonRunningCaptureDriver,
        ));

        let result = backend.start();

        assert!(result.is_err());
        assert!(!backend.is_running());
        assert!(matches!(backend.health(), BackendHealth::Degraded { .. }));
    }

    #[test]
    fn portable_inject_returns_error_when_inactive() {
        let mut backend = PortableInjectBackend::new_for_test().unwrap();
        backend.deactivate().unwrap();

        let result = backend.inject(InputEvent::mouse_move(10, 10));

        assert!(result.is_err());
    }

    #[derive(Debug, Default)]
    struct FakeCaptureDriver {
        running: bool,
    }

    impl CaptureDriver for FakeCaptureDriver {
        fn start(&mut self) -> Result<()> {
            self.running = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<()> {
            self.running = false;
            Ok(())
        }

        fn is_running(&self) -> bool {
            self.running
        }
    }

    #[derive(Debug)]
    struct FailingCaptureDriver;

    impl CaptureDriver for FailingCaptureDriver {
        fn start(&mut self) -> Result<()> {
            anyhow::bail!("capture driver failed to start")
        }

        fn stop(&mut self) -> Result<()> {
            Ok(())
        }

        fn is_running(&self) -> bool {
            false
        }
    }

    #[derive(Debug)]
    struct NonRunningCaptureDriver;

    impl CaptureDriver for NonRunningCaptureDriver {
        fn start(&mut self) -> Result<()> {
            Ok(())
        }

        fn stop(&mut self) -> Result<()> {
            Ok(())
        }

        fn is_running(&self) -> bool {
            false
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn virtual_hid_capture_backend_reports_virtual_hid_kind() {
        let backend = VirtualHidCaptureBackend::new_for_test().unwrap();
        assert_eq!(backend.kind(), BackendKind::VirtualHid);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn virtual_hid_capture_backend_starts_degraded() {
        let backend = VirtualHidCaptureBackend::new();
        assert!(matches!(backend.health(), BackendHealth::Degraded { .. }));
        assert!(!backend.is_running());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn virtual_hid_capture_start_and_stop_drive_driver_health() {
        let mut backend = VirtualHidCaptureBackend::new_with_driver_for_test(Box::new(
            FakeCaptureDriver::default(),
        ));

        assert!(!backend.is_running());
        assert!(matches!(backend.health(), BackendHealth::Degraded { .. }));

        backend.start().unwrap();

        assert!(backend.is_running());
        assert_eq!(backend.health(), BackendHealth::Healthy);

        backend.stop().unwrap();

        assert!(!backend.is_running());
        assert!(matches!(backend.health(), BackendHealth::Degraded { .. }));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn virtual_hid_inject_backend_reports_virtual_hid_kind() {
        let backend = VirtualHidInjectBackend::new_for_test();
        // This will fail if the driver is not installed, which is expected
        // In a real test environment with the driver, it would succeed
        if backend.is_ok() {
            let backend = backend.unwrap();
            assert_eq!(backend.kind(), BackendKind::VirtualHid);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn virtual_hid_inject_backend_is_active_when_healthy() {
        let backend = VirtualHidInjectBackend::new_for_test();
        if backend.is_ok() {
            let backend = backend.unwrap();
            assert!(backend.is_active());
            assert!(matches!(backend.health(), BackendHealth::Healthy));
        }
    }
}
