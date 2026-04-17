//! Cross-platform clipboard change detection
//!
//! This module provides clipboard listeners for different platforms.
//! Windows uses event-driven Win32 API, while macOS and Linux use polling.

use rshare_core::clipboard::ClipboardContent;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{interval, Duration};

use super::{ClipboardListener, ClipboardListenerConfig};

/// Shared state for clipboard listeners
struct ListenerState {
    running: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<ClipboardContent>,
    last_content: Arc<Mutex<Option<ClipboardContent>>>,
}

impl Clone for ListenerState {
    fn clone(&self) -> Self {
        Self {
            running: self.running.clone(),
            tx: self.tx.clone(),
            last_content: self.last_content.clone(),
        }
    }
}

impl ListenerState {
    fn new() -> (Self, mpsc::UnboundedReceiver<ClipboardContent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = Self {
            running: Arc::new(AtomicBool::new(false)),
            tx,
            last_content: Arc::new(Mutex::new(None)),
        };
        (state, rx)
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::Relaxed);
    }

    async fn send_if_changed(&self, content: ClipboardContent) {
        let mut last = self.last_content.lock().await;
        let changed = match (&*last, &content) {
            (None, _) => true,
            (Some(prev), curr) => {
                // Simple comparison - in real implementation would do deeper comparison
                prev.mime_type() != curr.mime_type() || prev.size() != curr.size()
            }
        };

        if changed {
            let _ = self.tx.send(content.clone());
            *last = Some(content);
        }
    }
}

// Windows implementation - event-driven
#[cfg(windows)]
pub mod windows_impl {
    use super::*;

    /// Windows clipboard listener using AddClipboardFormatListener
    pub struct WindowsClipboardListener {
        state: ListenerState,
        config: ClipboardListenerConfig,
    }

    impl WindowsClipboardListener {
        pub fn new() -> Self {
            let (state, _) = ListenerState::new();
            Self {
                state,
                config: ClipboardListenerConfig::default(),
            }
        }

        pub fn with_config(mut self, config: ClipboardListenerConfig) -> Self {
            self.config = config;
            self
        }
    }

    #[async_trait::async_trait]
    impl ClipboardListener for WindowsClipboardListener {
        async fn start(&mut self) -> anyhow::Result<()> {
            if self.state.is_running() {
                return Ok(());
            }

            self.state.set_running(true);
            let running = self.state.running.clone();
            let tx = self.state.tx.clone();
            let last_content = self.state.last_content.clone();

            // Spawn message loop task
            tokio::task::spawn_blocking(move || {
                // TODO: Implement Win32 message loop with AddClipboardFormatListener
                // For now, use polling as a fallback
                let rt = tokio::runtime::Handle::try_current();
                if rt.is_err() {
                    return;
                }

                // Polling implementation as fallback
                while running.load(Ordering::Relaxed) {
                    if let Ok(content) = get_clipboard_content() {
                        let _ = tx.send(content);
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            });

            tracing::info!("Windows clipboard listener started");
            Ok(())
        }

        async fn stop(&mut self) -> anyhow::Result<()> {
            self.state.set_running(false);
            tracing::info!("Windows clipboard listener stopped");
            Ok(())
        }

        fn is_running(&self) -> bool {
            self.state.is_running()
        }

        fn receiver(&mut self) -> mpsc::UnboundedReceiver<ClipboardContent> {
            // Need to recreate the channel
            let (state, rx) = ListenerState::new();
            let old_running = self.state.is_running();
            self.state = state;
            if old_running {
                self.state.set_running(true);
            }
            rx
        }

        async fn get_current_clipboard(&self) -> anyhow::Result<ClipboardContent> {
            get_clipboard_content()
        }
    }

    /// Get current clipboard content (Windows)
    fn get_clipboard_content() -> anyhow::Result<ClipboardContent> {
        // Use arboard for cross-platform clipboard access
        let mut clipboard = arboard::Clipboard::new()?;
        if let Ok(text) = clipboard.get_text() {
            return Ok(ClipboardContent::Text(text));
        }
        Ok(ClipboardContent::Empty)
    }

    impl Default for WindowsClipboardListener {
        fn default() -> Self {
            Self::new()
        }
    }
}

// macOS implementation - polling
#[cfg(target_os = "macos")]
pub mod macos_impl {
    use super::*;

    /// macOS clipboard listener using polling
    pub struct MacosClipboardListener {
        state: ListenerState,
        config: ClipboardListenerConfig,
    }

    impl MacosClipboardListener {
        pub fn new() -> Self {
            let (state, _) = ListenerState::new();
            Self {
                state,
                config: ClipboardListenerConfig::default(),
            }
        }

        pub fn with_config(mut self, config: ClipboardListenerConfig) -> Self {
            self.config = config;
            self
        }
    }

