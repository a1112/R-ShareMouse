//! Clipboard synchronization service
//!
//! This module handles clipboard synchronization between devices.

use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, RwLock};

use crate::clipboard::{ClipboardContent, ContentType};
use crate::{DeviceId, Message};

/// Trait for system clipboard listeners
///
/// Platform-specific implementations should implement this trait
/// to detect clipboard changes.
#[async_trait::async_trait]
pub trait ClipboardListener: Send + Sync {
    /// Start listening for clipboard changes
    async fn start(&mut self) -> Result<()>;

    /// Stop listening
    async fn stop(&mut self) -> Result<()>;

    /// Check if listener is running
    fn is_running(&self) -> bool;

    /// Get the event receiver
    fn receiver(&mut self) -> mpsc::UnboundedReceiver<ClipboardContent>;

    /// Get current clipboard content
    async fn get_current_clipboard(&self) -> Result<ClipboardContent>;
}

/// Clipboard sync mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardSyncMode {
    /// Disabled - no clipboard sync
    Disabled,
    /// One way - only send local clipboard
    SendOnly,
    /// One way - only receive remote clipboard
    ReceiveOnly,
    /// Two way - send and receive
    Bidirectional,
}

impl Default for ClipboardSyncMode {
    fn default() -> Self {
        Self::Bidirectional
    }
}

/// Configuration for clipboard synchronization
#[derive(Debug, Clone)]
pub struct ClipboardSyncConfig {
    /// Sync mode
    pub mode: ClipboardSyncMode,
    /// Maximum clipboard size to transfer (bytes)
    pub max_size: usize,
    /// Whether to sync text
    pub sync_text: bool,
    /// Whether to sync images
    pub sync_images: bool,
    /// Whether to sync other formats
    pub sync_other: bool,
    /// Debounce delay (prevent rapid updates)
    pub debounce: Duration,
}

impl Default for ClipboardSyncConfig {
    fn default() -> Self {
        Self {
            mode: ClipboardSyncMode::Bidirectional,
            max_size: 10 * 1024 * 1024, // 10 MB
            sync_text: true,
            sync_images: true,
            sync_other: false,
            debounce: Duration::from_millis(500),
        }
    }
}

/// Clipboard event
#[derive(Debug, Clone)]
pub enum ClipboardEvent {
    /// Local clipboard changed
    LocalChanged(ClipboardContent),
    /// Remote clipboard received
    RemoteReceived {
        device: DeviceId,
        content: ClipboardContent,
    },
    /// Clipboard sync failed
    SyncFailed { device: DeviceId, error: String },
}

/// Clipboard statistics
#[derive(Debug, Clone, Default)]
pub struct ClipboardStats {
    pub items_sent: u64,
    pub items_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub errors: u64,
    pub last_sync: Option<Instant>,
}

/// Clipboard synchronization service
pub struct ClipboardSyncService {
    config: ClipboardSyncConfig,
    local_clipboard: Option<ClipboardContent>,
    last_sync: Option<Instant>,
    stats: ClipboardStats,
    enabled: bool,
    /// Event channel for clipboard changes
    event_tx: mpsc::Sender<ClipboardEvent>,
    event_rx: Option<mpsc::Receiver<ClipboardEvent>>,
    /// Listener handle for managing system clipboard listener
    listener_handle: Option<ClipboardListenerHandle>,
}

/// Handle for managing a running clipboard listener
pub struct ClipboardListenerHandle {
    shutdown_tx: broadcast::Sender<()>,
}

impl ClipboardListenerHandle {
    /// Stop the listener
    pub async fn stop(self) {
        let _ = self.shutdown_tx.send(());
    }
}

impl Clone for ClipboardListenerHandle {
    fn clone(&self) -> Self {
        Self {
            shutdown_tx: self.shutdown_tx.clone(),
        }
    }
}

