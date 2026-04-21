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

// Linux implementation with X11 DND support
#[cfg(target_os = "linux")]
mod linux_impl {
    use super::{FileDragDetector, FileDragEvent};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    #[cfg(feature = "x11")]
    use x11::xlib;

    /// Linux file drag detector
    ///
    /// Monitors X11 selection changes for drag-and-drop operations.
    pub struct LinuxFileDragDetector {
        tx: mpsc::Sender<FileDragEvent>,
        rx: Option<mpsc::Receiver<FileDragEvent>>,
        active: bool,
        worker: Option<JoinHandle<()>>,
    }

    impl LinuxFileDragDetector {
        pub fn new() -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                tx,
                rx: Some(rx),
                active: false,
                worker: None,
            }
        }

        #[cfg(feature = "x11")]
        fn start_x11_monitor(&mut self) -> anyhow::Result<()> {
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::Arc;

            let running = Arc::new(AtomicBool::new(true));
            let tx = self.tx.clone();
            let running_clone = running.clone();

            let worker = thread::spawn(move || {
                // Try to open X11 display
                let display = unsafe { xlib::XOpenDisplay(std::ptr::null()) };
                if display.is_null() {
                    tracing::warn!("Failed to open X11 display for file drag detection");
                    return;
                }

                tracing::info!("X11 file drag monitor started");

                // XdndSelection atom
                let xdnd_selection_atom = unsafe {
                    xlib::XInternAtom(
                        display as *mut _,
                        b"XdndSelection\0".as_ptr() as *const i8,
                        0,
                    )
                };

                let mut last_files = Vec::new();

                while running_clone.load(Ordering::Relaxed) {
                    // Check for selection owner changes
                    let owner =
                        unsafe { xlib::XGetSelectionOwner(display as *mut _, xdnd_selection_atom) };

                    if owner != 0 {
                        // Request selection content
                        unsafe {
                            // Convert window - use root window
                            let screen = xlib::XDefaultScreen(display as *mut _);
                            let root = xlib::XRootWindow(display as *mut _, screen);

                            // Request the selection as text/uri-list
                            let utf8_string_atom = xlib::XInternAtom(
                                display as *mut _,
                                b"TEXT/URI-LIST\0".as_ptr() as *const i8,
                                1, // only_if_exists = false
                            );

                            xlib::XConvertSelection(
                                display as *mut _,
                                xdnd_selection_atom,
                                utf8_string_atom,
                                xlib::XA_PRIMARY,
                                root,
                                xlib::CurrentTime,
                            );
                            xlib::XFlush(display as *mut _);
                        }

                        // Small delay to let the selection owner respond
                        thread::sleep(Duration::from_millis(50));

                        // Try to get the selection content
                        let files = Self::get_selection_content(display);
                        if let Ok(current_files) = files {
                            if !current_files.is_empty() && current_files != last_files {
                                last_files = current_files.clone();
                                let _ = tx.send(FileDragEvent::FilesDragged(current_files));
                            } else if current_files.is_empty() && !last_files.is_empty() {
                                last_files.clear();
                                let _ = tx.send(FileDragEvent::DragCancelled);
                            }
                        }
                    }

                    thread::sleep(Duration::from_millis(200));
                }

                unsafe { xlib::XCloseDisplay(display) };
            });

            self.worker = Some(worker);
            Ok(())
        }

        #[cfg(feature = "x11")]
        fn get_selection_content(display: *mut xlib::Display) -> anyhow::Result<Vec<String>> {
            use std::ptr;

            unsafe {
                let screen = xlib::XDefaultScreen(display);
                let root = xlib::XRootWindow(display, screen);

                // Try to get the property from PRIMARY selection
                let mut atom_return: u64 = 0;
                let mut actual_type: u64 = 0;
                let mut format: i32 = 0;
                let mut nitems: u64 = 0;
                let mut bytes_after: u64 = 0;
                let mut prop_return: *mut u8 = ptr::null_mut();

                let result = xlib::XGetWindowProperty(
                    display,
                    root,
                    xlib::XA_PRIMARY,
                    0,
                    1024,
                    0, // delete
                    xlib::XA_STRING,
                    &mut actual_type,
                    &mut format,
                    &mut nitems,
                    &mut bytes_after,
                    &mut prop_return,
                );

                if result == 0 && !prop_return.is_null() && nitems > 0 {
                    let slice = std::slice::from_raw_parts(prop_return, nitems as usize);
                    let content = String::from_utf8_lossy(slice);
                    xlib::XFree(prop_return as *mut _);

                    // Parse text/uri-list format
                    let files: Vec<String> = content
                        .lines()
                        .filter(|line| !line.is_empty() && !line.starts_with('#'))
                        .map(|line| {
                            // Remove file:// prefix if present
                            line.trim()
                                .strip_prefix("file://")
                                .unwrap_or(line.trim())
                                .to_string()
                        })
                        .collect();

                    Ok(files)
                } else {
                    Ok(Vec::new())
                }
            }
        }

        #[cfg(not(feature = "x11"))]
        fn start_x11_monitor(&mut self) -> anyhow::Result<()> {
            tracing::warn!("X11 feature not enabled, file drag detection not available");
            Ok(())
        }
    }

    impl FileDragDetector for LinuxFileDragDetector {
        fn start(&mut self) -> anyhow::Result<()> {
            if self.active {
                return Ok(());
            }

            self.active = true;
            tracing::info!("Linux file drag detector starting");

            #[cfg(feature = "x11")]
            {
                self.start_x11_monitor()?;
            }

            #[cfg(not(feature = "x11"))]
            {
                tracing::warn!("Linux file drag detection requires x11 feature");
            }

            Ok(())
        }

        fn stop(&mut self) -> anyhow::Result<()> {
            if !self.active {
                return Ok(());
            }

            self.active = false;
            if let Some(worker) = self.worker.take() {
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("Linux file drag detector thread panicked"))?;
            }

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

    impl Drop for LinuxFileDragDetector {
        fn drop(&mut self) {
            let _ = self.stop();
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
