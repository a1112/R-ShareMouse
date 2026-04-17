//! Integrated input processing engine
//!
//! This module ties together the input listener, edge detection, state machine,
//! and event forwarding into a unified system.

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, broadcast};

use crate::{DeviceId, Direction, Message};
use rshare_common::ScreenInfo;
use rshare_input::{
    events::{InputEvent, ButtonState, KeyCode, MouseButton},
    edge_detection::{EdgeDetector, Edge, EdgeDetectionConfig},
    emulator::{InputEmulator, EnigoInputEmulator},
    listener::{RDevInputListener, InputEventChannel},
};

/// Integrated input engine configuration
#[derive(Debug, Clone)]
pub struct IntegratedEngineConfig {
    /// Screen dimensions
    pub screen_width: u32,
    pub screen_height: u32,

    /// Edge detection threshold (pixels)
    pub edge_threshold: u32,

    /// Cooldown after edge transition
    pub transition_cooldown: Duration,

    /// Enable input emulation when receiving remote events
    pub enable_emulation: bool,

    /// Enable local input capture
    pub enable_capture: bool,
}

impl Default for IntegratedEngineConfig {
    fn default() -> Self {
        Self {
            screen_width: 1920,
            screen_height: 1080,
            edge_threshold: 10,
            transition_cooldown: Duration::from_millis(500),
            enable_emulation: true,
            enable_capture: true,
        }
    }
}

impl From<&ScreenInfo> for IntegratedEngineConfig {
    fn from(screen: &ScreenInfo) -> Self {
        Self {
            screen_width: screen.width,
            screen_height: screen.height,
            ..Default::default()
        }
    }
}

/// Capture mode state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    Local,
    Remote { target: DeviceId },
}

impl CaptureMode {
    pub fn is_local(&self) -> bool {
        matches!(self, CaptureMode::Local)
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, CaptureMode::Remote { .. })
    }

    pub fn remote_target(&self) -> Option<DeviceId> {
        match self {
            CaptureMode::Remote { target } => Some(*target),
            _ => None,
        }
    }
}

/// Integrated input processing engine
pub struct IntegratedInputEngine {
    /// Configuration
    config: IntegratedEngineConfig,

    /// Current capture mode
    capture_mode: Arc<Mutex<CaptureMode>>,

    /// Edge detector
    edge_detector: EdgeDetector,

    /// Input listener
    listener: RDevInputListener,

    /// Input emulator (for remote events)
    emulator: Arc<Mutex<EnigoInputEmulator>>,

    /// Event channel (from listener)
    event_channel: InputEventChannel,

    /// Outgoing event sender (to network)
    tx_out: mpsc::UnboundedSender<OutgoingEvent>,

    /// Shutdown receiver
    shutdown_rx: broadcast::Receiver<()>,

    /// Mapping from edges to target devices
    edge_targets: Arc<Mutex<std::collections::HashMap<Edge, DeviceId>>>,

    /// Statistics
    stats: Arc<Mutex<EngineStats>>,
}

/// Statistics for the integrated engine
#[derive(Debug, Clone, Default)]
pub struct EngineStats {
    pub events_captured: u64,
    pub events_forwarded: u64,
    pub events_emulated: u64,
    pub transitions_to_remote: u64,
    pub transitions_to_local: u64,
    pub uptime_seconds: u64,
    pub current_mode: String,
}

/// Event to be sent to a remote device
#[derive(Debug, Clone)]
pub struct OutgoingEvent {
    pub target: DeviceId,
    pub event: InputEvent,
}