impl ClipboardSyncService {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel(100);
        Self {
            config: ClipboardSyncConfig::default(),
            local_clipboard: None,
            last_sync: None,
            stats: ClipboardStats::default(),
            enabled: true,
            event_tx,
            event_rx: Some(event_rx),
            listener_handle: None,
        }
    }

    pub fn with_config(mut self, config: ClipboardSyncConfig) -> Self {
        self.config = config;
        self
    }

    /// Process a clipboard event from local device
    pub fn on_local_clipboard_changed(&mut self, content: ClipboardContent) -> Option<Message> {
        if !self.enabled {
            return None;
        }

        if !matches!(
            self.config.mode,
            ClipboardSyncMode::Bidirectional | ClipboardSyncMode::SendOnly
        ) {
            return None;
        }

        // Check debounce
        if let Some(last) = self.last_sync {
            if last.elapsed() < self.config.debounce {
                return None;
            }
        }

        // Check if content actually changed
        if self.local_clipboard.as_ref() == Some(&content) {
            return None;
        }

        // Validate content based on config
        if !self.should_sync_content(&content) {
            return None;
        }

        // Check size limit
        if content.size() > self.config.max_size {
            tracing::warn!("Clipboard content too large: {} bytes", content.size());
            return None;
        }

        self.local_clipboard = Some(content.clone());
        self.last_sync = Some(Instant::now());
        self.stats.items_sent += 1;
        self.stats.bytes_sent += content.size() as u64;

        // Convert content to mime_type and data
        let (mime_type, data) = self.content_to_bytes(&content)?;

        Some(Message::ClipboardData { mime_type, data })
    }

    /// Process clipboard data from remote device
    pub fn on_remote_clipboard_received(
        &mut self,
        device: DeviceId,
        mime_type: String,
        data: Vec<u8>,
    ) -> Result<ClipboardEvent> {
        if !self.enabled {
            return Ok(ClipboardEvent::SyncFailed {
                device,
                error: "Sync disabled".to_string(),
            });
        }

        if !matches!(
            self.config.mode,
            ClipboardSyncMode::Bidirectional | ClipboardSyncMode::ReceiveOnly
        ) {
            return Ok(ClipboardEvent::SyncFailed {
                device,
                error: "Receive disabled".to_string(),
            });
        }

        let content = ClipboardContent::from_mime_and_data(&mime_type, &data)?;
        let size = content.size();

        self.stats.items_received += 1;
        self.stats.bytes_received += size as u64;

        Ok(ClipboardEvent::RemoteReceived { device, content })
    }

    /// Convert ClipboardContent to bytes
    fn content_to_bytes(&self, content: &ClipboardContent) -> Option<(String, Vec<u8>)> {
        match content {
            ClipboardContent::Text(text) => {
                Some(("text/plain".to_string(), text.as_bytes().to_vec()))
            }
            ClipboardContent::Html { html, .. } => {
                Some(("text/html".to_string(), html.as_bytes().to_vec()))
            }
            ClipboardContent::Image { data, .. } => Some(("image/png".to_string(), data.clone())),
            ClipboardContent::FileList(files) => {
                let data = files.join("\n").into_bytes();
                Some(("text/uri-list".to_string(), data))
            }
            ClipboardContent::Other { mime, data } => Some((mime.clone(), data.clone())),
            ClipboardContent::Empty => None,
        }
    }

    /// Check if content should be synced based on config
    fn should_sync_content(&self, content: &ClipboardContent) -> bool {
        match content.content_type() {
            ContentType::Text => self.config.sync_text,
            ContentType::Image => self.config.sync_images,
            ContentType::Other => self.config.sync_other,
        }
    }

    /// Enable clipboard sync
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Disable clipboard sync
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Check if sync is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get the statistics
    pub fn stats(&self) -> &ClipboardStats {
        &self.stats
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.stats = ClipboardStats::default();
    }

    /// Get the current local clipboard content
    pub fn local_clipboard(&self) -> Option<&ClipboardContent> {
        self.local_clipboard.as_ref()
    }

    /// Get the event receiver
    pub fn events(&mut self) -> mpsc::Receiver<ClipboardEvent> {
        self.event_rx.take().expect("Event receiver already taken")
    }

    /// Start the system clipboard listener
    pub async fn start_listener<L>(&mut self, mut listener: L) -> Result<ClipboardListenerHandle>
    where
        L: ClipboardListener + Send + 'static,
    {
        let (shutdown_tx, _) = broadcast::channel(1);
        let mut shutdown_rx = shutdown_tx.subscribe();

        // Start the listener
        listener.start().await?;

        // Get the receiver channel from the listener
        let mut listener_rx = listener.receiver();
        let event_tx = self.event_tx.clone();

        // Spawn a task to forward clipboard changes to our service
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = shutdown_rx.recv() => {
                        if result.is_ok() {
                            // Shutdown signal received
                            break;
                        }
                    }
                    content = listener_rx.recv() => {
                        if let Some(content) = content {
                            tracing::debug!("Clipboard changed: {:?}", content.mime_type());
                            let _ = event_tx.send(ClipboardEvent::LocalChanged(content)).await;
                        }
                    }
                }
            }

            // Stop the listener
            let _ = listener.stop().await;
        });

        self.listener_handle = Some(ClipboardListenerHandle {
            shutdown_tx: shutdown_tx.clone(),
        });

        tracing::info!("Clipboard listener started");
        Ok(ClipboardListenerHandle { shutdown_tx })
    }

    /// Stop the running listener
    pub async fn stop_listener(&mut self) -> Result<()> {
        if let Some(handle) = self.listener_handle.take() {
            handle.stop().await;
        }
        Ok(())
    }
}