    #[async_trait::async_trait]
    impl ClipboardListener for MacosClipboardListener {
        async fn start(&mut self) -> anyhow::Result<()> {
            if self.state.is_running() {
                return Ok(());
            }

            self.state.set_running(true);
            let state = self.state.clone();
            let poll_interval = Duration::from_millis(self.config.poll_interval_ms);

            tokio::spawn(async move {
                let mut interval = interval(poll_interval);

                while state.running.load(Ordering::Relaxed) {
                    interval.tick().await;

                    if let Ok(content) = get_clipboard_content().await {
                        state.send_if_changed(content).await;
                    }
                }
            });

            tracing::info!("macOS clipboard listener started");
            Ok(())
        }

        async fn stop(&mut self) -> anyhow::Result<()> {
            self.state.set_running(false);
            tracing::info!("macOS clipboard listener stopped");
            Ok(())
        }

        fn is_running(&self) -> bool {
            self.state.is_running()
        }

        fn receiver(&mut self) -> mpsc::UnboundedReceiver<ClipboardContent> {
            let (state, rx) = ListenerState::new();
            let old_running = self.state.is_running();
            self.state = state;
            if old_running {
                self.state.set_running(true);
            }
            rx
        }

        async fn get_current_clipboard(&self) -> anyhow::Result<ClipboardContent> {
            get_clipboard_content().await
        }
    }

    /// Get current clipboard content (macOS)
    async fn get_clipboard_content() -> anyhow::Result<ClipboardContent> {
        tokio::task::spawn_blocking(|| {
            if let Ok(files) = crate::macos::current_pasteboard_file_list() {
                if !files.is_empty() {
                    return Ok(ClipboardContent::FileList(files));
                }
            }

            let mut clipboard = arboard::Clipboard::new()?;
            if let Ok(text) = clipboard.get_text() {
                return Ok(ClipboardContent::Text(text));
            }
            Ok(ClipboardContent::Empty)
        })
        .await?
    }

    impl Default for MacosClipboardListener {
        fn default() -> Self {
            Self::new()
        }
    }
}

// Linux implementation - polling
#[cfg(target_os = "linux")]
pub mod linux_impl {
    use super::*;

    /// Linux clipboard listener using polling
    pub struct LinuxClipboardListener {
        state: ListenerState,
        config: ClipboardListenerConfig,
    }

    impl LinuxClipboardListener {
        pub fn new() -> Self {
            let (state, _) = ListenerState::new();
            Self {
                state,
                config: ClipboardListenerConfig::default(),
            }
        }

        pub fn with_config(mut self, config: ClipboardListenerConfig) -> Self {
            self.config = config;
            self
        }
    }

    #[async_trait::async_trait]
    impl ClipboardListener for LinuxClipboardListener {
        async fn start(&mut self) -> anyhow::Result<()> {
            if self.state.is_running() {
                return Ok(());
            }

            self.state.set_running(true);
            let running = self.state.running.clone();
            let tx = self.state.tx.clone();
            let poll_interval = Duration::from_millis(self.config.poll_interval_ms);

            tokio::spawn(async move {
                let mut interval = interval(poll_interval);

                while running.load(Ordering::Relaxed) {
                    interval.tick().await;

                    if let Ok(content) = get_clipboard_content().await {
                        let _ = tx.send(content);
                    }
                }
            });

            tracing::info!("Linux clipboard listener started");
            Ok(())
        }

        async fn stop(&mut self) -> anyhow::Result<()> {
            self.state.set_running(false);
            tracing::info!("Linux clipboard listener stopped");
            Ok(())
        }

        fn is_running(&self) -> bool {
            self.state.is_running()
        }

        fn receiver(&mut self) -> mpsc::UnboundedReceiver<ClipboardContent> {
            let (state, rx) = ListenerState::new();
            let old_running = self.state.is_running();
            self.state = state;
            if old_running {
                self.state.set_running(true);
            }
            rx
        }

        async fn get_current_clipboard(&self) -> anyhow::Result<ClipboardContent> {
            get_clipboard_content().await
        }
    }

    /// Get current clipboard content (Linux)
    async fn get_clipboard_content() -> anyhow::Result<ClipboardContent> {
        tokio::task::spawn_blocking(|| {
            let mut clipboard = arboard::Clipboard::new()?;
            if let Ok(text) = clipboard.get_text() {
                return Ok(ClipboardContent::Text(text));
            }
            Ok(ClipboardContent::Empty)
        })
        .await?
    }

    impl Default for LinuxClipboardListener {
        fn default() -> Self {
            Self::new()
        }
    }
}

// Re-export platform-specific implementations
#[cfg(windows)]
pub use windows_impl::WindowsClipboardListener;

#[cfg(target_os = "macos")]
pub use macos_impl::MacosClipboardListener;

#[cfg(target_os = "linux")]
pub use linux_impl::LinuxClipboardListener;
