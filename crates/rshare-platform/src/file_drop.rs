//! File drag and drop detection
//!
//! This module provides cross-platform file drag and drop detection.
//! When files are dragged, they can be converted to ClipboardContent::FileList
//! for transmission to other devices.

use rshare_core::clipboard::ClipboardContent;
use std::sync::mpsc;

/// File drag event
#[derive(Debug, Clone)]
pub enum FileDragEvent {
    /// Files are being dragged (with file paths)
    FilesDragged(Vec<String>),
    /// Drag operation ended without drop
    DragCancelled,
    /// Files were dropped (optional - can be handled by OS)
    FilesDropped(Vec<String>),
}

/// File drag detector
pub trait FileDragDetector {
    /// Start monitoring for file drag events
    fn start(&mut self) -> anyhow::Result<()>;

    /// Stop monitoring
    fn stop(&mut self) -> anyhow::Result<()>;

    /// Get the event receiver
    fn events(&mut self) -> mpsc::Receiver<FileDragEvent>;
}

/// Convert drag event to clipboard content
impl From<FileDragEvent> for ClipboardContent {
    fn from(event: FileDragEvent) -> Self {
        match event {
            FileDragEvent::FilesDragged(files) => ClipboardContent::FileList(files),
            FileDragEvent::FilesDropped(files) => ClipboardContent::FileList(files),
            FileDragEvent::DragCancelled => ClipboardContent::Empty,
        }
    }
}

// Platform-specific implementations

#[cfg(windows)]
pub use windows_impl::WindowsFileDragDetector as PlatformFileDragDetector;

#[cfg(target_os = "macos")]
pub use macos_impl::MacosFileDragDetector as PlatformFileDragDetector;

#[cfg(target_os = "linux")]
pub use linux_impl::LinuxFileDragDetector as PlatformFileDragDetector;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub use stub_impl::StubFileDragDetector as PlatformFileDragDetector;

// Windows implementation
#[cfg(windows)]
mod windows_impl {
    use super::{FileDragDetector, FileDragEvent};
    use std::sync::mpsc;

    /// Windows file drag detector
    ///
    /// Uses Windows hooks to detect file drag operations from Explorer
    /// and other applications.
    pub struct WindowsFileDragDetector {
        _tx: mpsc::Sender<FileDragEvent>,
        rx: Option<mpsc::Receiver<FileDragEvent>>,
        active: bool,
    }

    impl WindowsFileDragDetector {
        pub fn new() -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                _tx: tx,
                rx: Some(rx),
                active: false,
            }
        }
    }

    impl FileDragDetector for WindowsFileDragDetector {
        fn start(&mut self) -> anyhow::Result<()> {
            if self.active {
                return Ok(());
            }

            self.active = true;
            tracing::info!("Windows file drag detector started");

            // TODO: Implement Windows file drag detection
            // This requires:
            // 1. SetWindowsHookEx with WH_CALLWNDPROC or WH_SHELL
            // 2. Monitor WM_DROPFILES or WM_COPYDATA with file drag info
            // 3. Parse HDROP data structure to get file paths
            // 4. Send events through the channel

            Ok(())
        }

        fn stop(&mut self) -> anyhow::Result<()> {
            self.active = false;
            tracing::info!("Windows file drag detector stopped");
            Ok(())
        }

        fn events(&mut self) -> mpsc::Receiver<FileDragEvent> {
            self.rx.take().expect("Event receiver already taken")
        }
    }

    impl Default for WindowsFileDragDetector {
        fn default() -> Self {
            Self::new()
        }
    }
}