impl Default for ClipboardSyncService {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared clipboard sync service for async access
pub type SharedClipboardSyncService = Arc<RwLock<ClipboardSyncService>>;

/// Create a shared clipboard sync service
pub fn create_shared_clipboard_sync() -> SharedClipboardSyncService {
    Arc::new(RwLock::new(ClipboardSyncService::new()))
}

/// Clipboard content type helper
pub trait ContentTypeHelper {
    fn content_type(&self) -> ContentType;
    fn from_mime_and_data(mime: &str, data: &[u8]) -> Result<Self>
    where
        Self: Sized;
}

impl ContentTypeHelper for ClipboardContent {
    fn content_type(&self) -> ContentType {
        match self {
            ClipboardContent::Text(_) | ClipboardContent::Html { .. } => ContentType::Text,
            ClipboardContent::Image { .. } => ContentType::Image,
            ClipboardContent::FileList(_) | ClipboardContent::Other { .. } => ContentType::Other,
            ClipboardContent::Empty => ContentType::Other,
        }
    }

    fn from_mime_and_data(mime: &str, data: &[u8]) -> Result<Self>
    where
        Self: Sized,
    {
        match mime {
            "text/plain" => {
                let text = String::from_utf8(data.to_vec())
                    .map_err(|e| anyhow::anyhow!("Invalid UTF-8: {}", e))?;
                Ok(ClipboardContent::Text(text))
            }
            "text/html" => {
                let html = String::from_utf8(data.to_vec())
                    .map_err(|e| anyhow::anyhow!("Invalid UTF-8: {}", e))?;
                let text = strip_html::strip_html(&html);
                Ok(ClipboardContent::Html { html, text })
            }
            mime if mime.starts_with("image/") => {
                let format = crate::clipboard::ImageFormat::from_mime(mime)
                    .unwrap_or(crate::clipboard::ImageFormat::Png);
                Ok(ClipboardContent::Image {
                    format,
                    data: data.to_vec(),
                    width: 0,
                    height: 0,
                })
            }
            _ => Ok(ClipboardContent::Other {
                mime: mime.to_string(),
                data: data.to_vec(),
            }),
        }
    }
}

/// Simple HTML tag stripper
mod strip_html {
    pub fn strip_html(html: &str) -> String {
        let mut result = String::new();
        let mut in_tag = false;

        for c in html.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => result.push(c),
                _ => {}
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_content_text() {
        let content = ClipboardContent::from_text("Hello, World!".to_string());
        assert_eq!(content.as_text(), Some("Hello, World!"));
        assert_eq!(content.size(), 13);
        assert_eq!(content.content_type(), ContentType::Text);
    }

    #[test]
    fn test_clipboard_sync_config_default() {
        let config = ClipboardSyncConfig::default();
        assert_eq!(config.mode, ClipboardSyncMode::Bidirectional);
        assert!(config.sync_text);
        assert!(config.sync_images);
    }

    #[test]
    fn test_clipboard_sync_service() {
        let mut service = ClipboardSyncService::new();

        let content = ClipboardContent::from_text("test".to_string());
        let message = service.on_local_clipboard_changed(content.clone());

        assert!(message.is_some());
        assert_eq!(service.stats().items_sent, 1);

        // Same content should not trigger sync
        let message2 = service.on_local_clipboard_changed(content);
        assert!(message2.is_none());
    }

    #[test]
    fn test_clipboard_sync_debounce() {
        let config = ClipboardSyncConfig {
            debounce: Duration::from_millis(100),
            ..Default::default()
        };
        let mut service = ClipboardSyncService::new().with_config(config);

        let content1 = ClipboardContent::from_text("first".to_string());
        service.on_local_clipboard_changed(content1);

        // Immediate update should be debounced
        let content2 = ClipboardContent::from_text("second".to_string());
        let message = service.on_local_clipboard_changed(content2);
        assert!(message.is_none());
    }

    #[test]
    fn test_clipboard_sync_mode() {
        let config = ClipboardSyncConfig {
            mode: ClipboardSyncMode::SendOnly,
            ..Default::default()
        };
        let mut service = ClipboardSyncService::new().with_config(config);

        // Send mode should allow sending
        let content = ClipboardContent::from_text("test".to_string());
        let message = service.on_local_clipboard_changed(content);
        assert!(message.is_some());

        // But receiving should fail
        let result = service.on_remote_clipboard_received(
            DeviceId::new_v4(),
            "text/plain".to_string(),
            b"remote".to_vec(),
        );
        assert!(result.is_ok());
        match result.unwrap() {
            ClipboardEvent::SyncFailed { .. } => {}
            _ => panic!("Expected SyncFailed event"),
        }
    }

    #[test]
    fn test_clipboard_stats() {
        let mut service = ClipboardSyncService::new();

        service.on_local_clipboard_changed(ClipboardContent::from_text("test".to_string()));
        assert_eq!(service.stats().items_sent, 1);

        service.reset_stats();
        assert_eq!(service.stats().items_sent, 0);
    }

    #[test]
    fn test_strip_html() {
        let html = "<p>Hello, <b>World</b>!</p>";
        let text = strip_html::strip_html(html);
        assert_eq!(text, "Hello, World!");
    }
}
