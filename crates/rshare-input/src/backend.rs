//! Input backend traits and adapter implementations
//!
//! This module defines the core backend abstractions for input capture
//! and injection, along with portable adapters that wrap existing implementations.

use crate::emulator::InputEmulator;
use crate::events::InputEvent;
use crate::listener::RDevInputListener;
use anyhow::Result;
use std::fmt::{self, Debug};

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
    pub fn new_for_test() -> Result<Self> {
        Ok(Self::new())
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

/// Portable injection backend using enigo.
pub struct PortableInjectBackend {
    emulator: crate::emulator::EnigoInputEmulator,
    health: BackendHealth,
}

impl PortableInjectBackend {
    /// Create a new portable injection backend.
    pub fn new() -> Result<Self> {
        let mut emulator = crate::emulator::EnigoInputEmulator::new()?;
        emulator.activate()?;

        Ok(Self {
            emulator,
            health: BackendHealth::Healthy,
        })
    }

    /// Create a new portable injection backend for testing.
    pub fn new_for_test() -> Result<Self> {
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
    _private: (),
}

#[cfg(target_os = "windows")]
impl VirtualHidInjectBackend {
    /// Create a new Virtual HID injection backend.
    ///
    /// Returns an error because Virtual HID is not yet implemented.
    pub fn new() -> Result<Self> {
        Err(anyhow::anyhow!(
            "Virtual HID backend is not yet implemented. \
             Please use Portable or WindowsNative backends."
        ))
    }

    /// Create a new Virtual HID injection backend for testing.
    ///
    /// Returns an error because Virtual HID is not yet implemented.
    pub fn new_for_test() -> Result<Self> {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
impl Debug for VirtualHidInjectBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualHidInjectBackend").finish()
    }
}

#[cfg(target_os = "windows")]
impl InjectBackend for VirtualHidInjectBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::VirtualHid
    }

    fn health(&self) -> BackendHealth {
        BackendHealth::Degraded {
            reason: BackendFailureReason::Unavailable,
        }
    }

    fn inject(&mut self, _event: InputEvent) -> Result<()> {
        Err(anyhow::anyhow!("Virtual HID injection not implemented"))
    }

    fn is_active(&self) -> bool {
        false
    }
}

/// Virtual HID capture backend (Windows only).
///
/// This backend uses virtual HID devices to capture input at a lower level.
/// Currently scaffold only - returns Unsupported error.
#[cfg(target_os = "windows")]
pub struct VirtualHidCaptureBackend {
    _private: (),
}

#[cfg(target_os = "windows")]
impl VirtualHidCaptureBackend {
    /// Create a new Virtual HID capture backend.
    ///
    /// Returns an error because Virtual HID is not yet implemented.
    pub fn new() -> Result<Self> {
        Err(anyhow::anyhow!(
            "Virtual HID backend is not yet implemented. \
             Please use Portable or WindowsNative backends."
        ))
    }

    /// Create a new Virtual HID capture backend for testing.
    ///
    /// Returns an error because Virtual HID is not yet implemented.
    pub fn new_for_test() -> Result<Self> {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
impl Debug for VirtualHidCaptureBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualHidCaptureBackend").finish()
    }
}

#[cfg(target_os = "windows")]
impl Default for VirtualHidCaptureBackend {
    fn default() -> Self {
        Self { _private: () }
    }
}

#[cfg(target_os = "windows")]
impl CaptureBackend for VirtualHidCaptureBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::VirtualHid
    }

    fn health(&self) -> BackendHealth {
        BackendHealth::Degraded {
            reason: BackendFailureReason::Unavailable,
        }
    }

    fn start(&mut self) -> Result<()> {
        Err(anyhow::anyhow!("Virtual HID capture not implemented"))
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn is_running(&self) -> bool {
        false
    }
}

impl PrivilegeBackend for NoopPrivilegeBackend {
    fn current_state(&self) -> rshare_core::PrivilegeState {
        rshare_core::PrivilegeState::UnlockedDesktop
    }
}

/// Re-export privilege types for convenience
pub use crate::privilege::WindowsPrivilegeBackend as PrivilegeTracker;

#[cfg(not(target_os = "windows"))]
pub use NoopPrivilegeBackend as PrivilegeTracker;

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
}