// macOS implementation
#[cfg(target_os = "macos")]
mod macos_impl {
    use super::{FileDragDetector, FileDragEvent};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    /// macOS file drag detector
    ///
    /// Polls NSPasteboard for file URL changes. macOS does not expose a
    /// general global drag hook that works across all apps.
    pub struct MacosFileDragDetector {
        tx: mpsc::Sender<FileDragEvent>,
        rx: Option<mpsc::Receiver<FileDragEvent>>,
        running: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl MacosFileDragDetector {
        pub fn new() -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                tx,
                rx: Some(rx),
                running: Arc::new(AtomicBool::new(false)),
                worker: None,
            }
        }
    }

    impl FileDragDetector for MacosFileDragDetector {
        fn start(&mut self) -> anyhow::Result<()> {
            if self.running.load(Ordering::Relaxed) {
                return Ok(());
            }

            self.running.store(true, Ordering::Relaxed);
            let running = self.running.clone();
            let tx = self.tx.clone();

            self.worker = Some(thread::spawn(move || {
                let mut last_files = Vec::new();

                while running.load(Ordering::Relaxed) {
                    match crate::macos::current_pasteboard_file_list() {
                        Ok(files) if !files.is_empty() && files != last_files => {
                            last_files = files.clone();
                            let _ = tx.send(FileDragEvent::FilesDragged(files));
                        }
                        Ok(files) if files.is_empty() && !last_files.is_empty() => {
                            last_files.clear();
                            let _ = tx.send(FileDragEvent::DragCancelled);
                        }
                        Ok(_) => {}
                        Err(err) => {
                            tracing::debug!("macOS file drag pasteboard poll failed: {}", err);
                        }
                    }

                    thread::sleep(Duration::from_millis(250));
                }
            }));

            tracing::info!("macOS file drag detector started");
            Ok(())
        }

        fn stop(&mut self) -> anyhow::Result<()> {
            self.running.store(false, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("macOS file drag detector thread panicked"))?;
            }
            tracing::info!("macOS file drag detector stopped");
            Ok(())
        }

        fn events(&mut self) -> mpsc::Receiver<FileDragEvent> {
            self.rx.take().expect("Event receiver already taken")
        }
    }

    impl Default for MacosFileDragDetector {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for MacosFileDragDetector {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }
}

// Linux implementation
#[cfg(target_os = "linux")]
mod linux_impl {
    use super::{FileDragDetector, FileDragEvent};
    use std::sync::mpsc;

    /// Linux file drag detector
    ///
    /// Uses X11 DND or Wayland drag-and-drop protocols.
    pub struct LinuxFileDragDetector {
        tx: mpsc::Sender<FileDragEvent>,
        rx: Option<mpsc::Receiver<FileDragEvent>>,
        active: bool,
    }

    impl LinuxFileDragDetector {
        pub fn new() -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                tx,
                rx: Some(rx),
                active: false,
            }
        }
    }

    impl FileDragDetector for LinuxFileDragDetector {
        fn start(&mut self) -> anyhow::Result<()> {
            if self.active {
                return Ok(());
            }

            self.active = true;
            tracing::info!("Linux file drag detector started");

            // TODO: Implement Linux file drag detection
            // This requires:
            // 1. X11: Monitor XdndSelection events
            // 2. Wayland: Monitor data_device drag events
            // 3. Parse text/uri-list MIME data
            // 4. Send events through the channel

            Ok(())
        }

        fn stop(&mut self) -> anyhow::Result<()> {
            self.active = false;
            tracing::info!("Linux file drag detector stopped");
            Ok(())
        }

        fn events(&mut self) -> mpsc::Receiver<FileDragEvent> {
            self.rx.take().expect("Event receiver already taken")
        }
    }

    impl Default for LinuxFileDragDetector {
        fn default() -> Self {
            Self::new()
        }
    }
}

// Stub implementation for unsupported platforms
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod stub_impl {
    use super::{FileDragDetector, FileDragEvent};
    use std::sync::mpsc;

    pub struct StubFileDragDetector {
        rx: Option<mpsc::Receiver<FileDragEvent>>,
    }

    impl StubFileDragDetector {
        pub fn new() -> Self {
            let (_tx, rx) = mpsc::channel();
            Self { rx: Some(rx) }
        }
    }

    impl FileDragDetector for StubFileDragDetector {
        fn start(&mut self) -> anyhow::Result<()> {
            tracing::warn!("File drag detection not supported on this platform");
            Ok(())
        }

        fn stop(&mut self) -> anyhow::Result<()> {
            Ok(())
        }

        fn events(&mut self) -> mpsc::Receiver<FileDragEvent> {
            self.rx.take().expect("Event receiver already taken")
        }
    }

    impl Default for StubFileDragDetector {
        fn default() -> Self {
            Self::new()
        }
    }
}