impl IntegratedInputEngine {
    /// Create a new integrated input engine
    pub async fn new(
        config: IntegratedEngineConfig,
        tx_out: mpsc::UnboundedSender<OutgoingEvent>,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<Self> {
        // Create edge detector
        let edge_config = EdgeDetectionConfig {
            threshold: config.edge_threshold,
            screen_width: config.screen_width,
            screen_height: config.screen_height,
            cooldown: config.transition_cooldown,
        };
        let edge_detector = EdgeDetector::new(edge_config);

        // Create input listener
        let listener = RDevInputListener::new();
        let event_channel = listener.channel();

        // Create input emulator
        let emulator = Arc::new(Mutex::new(EnigoInputEmulator::new()?));

        Ok(Self {
            config,
            capture_mode: Arc::new(Mutex::new(CaptureMode::Local)),
            edge_detector,
            listener,
            emulator,
            event_channel,
            tx_out,
            shutdown_rx,
            edge_targets: Arc::new(Mutex::new(std::collections::HashMap::new())),
            stats: Arc::new(Mutex::new(EngineStats::default())),
        })
    }

    /// Set the target device for an edge
    pub async fn set_edge_target(&self, edge: Edge, device_id: DeviceId) {
        let mut targets = self.edge_targets.lock().await;
        targets.insert(edge, device_id);

        // Also update edge detector
        self.edge_detector.set_target(edge, device_id);
    }

    /// Remove target for an edge
    pub async fn remove_edge_target(&self, edge: &Edge) {
        let mut targets = self.edge_targets.lock().await;
        targets.remove(edge);
    }

    /// Get the current capture mode
    pub async fn capture_mode(&self) -> CaptureMode {
        *self.capture_mode.lock().await
    }

    /// Transition to remote mode
    async fn transition_to_remote(&self, target: DeviceId, direction: Direction) -> Result<()> {
        tracing::info!("Transitioning to remote mode: target={:?}, direction={:?}", target, direction);

        *self.capture_mode.lock().await = CaptureMode::Remote { target };

        // Update statistics
        let mut stats = self.stats.lock().await;
        stats.transitions_to_remote += 1;
        stats.current_mode = format!("Remote({})", target);

        Ok(())
    }

    /// Transition to local mode
    async fn transition_to_local(&self) -> Result<()> {
        tracing::info!("Transitioning to local mode");

        *self.capture_mode.lock().await = CaptureMode::Local;

        // Update statistics
        let mut stats = self.stats.lock().await;
        stats.transitions_to_local += 1;
        stats.current_mode = "Local".to_string();

        Ok(())
    }

    /// Handle an edge transition
    async fn handle_edge_transition(&self, edge: Edge, event: InputEvent) -> Result<bool> {
        let targets = self.edge_targets.lock().await;
        let target = match targets.get(&edge) {
            Some(device) => *device,
            None => {
                tracing::debug!("No target device for edge {:?}", edge);
                return Ok(false);
            }
        };
        drop(targets);

        let direction = match edge {
            Edge::Left => Direction::Left,
            Edge::Right => Direction::Right,
            Edge::Top => Direction::Top,
            Edge::Bottom => Direction::Bottom,
        };

        self.transition_to_remote(target, direction).await?;

        // Forward the triggering event
        self.forward_event(target, event).await?;

        Ok(true)
    }

    /// Forward an event to a remote device
    async fn forward_event(&self, target: DeviceId, event: InputEvent) -> Result<()> {
        let outgoing = OutgoingEvent { target, event };
        self.tx_out.send(outgoing)
            .map_err(|e| anyhow::anyhow!("Failed to forward event: {}", e))?;

        // Update statistics
        let mut stats = self.stats.lock().await;
        stats.events_forwarded += 1;

        Ok(())
    }

    /// Emulate a remote event locally
    async fn emulate_event(&self, event: InputEvent) -> Result<()> {
        if !self.config.enable_emulation {
            return Ok(());
        }

        let mut emulator = self.emulator.lock().await;
        emulator.emulate(event)?;

        // Update statistics
        let mut stats = self.stats.lock().await;
        stats.events_emulated += 1;

        Ok(())
    }

    /// Process a remote event received from network
    pub async fn process_remote_event(&self, event: InputEvent) -> Result<()> {
        // Only emulate if we're in local mode (shouldn't happen, but safety check)
        if self.capture_mode().await.is_local() {
            self.emulate_event(event).await?;
        } else {
            tracing::warn!("Received remote event while in remote mode, ignoring");
        }
        Ok(())
    }

    /// Get statistics
    pub async fn stats(&self) -> EngineStats {
        self.stats.lock().await.clone()
    }

    /// Start the integrated engine
    pub async fn start(self: Arc<Self>) -> Result<()> {
        tracing::info!("Starting integrated input engine");

        // Start input listener
        self.listener.start().await?;

        // Get event receiver
        let mut event_rx = self.listener.receiver().await;
        let engine = self.clone();

        // Spawn event processing task
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            let start_time = std::time::Instant::now();

            loop {
                tokio::select! {
                    // Check for shutdown
                    _ = engine.shutdown_rx.recv() => {
                        tracing::info!("Integrated engine shutting down");
                        let _ = engine.listener.stop().await;
                        break;
                    }

                    // Process input events
                    event = event_rx.recv() => {
                        if let Some(event) = event {
                            if let Err(e) = engine.process_local_event(event).await {
                                tracing::error!("Error processing local event: {:?}", e);
                            }
                        }
                    }

                    // Update statistics periodically
                    _ = interval.tick() => {
                        let mut stats = engine.stats.lock().await;
                        stats.uptime_seconds = start_time.elapsed().as_secs();
                        stats.current_mode = format!("{:?}", *engine.capture_mode.lock().await);
                    }
                }
            }
        });

        Ok(())
    }

    /// Process a local input event
    async fn process_local_event(&self, event: InputEvent) -> Result<()> {
        // Update statistics
        {
            let mut stats = self.stats.lock().await;
            stats.events_captured += 1;
        }

        // Check for edge transitions first
        if let Some(edge_result) = self.edge_detector.process_event(&event) {
            if self.handle_edge_transition(edge_result.edge, event).await? {
                // Edge transition handled
                return Ok(());
            }
        }

        // Check capture mode
        let capture_mode = self.capture_mode().await;
        if capture_mode.is_remote() {
            if let Some(target) = capture_mode.remote_target() {
                self.forward_event(target, event).await?;
            }
        }

        Ok(())
    }

    /// Stop the integrated engine
    pub async fn stop(&self) -> Result<()> {
        self.listener.stop().await?;
        tracing::info!("Integrated input engine stopped");
        Ok(())
    }
}

impl Clone for IntegratedInputEngine {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            capture_mode: self.capture_mode.clone(),
            edge_detector: EdgeDetector::new(self.edge_detector.config().clone()),
            listener: RDevInputListener::new(),
            emulator: self.emulator.clone(),
            event_channel: self.event_channel.clone(),
            tx_out: self.tx_out.clone(),
            shutdown_rx: self.shutdown_rx.resubscribe(),
            edge_targets: self.edge_targets.clone(),
            stats: self.stats.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = IntegratedEngineConfig::default();
        assert_eq!(config.screen_width, 1920);
        assert_eq!(config.screen_height, 1080);
    }

    #[test]
    fn test_config_from_screen_info() {
        let screen = ScreenInfo::new(0, 0, 2560, 1440);
        let config = IntegratedEngineConfig::from(&screen);
        assert_eq!(config.screen_width, 2560);
        assert_eq!(config.screen_height, 1440);
    }

    #[test]
    fn test_capture_mode() {
        let mode = CaptureMode::Local;
        assert!(mode.is_local());
        assert!(!mode.is_remote());

        let device = DeviceId::new_v4();
        let remote = CaptureMode::Remote { target: device };
        assert!(remote.is_remote());
        assert_eq!(remote.remote_target(), Some(device));
    }

    #[tokio::test]
    async fn test_set_edge_target() {
        let (tx_out, _rx) = mpsc::unbounded_channel();
        let (_shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let engine = IntegratedInputEngine::new(
            IntegratedEngineConfig::default(),
            tx_out,
            shutdown_rx,
        ).await.unwrap();

        let device = DeviceId::new_v4();
        engine.set_edge_target(Edge::Left, device).await;

        let targets = engine.edge_targets.lock().await;
        assert_eq!(targets.get(&Edge::Left), Some(&device));
    }
}
