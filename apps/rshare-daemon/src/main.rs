//! R-ShareMouse daemon service.
//!
//! Background service that handles input sharing and local IPC for status queries.

mod audio_runtime;
mod endpoint_runtime;
mod mobile_gateway;
mod static_capture;

use anyhow::{Context, Result};
use endpoint_runtime::inject_endpoint_event;
use futures_util::future::BoxFuture;
use rshare_core::{
    default_ipc_addr, default_local_controls_ws_addr, default_mobile_gateway_addr,
    local_capability_snapshots, remote_capability_snapshots, AudioFormat, BackendFailureReason,
    BackendHealth, BackendKind, BackendRuntimeState, CapabilityRegistrySnapshot, CapabilityState,
    CaptureSessionStateMachine, Config, ControlConnectionId, ControlSessionState,
    DaemonDeviceSnapshot, DaemonRequest, DaemonResponse, DeviceCapabilities,
    DeviceCapabilitySnapshot, DeviceId, DisplayCaptureResult, DisplayIdentifyResult, DisplayNode,
    DisplayOperationStatus, DisplaySettingsUpdateResult, EndpointCapabilityKind,
    EndpointCapabilitySnapshot, EndpointEvent, EndpointEventFilter, EndpointEventStore,
    EndpointInjectError, EndpointInjectRequest, EndpointInjectResult, EndpointInjectTarget,
    FeatureConfig, InputRouter, IpcEnvelopeKind, IpcFrame, IpcFrameCodec, LatencyFeedbackSnapshot,
    LatencyFeedbackStatus, LayoutGraph, LayoutNode, LocalAudioCaptureSource,
    LocalAudioCaptureStatus, LocalAudioTestResult, LocalAudioTestStatus,
    LocalControlDeviceSnapshot, LocalDisplayInfo, LocalDisplayState, LocalGamepadState,
    LocalInputDeviceKind, LocalInputDiagnosticEvent, LocalInputEventSource, LocalInputFeedback,
    LocalInputTestKind, LocalInputTestRequest, LocalInputTestResult, LocalInputTestStatus, Message,
    NetworkTransportSnapshot, RemoteDeviceLatencyFeedback, RemoteLatencyFeedback,
    RemoteUsbDeviceSnapshot, ResolvedInputMode, RouterCommand, ScreenInfo, ServiceStatusSnapshot,
    TransportFeedback, UiActiveSessions, UiDynamicState, UiPointerState, UiSnapshot,
    UsbControlSetupPacket, UsbDescriptorProbeResult, UsbDescriptorProbeStatus,
    UsbDeviceClaimRequest, UsbDeviceDescriptor, UsbDeviceSpeed, UsbTransferDirection,
    UsbTransferKind, UsbTransferPayload, UsbTransferStatus, VirtualDesktopGeometry,
    VirtualDisplayCreateRequest, VirtualDisplayOperationResult, VirtualDisplayOperationStatus,
    VirtualDisplayRemoveRequest, VirtualDisplaySnapshot, VirtualDisplayStatus,
    UI_STATE_PROTOCOL_VERSION,
};
#[cfg(test)]
use rshare_core::{Direction, UiChange, UiEnvelope};
use rshare_daemon::diagnostics_runtime::{
    DiagnosticPayload, DiagnosticPublicationItem, DiagnosticSubscriberId, DiagnosticsHandle,
    DiagnosticsRuntime, DiagnosticsSubscription, DIAGNOSTICS_HISTORY_CAPACITY,
};
use rshare_daemon::input_runtime::{
    dispatch_system_safety_event, run_authenticated_input_peers, InputForwardingPolicy,
    InputRuntime, LocalShortcutSuppressor,
};
use rshare_daemon::input_state::{input_state_channel, ControlMetricSnapshot, ControlMetrics};
use rshare_daemon::ipc_server::{
    handle_persistent_json_connection_with_first, read_json_request, stream_ui_state,
    ui_state_subscriber_for_request, write_json_response,
};
use rshare_daemon::state_aggregator::{StateAggregator, StateAggregatorHandle, UiProjectionSource};
use rshare_daemon::ui_state_server::{
    run_ui_state_server, LocalControlsFeed, LocalControlsSnapshotFuture,
};
use rshare_input::{
    BackendCandidate, BackendSelector, CaptureBackend, CaptureOrigin, CaptureSource,
    CapturedInputPayload, ContinuousInput, GamepadListenerConfig, GilrsGamepadListener,
    InjectBackend, InjectionActorConfig, InputEvent, InputInjectionHandle, PointerSample,
    PortableCaptureBackend, PortableInjectBackend, SemanticInputIngress, SemanticInputProducer,
};

#[cfg(not(windows))]
use rshare_input::RDevInputListener;
#[cfg(windows)]
use rshare_input::{DefaultInputListener, InputListener};
use rshare_net::{
    connection::{ConnectionInfo, ConnectionSnapshotReader, ConnectionState},
    qos::{ClassifiedMessage, ConnectionRegistry, TransportSendError},
    DiscoveredDevice, NetworkEvent, NetworkManager, NetworkManagerConfig, TelemetryFrame,
};
use tracing_subscriber::prelude::*;

#[cfg(windows)]
use rshare_platform::firewall;
use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::sync::{broadcast, oneshot, watch, Mutex, RwLock};
use tokio::time::{Duration, Instant};

#[derive(Clone)]
struct TrackedDevice {
    id: DeviceId,
    name: String,
    hostname: String,
    addresses: Vec<String>,
    connected: bool,
    discovered: bool,
    capabilities: DeviceCapabilities,
    last_seen_at: Instant,
}

#[derive(Debug, Clone)]
struct PendingLatencyProbe {
    target: DeviceId,
    sent_at_ms: u64,
    role: PendingLatencyProbeRole,
}

#[derive(Debug, Clone)]
enum PendingLatencyProbeRole {
    LocalRequested,
    EndpointSwitchReport {
        origin_device_id: DeviceId,
        origin_sequence: u64,
    },
}

struct PendingUsbClaim {
    target: DeviceId,
    bus_id: String,
    request_id: u64,
    transfer_id: u64,
    started_at_ms: u64,
    result_tx: oneshot::Sender<UsbDescriptorProbeResult>,
}

struct PendingUsbTransfer {
    target: DeviceId,
    bus_id: String,
    request_id: u64,
    transfer_id: u64,
    session_id: DeviceId,
    started_at_ms: u64,
    result_tx: oneshot::Sender<UsbDescriptorProbeResult>,
}

struct PendingEndpointInject {
    target: DeviceId,
    started_at_ms: u64,
    result_tx: oneshot::Sender<EndpointInjectResult>,
}

#[derive(Debug, Default)]
struct VirtualDisplayManager {
    displays: BTreeMap<String, VirtualDisplaySnapshot>,
}

const DEFAULT_VIRTUAL_DISPLAY_ID: &str = "rshare-vdisplay-1";

impl VirtualDisplayManager {
    fn list(&self) -> Vec<VirtualDisplaySnapshot> {
        self.displays.values().cloned().collect()
    }

    fn sync_platform_displays(&mut self, displays: Vec<VirtualDisplaySnapshot>) -> bool {
        let platform_ids = displays
            .iter()
            .map(|display| display.id.clone())
            .collect::<BTreeSet<_>>();
        let mut changed = false;

        for display in displays {
            if self.displays.get(&display.id) != Some(&display) {
                changed = true;
            }
            self.displays.insert(display.id.clone(), display);
        }

        for display in self.displays.values_mut() {
            if matches!(
                display.status,
                VirtualDisplayStatus::Active | VirtualDisplayStatus::Pending
            ) && !platform_ids.contains(&display.id)
            {
                display.status = VirtualDisplayStatus::Removed;
                display.display_id = None;
                changed = true;
            }
        }

        changed
    }

    fn create(&mut self, request: VirtualDisplayCreateRequest) -> VirtualDisplayOperationResult {
        self.create_with(
            request,
            rshare_platform::virtual_display::create_virtual_display,
        )
    }

    fn create_with<F>(
        &mut self,
        request: VirtualDisplayCreateRequest,
        create_virtual_display: F,
    ) -> VirtualDisplayOperationResult
    where
        F: FnOnce(&VirtualDisplayCreateRequest) -> Result<VirtualDisplayOperationResult>,
    {
        let id = virtual_display_request_id(request.id.as_deref());
        if let Some(existing) = self.displays.get(&id) {
            if !virtual_display_status_allows_create_retry(existing.status)
                && virtual_display_matches_create_request(existing, &request)
            {
                return VirtualDisplayOperationResult {
                    status: VirtualDisplayOperationStatus::AlreadyExists,
                    display: Some(existing.clone()),
                    message: Some(format!("virtual display id {id} already exists")),
                };
            }
        }

        if !valid_virtual_display_mode(request.width, request.height, request.refresh_rate_millihz)
        {
            return VirtualDisplayOperationResult {
                status: VirtualDisplayOperationStatus::InvalidMode,
                display: None,
                message: Some(
                    "virtual display width, height and refresh rate must be positive".to_string(),
                ),
            };
        }

        let mut platform_request = request;
        platform_request.id = Some(id.clone());
        match create_virtual_display(&platform_request) {
            Ok(result) => {
                if let Some(display) = result.display.clone() {
                    self.displays.insert(display.id.clone(), display);
                }
                result
            }
            Err(error) => VirtualDisplayOperationResult {
                status: VirtualDisplayOperationStatus::Failed,
                display: None,
                message: Some(error.to_string()),
            },
        }
    }

    fn remove(&mut self, request: VirtualDisplayRemoveRequest) -> VirtualDisplayOperationResult {
        self.remove_with(
            request,
            rshare_platform::virtual_display::remove_virtual_display,
        )
    }

    fn remove_with<F>(
        &mut self,
        request: VirtualDisplayRemoveRequest,
        remove_virtual_display: F,
    ) -> VirtualDisplayOperationResult
    where
        F: FnOnce(&VirtualDisplayRemoveRequest) -> Result<VirtualDisplayOperationResult>,
    {
        let id = request.id.trim().to_string();
        if id.is_empty() {
            return VirtualDisplayOperationResult {
                status: VirtualDisplayOperationStatus::Failed,
                display: None,
                message: Some("virtual display id is required".to_string()),
            };
        }

        let platform_result = remove_virtual_display(&request);
        self.displays.remove(&id);
        platform_result.unwrap_or_else(|error| VirtualDisplayOperationResult {
            status: VirtualDisplayOperationStatus::Failed,
            display: None,
            message: Some(error.to_string()),
        })
    }
}

fn virtual_display_request_id(id: Option<&str>) -> String {
    id.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| DEFAULT_VIRTUAL_DISPLAY_ID.to_string())
}

fn valid_virtual_display_mode(width: u32, height: u32, refresh_rate_millihz: Option<u32>) -> bool {
    width > 0 && height > 0 && refresh_rate_millihz.unwrap_or(60_000) > 0
}

fn virtual_display_status_allows_create_retry(status: VirtualDisplayStatus) -> bool {
    !matches!(
        status,
        VirtualDisplayStatus::Active | VirtualDisplayStatus::Pending
    )
}

fn virtual_display_matches_create_request(
    display: &VirtualDisplaySnapshot,
    request: &VirtualDisplayCreateRequest,
) -> bool {
    display.width == request.width
        && display.height == request.height
        && display.refresh_rate_millihz == request.refresh_rate_millihz.or(Some(60_000))
}

#[derive(Debug, Clone)]
struct RuntimeFeatureConfig {
    mobile_gateway_enabled: bool,
    suppress_local_shortcuts_when_remote: bool,
    automatic_input_forwarding: bool,
    auto_endpoint_latency_probe: bool,
    audio_capture: bool,
    audio_forwarding: bool,
    usb_forwarding_experimental: bool,
    usb_device_advertising: bool,
    usb_descriptor_probe: bool,
}

impl RuntimeFeatureConfig {
    fn from_config(config: &Config) -> Self {
        Self {
            mobile_gateway_enabled: config.features.mobile_gateway_enabled,
            suppress_local_shortcuts_when_remote: config
                .features
                .suppress_local_shortcuts_when_remote,
            automatic_input_forwarding: config.features.automatic_input_forwarding,
            auto_endpoint_latency_probe: config.features.auto_endpoint_latency_probe,
            audio_capture: config.features.audio_capture,
            audio_forwarding: config.features.audio_forwarding,
            usb_forwarding_experimental: config.features.usb_forwarding_experimental,
            usb_device_advertising: config.features.usb_device_advertising,
            usb_descriptor_probe: config.features.usb_descriptor_probe,
        }
    }

    fn usb_advertising_enabled(&self) -> bool {
        self.usb_forwarding_experimental && self.usb_device_advertising
    }

    fn to_feature_config(&self) -> FeatureConfig {
        let mut features = FeatureConfig::default();
        features.mobile_gateway_enabled = self.mobile_gateway_enabled;
        features.suppress_local_shortcuts_when_remote = self.suppress_local_shortcuts_when_remote;
        features.automatic_input_forwarding = self.automatic_input_forwarding;
        features.auto_endpoint_latency_probe = self.auto_endpoint_latency_probe;
        features.audio_capture = self.audio_capture;
        features.audio_forwarding = self.audio_forwarding;
        features.usb_forwarding_experimental = self.usb_forwarding_experimental;
        features.usb_device_advertising = self.usb_device_advertising;
        features.usb_descriptor_probe = self.usb_descriptor_probe;
        features
    }
}

impl Default for RuntimeFeatureConfig {
    fn default() -> Self {
        Self::from_config(&Config::default())
    }
}

struct DaemonState {
    status: ServiceStatusSnapshot,
    devices: HashMap<DeviceId, TrackedDevice>,
    // Layout and routing state
    layout: LayoutGraph,
    session: CaptureSessionStateMachine,
    // Backend state with separate capture/inject health
    backend_state: BackendRuntimeState,
    features: RuntimeFeatureConfig,
    local_controls: LocalControlDeviceSnapshot,
    endpoint_events: EndpointEventStore,
    pending_keyboard_loopback_until_ms: u64,
    pending_keyboard_loopback_events: u8,
    pending_mouse_loopback_until_ms: u64,
    pending_mouse_loopback_events: u8,
    pending_latency_probes: HashMap<u64, PendingLatencyProbe>,
    pending_usb_claims: HashMap<u64, PendingUsbClaim>,
    pending_usb_transfers: HashMap<u64, PendingUsbTransfer>,
    pending_endpoint_injects: HashMap<String, PendingEndpointInject>,
    virtual_displays: VirtualDisplayManager,
    mobile_access: mobile_gateway::MobileGatewayAccess,
}

impl DaemonState {
    #[cfg(test)]
    fn new(status: ServiceStatusSnapshot) -> Self {
        Self::new_with_features(status, RuntimeFeatureConfig::default())
    }

    fn new_with_features(status: ServiceStatusSnapshot, features: RuntimeFeatureConfig) -> Self {
        let local_id = status.device_id;
        let mut layout = LayoutGraph::new(local_id);
        let local_screen = current_primary_screen_info();
        layout.add_node(LayoutNode::new(
            local_id,
            0,
            0,
            local_screen.width,
            local_screen.height,
        ));

        let mut backend_state = BackendRuntimeState::new();
        backend_state.available_backends = vec![BackendKind::Portable];
        let local_controls =
            default_local_control_snapshot(local_screen.width, local_screen.height, &features);
        let mobile_access = if features.mobile_gateway_enabled {
            mobile_gateway::MobileGatewayAccess::new(
                default_mobile_gateway_addr(),
                DeviceId::new_v4().simple().to_string(),
                mobile_gateway::preferred_mobile_advertise_host(&status.hostname),
            )
        } else {
            mobile_gateway::MobileGatewayAccess::disabled(
                "mobile gateway disabled by configuration".to_string(),
            )
        };

        Self {
            status,
            devices: HashMap::new(),
            layout,
            session: CaptureSessionStateMachine::new(),
            backend_state,
            features,
            local_controls,
            endpoint_events: EndpointEventStore::default(),
            pending_keyboard_loopback_until_ms: 0,
            pending_keyboard_loopback_events: 0,
            pending_mouse_loopback_until_ms: 0,
            pending_mouse_loopback_events: 0,
            pending_latency_probes: HashMap::new(),
            pending_usb_claims: HashMap::new(),
            pending_usb_transfers: HashMap::new(),
            pending_endpoint_injects: HashMap::new(),
            virtual_displays: VirtualDisplayManager::default(),
            mobile_access,
        }
    }

    fn upsert_discovered(&mut self, device: DiscoveredDevice) -> bool {
        let connected = self
            .devices
            .get(&device.id)
            .map(|existing| existing.connected)
            .unwrap_or(false);
        let screen_info = device.screen_info.clone();
        self.devices.insert(
            device.id,
            TrackedDevice {
                id: device.id,
                name: device.name,
                hostname: device.hostname,
                addresses: device
                    .addresses
                    .into_iter()
                    .map(|addr| addr.to_string())
                    .collect(),
                connected,
                discovered: true,
                capabilities: device.capabilities,
                last_seen_at: Instant::now(),
            },
        );
        self.layout
            .merge_discovered_peers_to_right_with_screens([(device.id, screen_info)])
    }

    fn remove_device(&mut self, id: &DeviceId) {
        self.devices.remove(id);
        self.clear_pending_latency_probes_for(*id);
        self.local_controls
            .remote_usb_devices
            .retain(|device| device.device_id != *id);
    }

    fn clear_pending_latency_probes_for(&mut self, id: DeviceId) {
        self.pending_latency_probes
            .retain(|_, probe| probe.target != id);
    }

    fn mark_connected(&mut self, id: &DeviceId, connected: bool) -> bool {
        if let Some(device) = self.devices.get_mut(id) {
            device.connected = connected;
            device.last_seen_at = Instant::now();
        } else if connected {
            self.devices.insert(
                *id,
                TrackedDevice {
                    id: *id,
                    name: format!("Device {}", short_device_id(*id)),
                    hostname: "unknown".to_string(),
                    addresses: Vec::new(),
                    connected: true,
                    discovered: false,
                    capabilities: DeviceCapabilities::default(),
                    last_seen_at: Instant::now(),
                },
            );
        }
        let mut layout_changed = false;
        if connected {
            layout_changed = self.layout.merge_discovered_peers_to_right([*id]);
        }
        if !connected {
            self.clear_pending_latency_probes_for(*id);
        }
        for device in &mut self.local_controls.remote_usb_devices {
            if device.device_id == *id {
                device.connected = connected;
            }
        }
        layout_changed
    }

    /// Discovery loss only removes an unconnected peer from live visibility.
    /// Layout nodes are persisted topology and never removed by discovery churn.
    fn mark_discovery_lost(&mut self, id: &DeviceId) {
        if self.devices.get(id).is_some_and(|device| !device.connected) {
            self.remove_device(id);
        } else if let Some(device) = self.devices.get_mut(id) {
            device.discovered = false;
        }
    }

    /// A transport disconnect preserves a peer that is still present in discovery.
    fn mark_disconnected(&mut self, id: &DeviceId) {
        self.mark_connected(id, false);
        if self
            .devices
            .get(id)
            .is_some_and(|device| !device.discovered)
        {
            self.remove_device(id);
        }
    }

    fn status_snapshot(&self) -> ServiceStatusSnapshot {
        let mut snapshot = self.status.clone();
        snapshot.discovered_devices = self.devices.len();
        snapshot.connected_devices = self
            .devices
            .values()
            .filter(|device| device.connected)
            .count();

        // Populate backend status fields from BackendRuntimeState
        snapshot.input_mode = self.backend_state.selected_mode;
        snapshot.available_backends = Some(self.backend_state.available_backends.clone());
        snapshot.backend_health = Some(self.backend_state.aggregate_health.clone());
        snapshot.privilege_state = Some(self.backend_state.privilege_state);
        snapshot.last_backend_error = self.backend_state.last_error.clone();

        // Populate session state from CaptureSessionStateMachine
        snapshot.session_state = Some(self.session.state());
        snapshot.active_target = self.session.active_target();

        snapshot
    }

    fn latency_feedback_snapshot(&self, transport: TransportFeedback) -> LatencyFeedbackSnapshot {
        let now_ms = timestamp_ms_now();

        LatencyFeedbackSnapshot {
            generated_at_ms: now_ms,
            local_input: self.local_input_feedback(),
            remote_latency: self.remote_latency_feedback(now_ms),
            transport,
        }
    }

    fn status_snapshot_with_network_and_transport(
        &self,
        network: &NetworkTransportSnapshot,
        transport: TransportFeedback,
    ) -> ServiceStatusSnapshot {
        let mut snapshot = self.status_snapshot();
        snapshot.network = network.clone();
        snapshot.latency_feedback = self.latency_feedback_snapshot(transport);
        snapshot
    }

    fn status_snapshot_for_connections(
        &self,
        connection_infos: &[ConnectionInfo],
    ) -> ServiceStatusSnapshot {
        let network = network_snapshot_from_connections(connection_infos);
        let transport = transport_feedback_from_connections(&network, connection_infos);
        let mut snapshot = self.status_snapshot_with_network_and_transport(&network, transport);
        snapshot.connected_devices = connected_connection_count(connection_infos);
        snapshot
    }

    fn device_snapshots(&self) -> Vec<DaemonDeviceSnapshot> {
        let mut devices: Vec<_> = self
            .devices
            .values()
            .map(|device| DaemonDeviceSnapshot {
                id: device.id,
                name: device.name.clone(),
                hostname: device.hostname.clone(),
                addresses: device.addresses.clone(),
                connected: device.connected,
                last_seen_secs: Some(device.last_seen_at.elapsed().as_secs()),
            })
            .collect();

        devices.sort_by(|left, right| left.name.cmp(&right.name));
        devices
    }

    fn capability_registry_snapshot(
        &self,
        network: &NetworkTransportSnapshot,
        device_id_filter: Option<DeviceId>,
    ) -> CapabilityRegistrySnapshot {
        let mut devices = Vec::with_capacity(self.devices.len() + 1);
        let mut local_capabilities = local_capability_snapshots(
            &self.backend_state,
            &self.local_controls,
            network,
            &self.features.to_feature_config(),
        );
        enrich_display_topology_from_layout(
            &mut local_capabilities,
            self.layout.get_node(self.status.device_id),
        );
        devices.push(DeviceCapabilitySnapshot {
            device_id: self.status.device_id,
            device_name: self.status.device_name.clone(),
            hostname: self.status.hostname.clone(),
            connected: true,
            capabilities: local_capabilities,
        });

        devices.extend(self.devices.values().map(|device| {
            let mut capabilities =
                remote_capability_snapshots(&device.capabilities, device.connected);
            enrich_display_topology_from_layout(&mut capabilities, self.layout.get_node(device.id));
            DeviceCapabilitySnapshot {
                device_id: device.id,
                device_name: device.name.clone(),
                hostname: device.hostname.clone(),
                connected: device.connected,
                capabilities,
            }
        }));

        devices.sort_by(|left, right| {
            left.device_name
                .cmp(&right.device_name)
                .then_with(|| left.device_id.cmp(&right.device_id))
        });

        if let Some(device_id) = device_id_filter {
            devices.retain(|device| device.device_id == device_id);
        }

        CapabilityRegistrySnapshot {
            local_device_id: self.status.device_id,
            generated_at_ms: timestamp_ms_now(),
            devices,
        }
    }

    fn reconcile_local_layout_geometry(&mut self) -> bool {
        let displays = display_nodes_from_local_display_state(&self.local_controls.display);
        upsert_layout_node_displays(&mut self.layout, self.status.device_id, displays)
    }

    fn update_backend_state(
        &mut self,
        mode: Option<ResolvedInputMode>,
        available: Vec<BackendKind>,
        capture_health: BackendHealth,
        inject_kind: BackendKind,
        inject_health: BackendHealth,
        text_commit_supported: bool,
        error: Option<String>,
    ) {
        self.backend_state.selected_mode = mode;
        self.backend_state.available_backends = available;
        self.backend_state.capture_health = capture_health.clone();
        self.backend_state.inject_health = inject_health.clone();
        self.backend_state.last_error = error.clone();
        self.backend_state.update_aggregate_health();
        self.local_controls.capture_backend.mode = mode;
        self.local_controls.capture_backend.kind = mode.map(backend_kind_from_resolved_mode);
        self.local_controls.capture_backend.health = Some(capture_health.clone());
        self.local_controls.capture_backend.active =
            matches!(capture_health, BackendHealth::Healthy);
        self.local_controls.inject_backend.mode = mode;
        self.local_controls.inject_backend.kind = Some(inject_kind);
        self.local_controls.inject_backend.health = Some(inject_health.clone());
        self.local_controls.inject_backend.active = matches!(inject_health, BackendHealth::Healthy);
        self.local_controls.inject_backend.text_commit_supported = text_commit_supported;
        self.local_controls.privilege_state = Some(self.backend_state.privilege_state);
        self.local_controls.last_error = error;

        // Notify session machine if backend is degraded
        if matches!(
            self.backend_state.aggregate_health,
            BackendHealth::Degraded { .. }
        ) {
            self.session.on_backend_degraded();
        }
    }

    fn local_control_snapshot(&self) -> LocalControlDeviceSnapshot {
        self.local_controls.clone()
    }

    fn ui_snapshot(&self) -> UiSnapshot {
        let status = self.status_snapshot();
        let diagnostics = status.latency_feedback.clone();
        let capabilities = self.capability_registry_snapshot(&status.network, None);
        UiSnapshot {
            protocol_version: UI_STATE_PROTOCOL_VERSION,
            boot_id: DeviceId::nil(),
            revision: 0,
            status,
            devices: self.device_snapshots(),
            layout: self.layout.clone(),
            capabilities,
            display_inventory: self.local_controls.display.clone(),
            dynamic_state: UiDynamicState {
                pointer: Some(UiPointerState {
                    x: self.local_controls.mouse.x,
                    y: self.local_controls.mouse.y,
                    display_id: self.local_controls.mouse.current_display_id.clone(),
                    observed_at_ms: timestamp_ms_now(),
                }),
                gamepads: self.local_controls.gamepads.clone(),
                diagnostics,
                ..UiDynamicState::default()
            },
            active_sessions: UiActiveSessions {
                control: Some(self.session.state()),
                media_sessions: Vec::new(),
            },
        }
    }

    fn ui_snapshot_for_connections(&self, connection_infos: &[ConnectionInfo]) -> UiSnapshot {
        let mut snapshot = self.ui_snapshot();
        let status = self.status_snapshot_for_connections(connection_infos);
        snapshot.capabilities = self.capability_registry_snapshot(&status.network, None);
        snapshot.dynamic_state.diagnostics = status.latency_feedback.clone();
        snapshot.status = status;
        snapshot
    }

    fn local_input_feedback(&self) -> LocalInputFeedback {
        let event_count = self
            .local_controls
            .keyboard
            .event_count
            .saturating_add(self.local_controls.mouse.event_count)
            .saturating_add(local_gamepad_event_count(&self.local_controls));

        if self.backend_state.selected_mode.is_none() {
            return LocalInputFeedback {
                status: LatencyFeedbackStatus::Unavailable,
                event_count,
                ..LocalInputFeedback::default()
            };
        }

        let latest_event = self
            .local_controls
            .recent_events
            .iter()
            .filter(|event| is_eligible_local_input_feedback_event(event))
            .max_by_key(|event| (event.timestamp_ms, event.sequence));
        let latest_keyboard = self
            .local_controls
            .recent_events
            .iter()
            .filter(|event| {
                is_eligible_local_input_feedback_event(event)
                    && event.device_kind == LocalInputDeviceKind::Keyboard
            })
            .max_by_key(|event| (event.timestamp_ms, event.sequence));
        let latest_mouse = self
            .local_controls
            .recent_events
            .iter()
            .filter(|event| {
                is_eligible_local_input_feedback_event(event)
                    && event.device_kind == LocalInputDeviceKind::Mouse
            })
            .max_by_key(|event| (event.timestamp_ms, event.sequence));
        let latest_gamepad = self
            .local_controls
            .recent_events
            .iter()
            .filter(|event| {
                is_eligible_local_input_feedback_event(event)
                    && event.device_kind == LocalInputDeviceKind::Gamepad
            })
            .max_by_key(|event| (event.timestamp_ms, event.sequence));

        LocalInputFeedback {
            status: if matches!(
                self.backend_state.aggregate_health,
                BackendHealth::Degraded { .. }
            ) {
                LatencyFeedbackStatus::Degraded
            } else if event_count == 0 {
                LatencyFeedbackStatus::Idle
            } else {
                LatencyFeedbackStatus::Healthy
            },
            event_count,
            latest_sequence: latest_event.map(|event| event.sequence),
            latest_event_ms: latest_event.map(|event| event.timestamp_ms),
            latest_keyboard_event_ms: latest_keyboard.map(|event| event.timestamp_ms),
            latest_mouse_event_ms: latest_mouse.map(|event| event.timestamp_ms),
            latest_gamepad_event_ms: latest_gamepad.map(|event| event.timestamp_ms),
            latest_gamepad_id: latest_gamepad
                .and_then(|event| event.payload.get("gamepad_id"))
                .and_then(|value| value.parse::<u8>().ok()),
            latest_gamepad_event_kind: latest_gamepad.map(|event| event.event_kind.clone()),
            latest_gamepad_button: latest_gamepad
                .and_then(|event| {
                    event
                        .payload
                        .get("last_button")
                        .or_else(|| event.payload.get("button"))
                })
                .cloned(),
            latest_gamepad_axis: latest_gamepad
                .and_then(|event| event.payload.get("last_axis"))
                .cloned(),
            capture_path: latest_event.and_then(|event| event.capture_path.clone()),
        }
    }

    fn remote_latency_feedback(&self, now_ms: u64) -> RemoteLatencyFeedback {
        let mut tracked_devices: Vec<_> = self.devices.values().collect();
        tracked_devices.sort_by_key(|device| device.id.to_string());

        let devices = tracked_devices
            .into_iter()
            .map(|device| {
                let mut feedback = RemoteDeviceLatencyFeedback {
                    device_id: device.id,
                    status: LatencyFeedbackStatus::Unavailable,
                    device_name: Some(device.name.clone()),
                    latest_sequence: None,
                    last_probe_sent_ms: None,
                    last_ack_ms: None,
                    pending_duration_ms: None,
                    network_round_trip_ms: None,
                    raw_round_trip_ms: None,
                    estimated_one_way_ms: None,
                    remote_processing_ms: None,
                    direction: None,
                    summary: None,
                };

                if !device.connected {
                    return feedback;
                }

                let latest_pending = self
                    .pending_latency_probes
                    .iter()
                    .filter(|(_, probe)| probe.target == device.id)
                    .max_by_key(|(sequence, probe)| (probe.sent_at_ms, **sequence));
                let latest_ack = self
                    .local_controls
                    .recent_events
                    .iter()
                    .filter(|event| is_latency_ack_event(event))
                    .filter(|event| latency_event_matches_target(event, device.id))
                    .max_by_key(|event| latency_ack_order_key(event));

                if let Some((pending_sequence, pending)) = latest_pending {
                    let pending_is_newest = latest_ack
                        .map(|ack| {
                            latency_ack_completion_sequence_for_pending(ack, pending)
                                .map(|ack_sequence| *pending_sequence > ack_sequence)
                                .unwrap_or_else(|| *pending_sequence > ack.sequence)
                        })
                        .unwrap_or(true);
                    if pending_is_newest {
                        let pending_duration_ms = now_ms.saturating_sub(pending.sent_at_ms);
                        feedback.status = if pending_duration_ms > LATENCY_PROBE_TIMEOUT_MS {
                            LatencyFeedbackStatus::Timeout
                        } else {
                            LatencyFeedbackStatus::Pending
                        };
                        feedback.latest_sequence = Some(*pending_sequence);
                        feedback.last_probe_sent_ms = Some(pending.sent_at_ms);
                        feedback.pending_duration_ms = Some(pending_duration_ms);
                        return feedback;
                    }
                }

                if let Some(ack) = latest_ack {
                    let network_round_trip_ms = parse_latency_payload_u64(
                        &ack.payload,
                        &["network_round_trip_ms", "latency_ms"],
                    );
                    feedback.status =
                        if network_round_trip_ms.is_some_and(|rtt| rtt <= LATENCY_HEALTHY_RTT_MS) {
                            LatencyFeedbackStatus::Healthy
                        } else {
                            LatencyFeedbackStatus::Degraded
                        };
                    feedback.latest_sequence = Some(ack.sequence);
                    feedback.last_ack_ms = Some(ack.timestamp_ms);
                    feedback.network_round_trip_ms = network_round_trip_ms;
                    feedback.raw_round_trip_ms = parse_latency_payload_u64(
                        &ack.payload,
                        &["raw_round_trip_ms", "raw_latency_ms"],
                    );
                    feedback.estimated_one_way_ms =
                        parse_latency_payload_u64(&ack.payload, &["estimated_one_way_ms"]);
                    feedback.remote_processing_ms =
                        parse_latency_payload_u64(&ack.payload, &["remote_processing_ms"]);
                    feedback.direction = ack.payload.get("direction").cloned();
                    feedback.summary = Some(ack.summary.clone());
                } else {
                    feedback.status = LatencyFeedbackStatus::Idle;
                }

                feedback
            })
            .collect::<Vec<_>>();

        let status = devices
            .iter()
            .map(|device| device.status)
            .max_by_key(|status| latency_feedback_status_priority(*status))
            .unwrap_or(LatencyFeedbackStatus::Unavailable);

        RemoteLatencyFeedback { status, devices }
    }

    fn sync_endpoint_events_from_recent(&mut self) {
        let endpoint_id = self.status.device_id;
        let last_sequence = self.endpoint_events.last_sequence().unwrap_or_default();
        for event in self
            .local_controls
            .recent_events
            .iter()
            .filter(|event| event.sequence > last_sequence)
            .cloned()
        {
            self.endpoint_events
                .push(EndpointEvent::from_local_diagnostic(endpoint_id, event));
        }
    }

    fn endpoint_events(
        &mut self,
        filter: &EndpointEventFilter,
        after_sequence: Option<u64>,
        limit: Option<u16>,
    ) -> Vec<EndpointEvent> {
        self.sync_endpoint_events_from_recent();
        self.endpoint_events.query(filter, after_sequence, limit)
    }

    fn endpoint_event_from_local(&mut self, event: LocalInputDiagnosticEvent) -> EndpointEvent {
        let endpoint_event = EndpointEvent::from_local_diagnostic(self.status.device_id, event);
        self.endpoint_events.push(endpoint_event.clone());
        endpoint_event
    }

    fn mirror_remote_endpoint_event(
        &mut self,
        from: DeviceId,
        mut event: EndpointEvent,
    ) -> EndpointEvent {
        let sequence = self.local_controls.sequence.saturating_add(1);
        self.local_controls.sequence = sequence;
        event.event_id = sequence;
        event.sequence = sequence;
        event.endpoint_id = from;
        if event.origin_endpoint_id == DeviceId::nil() {
            event.origin_endpoint_id = from;
        }
        event.source = rshare_core::EndpointEventSource::RemoteMirror;
        self.endpoint_events.push(event.clone());
        event
    }

    fn complete_pending_endpoint_inject(
        &mut self,
        from: DeviceId,
        mut result: EndpointInjectResult,
    ) -> bool {
        let Some(pending) = self.pending_endpoint_injects.remove(&result.correlation_id) else {
            return false;
        };
        result.target = EndpointInjectTarget::Remote(from);
        result.elapsed_ms = timestamp_ms_now().saturating_sub(pending.started_at_ms);
        if pending.target != from {
            result.accepted = false;
            result.error = Some(EndpointInjectError::TransportFailed);
        }
        let _ = pending.result_tx.send(result);
        true
    }

    fn refresh_local_controls_platform(&mut self) {
        let features = self.features.clone();
        refresh_platform_local_controls(&mut self.local_controls, &features);
        self.reconcile_local_layout_geometry();
    }

    fn arm_injected_loopback(&mut self, device_kind: LocalInputDeviceKind, timestamp_ms: u64) {
        let until_ms = timestamp_ms.saturating_add(INJECTION_LOOPBACK_WINDOW_MS);
        match device_kind {
            LocalInputDeviceKind::Keyboard => {
                self.pending_keyboard_loopback_until_ms = until_ms;
                self.pending_keyboard_loopback_events = 4;
            }
            LocalInputDeviceKind::Mouse => {
                self.pending_mouse_loopback_until_ms = until_ms;
                self.pending_mouse_loopback_events = 4;
            }
            _ => {}
        }
    }

    fn local_input_source_for_event(
        &mut self,
        device_kind: LocalInputDeviceKind,
        timestamp_ms: u64,
        payload: &mut BTreeMap<String, String>,
    ) -> LocalInputEventSource {
        let (until_ms, budget) = match device_kind {
            LocalInputDeviceKind::Keyboard => (
                &mut self.pending_keyboard_loopback_until_ms,
                &mut self.pending_keyboard_loopback_events,
            ),
            LocalInputDeviceKind::Mouse => (
                &mut self.pending_mouse_loopback_until_ms,
                &mut self.pending_mouse_loopback_events,
            ),
            _ => return LocalInputEventSource::Hardware,
        };

        if *budget > 0 && timestamp_ms <= *until_ms {
            *budget = budget.saturating_sub(1);
            payload.insert(
                "source_note".to_string(),
                "possible daemon injection loopback".to_string(),
            );
            LocalInputEventSource::InjectedLoopback
        } else {
            *budget = 0;
            LocalInputEventSource::Hardware
        }
    }

    #[cfg(test)]
    fn record_local_input_event(&mut self, event: &InputEvent) -> LocalInputDiagnosticEvent {
        self.record_local_input_event_with_metadata(event, None)
    }

    fn record_local_input_event_with_metadata(
        &mut self,
        event: &InputEvent,
        metadata: Option<&LocalInputDeviceMetadata>,
    ) -> LocalInputDiagnosticEvent {
        let sequence = self.local_controls.sequence.saturating_add(1);
        self.local_controls.sequence = sequence;
        let timestamp_ms = timestamp_ms_now();
        let mut payload = BTreeMap::new();

        let (device_kind, event_kind, summary) = match event {
            InputEvent::MouseMove { x, y } => {
                self.local_controls.mouse.detected = true;
                self.local_controls.mouse.x = *x;
                self.local_controls.mouse.y = *y;
                self.local_controls.mouse.event_count =
                    self.local_controls.mouse.event_count.saturating_add(1);
                self.local_controls.mouse.move_count =
                    self.local_controls.mouse.move_count.saturating_add(1);
                update_mouse_display_position(&mut self.local_controls);
                payload.insert("x".to_string(), x.to_string());
                payload.insert("y".to_string(), y.to_string());
                insert_mouse_position_payload(&self.local_controls, &mut payload);
                (
                    LocalInputDeviceKind::Mouse,
                    "move".to_string(),
                    format!("Mouse move {}, {}", x, y),
                )
            }
            InputEvent::MouseButton { button, state } => {
                self.local_controls.mouse.detected = true;
                self.local_controls.mouse.event_count =
                    self.local_controls.mouse.event_count.saturating_add(1);
                self.local_controls.mouse.button_event_count = self
                    .local_controls
                    .mouse
                    .button_event_count
                    .saturating_add(1);
                let button = format!("{:?}", button);
                if state.is_pressed() {
                    self.local_controls.mouse.button_press_count = self
                        .local_controls
                        .mouse
                        .button_press_count
                        .saturating_add(1);
                    push_unique(
                        &mut self.local_controls.mouse.pressed_buttons,
                        button.clone(),
                    );
                } else {
                    self.local_controls.mouse.button_release_count = self
                        .local_controls
                        .mouse
                        .button_release_count
                        .saturating_add(1);
                    remove_value(&mut self.local_controls.mouse.pressed_buttons, &button);
                }
                update_mouse_display_position(&mut self.local_controls);
                payload.insert("button".to_string(), button.clone());
                payload.insert("state".to_string(), format!("{:?}", state));
                payload.insert("x".to_string(), self.local_controls.mouse.x.to_string());
                payload.insert("y".to_string(), self.local_controls.mouse.y.to_string());
                insert_mouse_position_payload(&self.local_controls, &mut payload);
                (
                    LocalInputDeviceKind::Mouse,
                    "button".to_string(),
                    format!("Mouse {} {:?}", button, state),
                )
            }
            InputEvent::MouseWheel { delta_x, delta_y } => {
                self.local_controls.mouse.detected = true;
                self.local_controls.mouse.wheel_delta_x = *delta_x;
                self.local_controls.mouse.wheel_delta_y = *delta_y;
                self.local_controls.mouse.event_count =
                    self.local_controls.mouse.event_count.saturating_add(1);
                self.local_controls.mouse.wheel_event_count = self
                    .local_controls
                    .mouse
                    .wheel_event_count
                    .saturating_add(1);
                self.local_controls.mouse.wheel_total_x = self
                    .local_controls
                    .mouse
                    .wheel_total_x
                    .saturating_add(*delta_x as i64);
                self.local_controls.mouse.wheel_total_y = self
                    .local_controls
                    .mouse
                    .wheel_total_y
                    .saturating_add(*delta_y as i64);
                update_mouse_display_position(&mut self.local_controls);
                payload.insert("delta_x".to_string(), delta_x.to_string());
                payload.insert("delta_y".to_string(), delta_y.to_string());
                payload.insert(
                    "total_x".to_string(),
                    self.local_controls.mouse.wheel_total_x.to_string(),
                );
                payload.insert(
                    "total_y".to_string(),
                    self.local_controls.mouse.wheel_total_y.to_string(),
                );
                payload.insert("x".to_string(), self.local_controls.mouse.x.to_string());
                payload.insert("y".to_string(), self.local_controls.mouse.y.to_string());
                insert_mouse_position_payload(&self.local_controls, &mut payload);
                (
                    LocalInputDeviceKind::Mouse,
                    "wheel".to_string(),
                    format!("Mouse wheel {}, {}", delta_x, delta_y),
                )
            }
            InputEvent::Key { keycode, state } | InputEvent::KeyExtended { keycode, state, .. } => {
                self.local_controls.keyboard.detected = true;
                self.local_controls.keyboard.event_count =
                    self.local_controls.keyboard.event_count.saturating_add(1);
                let key = format!("{:?}", keycode);
                self.local_controls.keyboard.last_key = Some(key.clone());
                if state.is_pressed() {
                    push_unique(&mut self.local_controls.keyboard.pressed_keys, key.clone());
                } else {
                    remove_value(&mut self.local_controls.keyboard.pressed_keys, &key);
                }
                payload.insert("key".to_string(), key.clone());
                payload.insert("state".to_string(), format!("{:?}", state));
                (
                    LocalInputDeviceKind::Keyboard,
                    "key".to_string(),
                    format!("Key {} {:?}", key, state),
                )
            }
            InputEvent::TextCommit { text } => {
                self.local_controls.keyboard.detected = true;
                self.local_controls.keyboard.event_count =
                    self.local_controls.keyboard.event_count.saturating_add(1);
                self.local_controls.keyboard.last_key = Some("TextCommit".to_string());
                payload.insert("char_count".to_string(), text.chars().count().to_string());
                (
                    LocalInputDeviceKind::Keyboard,
                    "text".to_string(),
                    format!("Text commit {} chars", text.chars().count()),
                )
            }
            InputEvent::GamepadConnected { info } => {
                upsert_gamepad_metadata(
                    &mut self.local_controls,
                    info.gamepad_id,
                    &info.name,
                    true,
                );
                payload.insert("gamepad_id".to_string(), info.gamepad_id.to_string());
                payload.insert("name".to_string(), info.name.clone());
                (
                    LocalInputDeviceKind::Gamepad,
                    "connected".to_string(),
                    format!("Gamepad connected: {}", info.name),
                )
            }
            InputEvent::GamepadDisconnected { gamepad_id } => {
                if let Some(gamepad) = self
                    .local_controls
                    .gamepads
                    .iter_mut()
                    .find(|gamepad| gamepad.gamepad_id == *gamepad_id)
                {
                    gamepad.connected = false;
                    gamepad.event_count = gamepad.event_count.saturating_add(1);
                    gamepad.last_seen_ms = timestamp_ms;
                }
                payload.insert("gamepad_id".to_string(), gamepad_id.to_string());
                (
                    LocalInputDeviceKind::Gamepad,
                    "disconnected".to_string(),
                    format!("Gamepad disconnected: {}", gamepad_id),
                )
            }
            InputEvent::GamepadButton {
                gamepad_id,
                button,
                pressed,
                state_after,
            } => {
                let existing = self
                    .local_controls
                    .gamepads
                    .iter()
                    .find(|gamepad| gamepad.gamepad_id == *gamepad_id);
                let existing_name = existing.map(|gamepad| gamepad.name.clone());
                let mut next = LocalGamepadState::from_state(state_after, existing_name, true);
                if let Some(existing) = existing {
                    next.event_count = existing.event_count.saturating_add(1);
                    next.button_event_count = existing.button_event_count.saturating_add(1);
                    next.button_press_count = existing
                        .button_press_count
                        .saturating_add(u64::from(*pressed));
                    next.button_release_count = existing
                        .button_release_count
                        .saturating_add(u64::from(!*pressed));
                    next.axis_event_count = existing.axis_event_count;
                    next.trigger_event_count = existing.trigger_event_count;
                    next.last_axis = existing.last_axis.clone();
                } else {
                    next.button_event_count = 1;
                    next.button_press_count = u64::from(*pressed);
                    next.button_release_count = u64::from(!*pressed);
                }
                let button_label = format!("{button:?}");
                next.last_button = Some(button_label.clone());
                payload.insert("gamepad_id".to_string(), gamepad_id.to_string());
                payload.insert("button".to_string(), button_label.clone());
                payload.insert("pressed".to_string(), pressed.to_string());
                upsert_gamepad_state(&mut self.local_controls, next);
                (
                    LocalInputDeviceKind::Gamepad,
                    "button".to_string(),
                    format!(
                        "Gamepad {} button {} {}",
                        gamepad_id,
                        button_label,
                        if *pressed { "pressed" } else { "released" }
                    ),
                )
            }
            InputEvent::GamepadState { state } => {
                let existing = self
                    .local_controls
                    .gamepads
                    .iter()
                    .find(|gamepad| gamepad.gamepad_id == state.gamepad_id);
                let existing_name = existing.map(|gamepad| gamepad.name.clone());
                let mut next = LocalGamepadState::from_state(state, existing_name, true);
                let button_delta = gamepad_button_delta(existing, state);
                let axis_delta = gamepad_axis_delta(existing, state);
                if let Some(existing) = existing {
                    next.event_count = existing.event_count.saturating_add(1);
                    next.button_event_count = existing
                        .button_event_count
                        .saturating_add(button_delta.event_count);
                    next.button_press_count = existing
                        .button_press_count
                        .saturating_add(button_delta.press_count);
                    next.button_release_count = existing
                        .button_release_count
                        .saturating_add(button_delta.release_count);
                    next.axis_event_count = existing
                        .axis_event_count
                        .saturating_add(if axis_delta.stick_changed { 1 } else { 0 });
                    next.trigger_event_count = existing
                        .trigger_event_count
                        .saturating_add(if axis_delta.trigger_changed { 1 } else { 0 });
                    next.last_button = button_delta
                        .last_button
                        .clone()
                        .or_else(|| existing.last_button.clone());
                    next.last_axis = axis_delta
                        .last_axis
                        .clone()
                        .or_else(|| existing.last_axis.clone());
                } else {
                    next.button_event_count = button_delta.event_count;
                    next.button_press_count = button_delta.press_count;
                    next.button_release_count = button_delta.release_count;
                    next.axis_event_count = if axis_delta.stick_changed { 1 } else { 0 };
                    next.trigger_event_count = if axis_delta.trigger_changed { 1 } else { 0 };
                    next.last_button = button_delta.last_button.clone();
                    next.last_axis = axis_delta.last_axis.clone();
                }
                let summary = gamepad_event_summary(state.gamepad_id, &button_delta, &axis_delta);
                insert_gamepad_state_payload(&next, state.sequence, &mut payload);
                upsert_gamepad_state(&mut self.local_controls, next);
                (LocalInputDeviceKind::Gamepad, "state".to_string(), summary)
            }
        };
        let source = self.local_input_source_for_event(device_kind, timestamp_ms, &mut payload);

        let mut event = LocalInputDiagnosticEvent {
            sequence,
            timestamp_ms,
            device_kind,
            event_kind,
            summary,
            device_id: metadata.map(|metadata| metadata.device_id.clone()),
            device_instance_id: metadata.and_then(|metadata| metadata.device_instance_id.clone()),
            capture_path: metadata.and_then(|metadata| metadata.capture_path.clone()),
            source,
            payload,
        };
        update_local_input_device_feedback(&mut self.local_controls, &mut event);
        push_recent_local_event(&mut self.local_controls, event.clone());
        event
    }
}

#[derive(Clone)]
struct DaemonUiProjectionSource {
    state: Arc<RwLock<DaemonState>>,
    network: Arc<dyn DaemonNetworkProjection>,
    diagnostics: Option<watch::Receiver<ControlMetricSnapshot>>,
}

trait DaemonNetworkProjection: Send + Sync {
    fn connection_infos(&self) -> Vec<ConnectionInfo>;
}

impl DaemonNetworkProjection for ConnectionSnapshotReader {
    fn connection_infos(&self) -> Vec<ConnectionInfo> {
        ConnectionSnapshotReader::connection_infos(self)
    }
}

impl UiProjectionSource for DaemonUiProjectionSource {
    fn project(&self) -> BoxFuture<'_, Result<UiSnapshot>> {
        let connection_infos = self.network.connection_infos();
        let diagnostics = self
            .diagnostics
            .as_ref()
            .map(|diagnostics| *diagnostics.borrow());
        Box::pin(async move {
            let state = self.state.read().await;
            let mut snapshot = state.ui_snapshot_for_connections(&connection_infos);
            if let Some(diagnostics) = diagnostics {
                overlay_sampled_control_metrics(&mut snapshot, diagnostics);
            }
            Ok(snapshot)
        })
    }
}

fn overlay_sampled_control_metrics(snapshot: &mut UiSnapshot, metrics: ControlMetricSnapshot) {
    let local = &mut snapshot.status.latency_feedback.local_input;
    local.event_count = metrics.captured;
    local.status = if metrics.reliable_overflow > 0 || metrics.realtime_dropped > 0 {
        LatencyFeedbackStatus::Degraded
    } else if metrics.captured > 0 {
        LatencyFeedbackStatus::Healthy
    } else {
        local.status
    };
    snapshot.dynamic_state.diagnostics = snapshot.status.latency_feedback.clone();
}

fn local_gamepad_event_count(snapshot: &LocalControlDeviceSnapshot) -> u64 {
    snapshot.gamepads.iter().fold(0_u64, |sum, gamepad| {
        sum.saturating_add(gamepad.event_count)
    })
}

fn is_eligible_local_input_feedback_event(event: &LocalInputDiagnosticEvent) -> bool {
    matches!(
        event.device_kind,
        LocalInputDeviceKind::Keyboard
            | LocalInputDeviceKind::Mouse
            | LocalInputDeviceKind::Gamepad
    ) && !event.payload.contains_key("remote_device_id")
        && !event.payload.contains_key("origin_event_device_id")
        && event.capture_path.as_deref() != Some("remote-daemon")
}

const LOCAL_CONTROL_RECENT_EVENT_LIMIT: usize = 64;
const INJECTION_LOOPBACK_WINDOW_MS: u64 = 750;

type UsbHostRuntime = Arc<Mutex<rshare_platform::ExperimentalUsbHostRuntime>>;

#[derive(Debug, Clone)]
struct CapturedInputEvent {
    event: InputEvent,
    metadata: Option<LocalInputDeviceMetadata>,
}

impl From<InputEvent> for CapturedInputEvent {
    fn from(event: InputEvent) -> Self {
        Self {
            event,
            metadata: None,
        }
    }
}

#[derive(Debug, Clone)]
struct LocalInputDeviceMetadata {
    device_id: String,
    device_instance_id: Option<String>,
    capture_path: Option<String>,
}

fn default_local_control_snapshot(
    width: u32,
    height: u32,
    features: &RuntimeFeatureConfig,
) -> LocalControlDeviceSnapshot {
    let mut snapshot = LocalControlDeviceSnapshot::default();
    snapshot.display = fallback_display_state(width, height);
    refresh_platform_local_controls(&mut snapshot, features);
    snapshot
}

fn refresh_platform_local_controls(
    snapshot: &mut LocalControlDeviceSnapshot,
    features: &RuntimeFeatureConfig,
) {
    #[cfg(windows)]
    {
        match rshare_platform::display::query_display_state() {
            Ok(display) if !display.displays.is_empty() => {
                snapshot.display = display;
            }
            Ok(_) => {
                let screens = rshare_platform::windows::get_all_screens();
                if !screens.is_empty() {
                    snapshot.display = display_state_from_windows_screens(&screens);
                }
            }
            Err(error) => {
                let screens = rshare_platform::windows::get_all_screens();
                if !screens.is_empty() {
                    snapshot.display = display_state_from_windows_screens(&screens);
                }
                snapshot.last_error = Some(format!("Display enumeration failed: {error}"));
            }
        }
        snapshot.driver = rshare_platform::windows::probe_rshare_driver();
        if windows_driver_filter_capture_ready(&snapshot.driver) {
            snapshot.keyboard.capture_source = "RShare filter driver + fallback hook".to_string();
            snapshot.mouse.capture_source = "RShare filter driver + fallback hook".to_string();
        }
    }
    #[cfg(target_os = "linux")]
    {
        match rshare_platform::display::query_display_state() {
            Ok(display) if !display.displays.is_empty() => {
                snapshot.display = display;
            }
            Ok(_) => {}
            Err(error) => {
                snapshot.last_error = Some(format!("Linux display enumeration failed: {error}"));
            }
        }
        match rshare_platform::enumerate_input_devices() {
            Ok((keyboards, mice)) => {
                snapshot.keyboard_devices = keyboards;
                snapshot.mouse_devices = mice;
                if !snapshot.keyboard_devices.is_empty() {
                    snapshot.keyboard.capture_source = "Linux evdev".to_string();
                }
                if !snapshot.mouse_devices.is_empty() {
                    snapshot.mouse.capture_source = "Linux evdev".to_string();
                }
            }
            Err(error) => {
                snapshot.last_error = Some(format!("Linux input enumeration failed: {error}"));
            }
        }
    }
    #[cfg(windows)]
    match rshare_platform::windows::enumerate_raw_input_devices() {
        Ok((keyboards, mice)) => {
            snapshot.keyboard_devices = keyboards;
            snapshot.mouse_devices = mice;
            if !snapshot.keyboard_devices.is_empty()
                && !windows_driver_filter_capture_ready(&snapshot.driver)
            {
                snapshot.keyboard.capture_source = "Windows Raw Input + low-level hook".to_string();
            }
            if !snapshot.mouse_devices.is_empty()
                && !windows_driver_filter_capture_ready(&snapshot.driver)
            {
                snapshot.mouse.capture_source = "Windows Raw Input + low-level hook".to_string();
            }
        }
        Err(error) => {
            snapshot.last_error = Some(format!("Raw Input enumeration failed: {error}"));
        }
    }
    #[cfg(windows)]
    match rshare_platform::windows::enumerate_audio_output_devices() {
        Ok(outputs) => {
            snapshot.audio_outputs = outputs;
        }
        Err(error) => {
            snapshot.audio_outputs.clear();
            snapshot.last_error = Some(format!("Core Audio enumeration failed: {error}"));
        }
    }
    #[cfg(windows)]
    match rshare_platform::windows::enumerate_audio_input_devices() {
        Ok(inputs) => {
            snapshot.audio_inputs = inputs;
        }
        Err(error) => {
            snapshot.audio_inputs.clear();
            snapshot.last_error = Some(format!("Core Audio input enumeration failed: {error}"));
        }
    }
    if features.usb_forwarding_experimental {
        #[cfg(windows)]
        match rshare_platform::ExperimentalUsbHostRuntime::new().enumerate_devices() {
            Ok(devices) => {
                snapshot.usb_devices = devices;
            }
            Err(error) => {
                snapshot.usb_devices.clear();
                snapshot.last_error = Some(format!("USB enumeration failed: {error}"));
            }
        }
    } else {
        snapshot.usb_devices.clear();
        snapshot.remote_usb_devices.clear();
    }
}

fn fallback_display_state(width: u32, height: u32) -> LocalDisplayState {
    LocalDisplayState {
        display_count: 1,
        virtual_x: 0,
        virtual_y: 0,
        primary_width: width,
        primary_height: height,
        layout_width: width,
        layout_height: height,
        displays: vec![LocalDisplayInfo {
            display_id: "primary".to_string(),
            x: 0,
            y: 0,
            width,
            height,
            primary: true,
            active: true,
            ..LocalDisplayInfo::default()
        }],
    }
}

#[cfg(windows)]
fn display_state_from_windows_screens(
    screens: &[rshare_platform::windows::ScreenInfo],
) -> LocalDisplayState {
    let min_x = screens.iter().map(|screen| screen.x).min().unwrap_or(0);
    let min_y = screens.iter().map(|screen| screen.y).min().unwrap_or(0);
    let max_x = screens
        .iter()
        .map(|screen| screen.x.saturating_add(screen.width as i32))
        .max()
        .unwrap_or(0);
    let max_y = screens
        .iter()
        .map(|screen| screen.y.saturating_add(screen.height as i32))
        .max()
        .unwrap_or(0);
    let primary = screens
        .iter()
        .find(|screen| screen.x == 0 && screen.y == 0)
        .unwrap_or(&screens[0]);

    let mut sorted_screens = screens.to_vec();
    sorted_screens.sort_by_key(|screen| (screen.x, screen.y));

    LocalDisplayState {
        display_count: screens.len(),
        virtual_x: min_x,
        virtual_y: min_y,
        primary_width: primary.width,
        primary_height: primary.height,
        layout_width: max_x.saturating_sub(min_x).max(0) as u32,
        layout_height: max_y.saturating_sub(min_y).max(0) as u32,
        displays: sorted_screens
            .iter()
            .enumerate()
            .map(|(index, screen)| LocalDisplayInfo {
                display_id: if screen.x == 0 && screen.y == 0 {
                    "primary".to_string()
                } else {
                    format!("display-{}", index + 1)
                },
                x: screen.x,
                y: screen.y,
                width: screen.width,
                height: screen.height,
                primary: screen.x == 0 && screen.y == 0,
                active: true,
                ..LocalDisplayInfo::default()
            })
            .collect(),
    }
}

fn display_nodes_from_local_display_state(display: &LocalDisplayState) -> Vec<DisplayNode> {
    let mut displays = if display.displays.is_empty() {
        vec![LocalDisplayInfo {
            display_id: "primary".to_string(),
            x: 0,
            y: 0,
            width: display.primary_width.max(1),
            height: display.primary_height.max(1),
            primary: true,
            active: true,
            ..LocalDisplayInfo::default()
        }]
    } else {
        display.displays.clone()
    };
    displays.sort_by_key(|display| (display.x, display.y));

    let has_primary = displays.iter().any(|display| display.primary);
    displays
        .into_iter()
        .enumerate()
        .map(|(index, display)| DisplayNode {
            display_id: if display.display_id.trim().is_empty() {
                if display.primary || (!has_primary && index == 0) {
                    "primary".to_string()
                } else {
                    format!("display-{}", index + 1)
                }
            } else {
                display.display_id
            },
            x: display.x,
            y: display.y,
            width: display.width.max(1),
            height: display.height.max(1),
            primary: display.primary || (!has_primary && index == 0),
            scale_percent: display.scale_percent,
            dpi_x: display.dpi_x,
            dpi_y: display.dpi_y,
        })
        .collect()
}

fn upsert_layout_node_displays(
    layout: &mut LayoutGraph,
    local_device_id: DeviceId,
    displays: Vec<DisplayNode>,
) -> bool {
    let displays = if displays.is_empty() {
        vec![DisplayNode::primary(0, 0, 1920, 1080)]
    } else {
        displays
    };

    if let Some(node) = layout
        .nodes
        .iter_mut()
        .find(|node| node.device_id == local_device_id)
    {
        if node.displays == displays {
            return false;
        }
        node.displays = displays;
        return true;
    }

    layout.add_node(LayoutNode {
        device_id: local_device_id,
        displays,
    });
    true
}

fn push_recent_local_event(
    snapshot: &mut LocalControlDeviceSnapshot,
    event: LocalInputDiagnosticEvent,
) {
    snapshot.recent_events.push(event);
    while snapshot.recent_events.len() > LOCAL_CONTROL_RECENT_EVENT_LIMIT {
        let remove_index = snapshot
            .recent_events
            .iter()
            .position(|event| {
                event.device_kind == LocalInputDeviceKind::Mouse && event.event_kind == "move"
            })
            .unwrap_or(0);
        snapshot.recent_events.remove(remove_index);
    }
}

fn update_mouse_display_position(snapshot: &mut LocalControlDeviceSnapshot) {
    let x = snapshot.mouse.x;
    let y = snapshot.mouse.y;
    let display = snapshot
        .display
        .displays
        .iter()
        .enumerate()
        .find(|(_, display)| {
            x >= display.x
                && x < display.x.saturating_add(display.width as i32)
                && y >= display.y
                && y < display.y.saturating_add(display.height as i32)
        });

    if let Some((index, display)) = display {
        snapshot.mouse.current_display_index = Some(index);
        snapshot.mouse.current_display_id = Some(display.display_id.clone());
        snapshot.mouse.display_relative_x = x.saturating_sub(display.x);
        snapshot.mouse.display_relative_y = y.saturating_sub(display.y);
    } else {
        snapshot.mouse.current_display_index = None;
        snapshot.mouse.current_display_id = None;
        snapshot.mouse.display_relative_x = x.saturating_sub(snapshot.display.virtual_x);
        snapshot.mouse.display_relative_y = y.saturating_sub(snapshot.display.virtual_y);
    }
}

fn insert_mouse_position_payload(
    snapshot: &LocalControlDeviceSnapshot,
    payload: &mut BTreeMap<String, String>,
) {
    payload.insert(
        "display_relative_x".to_string(),
        snapshot.mouse.display_relative_x.to_string(),
    );
    payload.insert(
        "display_relative_y".to_string(),
        snapshot.mouse.display_relative_y.to_string(),
    );
    if let Some(index) = snapshot.mouse.current_display_index {
        payload.insert("display_index".to_string(), index.to_string());
    }
    if let Some(display_id) = &snapshot.mouse.current_display_id {
        payload.insert("display_id".to_string(), display_id.clone());
    }
}

fn update_local_input_device_feedback(
    snapshot: &mut LocalControlDeviceSnapshot,
    event: &mut LocalInputDiagnosticEvent,
) {
    let devices = match event.device_kind {
        LocalInputDeviceKind::Keyboard => &mut snapshot.keyboard_devices,
        LocalInputDeviceKind::Mouse => &mut snapshot.mouse_devices,
        _ => return,
    };

    if devices.is_empty() {
        return;
    }

    let selected_index = event
        .device_id
        .as_deref()
        .or_else(|| event.payload.get("device_id").map(String::as_str))
        .and_then(|device_id| devices.iter().position(|device| device.id == device_id))
        .or_else(|| {
            event
                .device_instance_id
                .as_deref()
                .or_else(|| event.payload.get("device_instance_id").map(String::as_str))
                .and_then(|instance_id| {
                    devices.iter().position(|device| {
                        device.device_instance_id.as_deref() == Some(instance_id)
                    })
                })
        })
        .or_else(|| {
            event
                .capture_path
                .as_deref()
                .or_else(|| event.payload.get("capture_path").map(String::as_str))
                .and_then(|capture_path| {
                    devices
                        .iter()
                        .position(|device| device.capture_path.as_deref() == Some(capture_path))
                })
        })
        .or_else(|| if devices.len() == 1 { Some(0) } else { None });

    let Some(index) = selected_index else {
        return;
    };

    let device = &mut devices[index];
    device.connected = true;
    device.event_count = device.event_count.saturating_add(1);
    device.last_event_ms = event.timestamp_ms;

    if event.device_id.is_none() {
        event.device_id = Some(device.id.clone());
    }
    if event.device_instance_id.is_none() {
        event.device_instance_id = device.device_instance_id.clone();
    }
    if event.capture_path.is_none() {
        event.capture_path = device.capture_path.clone();
    }
    event
        .payload
        .entry("device_id".to_string())
        .or_insert_with(|| device.id.clone());
    if let Some(instance_id) = &device.device_instance_id {
        event
            .payload
            .entry("device_instance_id".to_string())
            .or_insert_with(|| instance_id.clone());
    }
    if let Some(capture_path) = &device.capture_path {
        event
            .payload
            .entry("capture_path".to_string())
            .or_insert_with(|| capture_path.clone());
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn remove_value(values: &mut Vec<String>, value: &str) {
    values.retain(|existing| existing != value);
}

#[derive(Debug, Default)]
struct GamepadButtonDelta {
    event_count: u64,
    press_count: u64,
    release_count: u64,
    last_button: Option<String>,
}

#[derive(Debug, Default)]
struct GamepadAxisDelta {
    stick_changed: bool,
    trigger_changed: bool,
    last_axis: Option<String>,
}

fn gamepad_button_delta(
    existing: Option<&LocalGamepadState>,
    state: &rshare_core::GamepadState,
) -> GamepadButtonDelta {
    let previous = existing
        .map(|gamepad| {
            gamepad
                .buttons
                .iter()
                .map(|button| (format!("{:?}", button.button), button.pressed))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut next = state
        .buttons
        .iter()
        .map(|button| (format!("{:?}", button.button), button.pressed))
        .collect::<BTreeMap<_, _>>();

    for button in previous.keys() {
        next.entry(button.clone()).or_insert(false);
    }

    let mut delta = GamepadButtonDelta::default();
    for (button, pressed) in next {
        let was_pressed = previous.get(&button).copied().unwrap_or(false);
        if was_pressed == pressed {
            continue;
        }
        delta.event_count = delta.event_count.saturating_add(1);
        if pressed {
            delta.press_count = delta.press_count.saturating_add(1);
            delta.last_button = Some(format!("{button} pressed"));
        } else {
            delta.release_count = delta.release_count.saturating_add(1);
            delta.last_button = Some(format!("{button} released"));
        }
    }
    delta
}

fn gamepad_axis_delta(
    existing: Option<&LocalGamepadState>,
    state: &rshare_core::GamepadState,
) -> GamepadAxisDelta {
    let Some(existing) = existing else {
        return GamepadAxisDelta {
            stick_changed: state.left_stick_x != 0
                || state.left_stick_y != 0
                || state.right_stick_x != 0
                || state.right_stick_y != 0,
            trigger_changed: state.left_trigger != 0 || state.right_trigger != 0,
            last_axis: None,
        };
    };

    let left_stick_changed =
        existing.left_stick_x != state.left_stick_x || existing.left_stick_y != state.left_stick_y;
    let right_stick_changed = existing.right_stick_x != state.right_stick_x
        || existing.right_stick_y != state.right_stick_y;
    let trigger_changed = existing.left_trigger != state.left_trigger
        || existing.right_trigger != state.right_trigger;
    let last_axis = if trigger_changed {
        Some("trigger".to_string())
    } else if right_stick_changed {
        Some("right_stick".to_string())
    } else if left_stick_changed {
        Some("left_stick".to_string())
    } else {
        existing.last_axis.clone()
    };

    GamepadAxisDelta {
        stick_changed: left_stick_changed || right_stick_changed,
        trigger_changed,
        last_axis,
    }
}

fn gamepad_event_summary(
    gamepad_id: u8,
    button_delta: &GamepadButtonDelta,
    axis_delta: &GamepadAxisDelta,
) -> String {
    if let Some(button) = &button_delta.last_button {
        return format!("Gamepad {gamepad_id} {button}");
    }
    if let Some(axis) = &axis_delta.last_axis {
        return format!("Gamepad {gamepad_id} {axis}");
    }
    format!("Gamepad {gamepad_id} state")
}

fn insert_gamepad_state_payload(
    state: &LocalGamepadState,
    sequence: u64,
    payload: &mut BTreeMap<String, String>,
) {
    payload.insert("gamepad_id".to_string(), state.gamepad_id.to_string());
    payload.insert("sequence".to_string(), sequence.to_string());
    payload.insert("name".to_string(), state.name.clone());
    payload.insert("connected".to_string(), state.connected.to_string());
    payload.insert(
        "pressed_buttons".to_string(),
        state.pressed_buttons.join(","),
    );
    if let Some(last_button) = &state.last_button {
        payload.insert("last_button".to_string(), last_button.clone());
    }
    if let Some(last_axis) = &state.last_axis {
        payload.insert("last_axis".to_string(), last_axis.clone());
    }
    payload.insert("left_stick_x".to_string(), state.left_stick_x.to_string());
    payload.insert("left_stick_y".to_string(), state.left_stick_y.to_string());
    payload.insert("right_stick_x".to_string(), state.right_stick_x.to_string());
    payload.insert("right_stick_y".to_string(), state.right_stick_y.to_string());
    payload.insert("left_trigger".to_string(), state.left_trigger.to_string());
    payload.insert("right_trigger".to_string(), state.right_trigger.to_string());
    payload.insert("event_count".to_string(), state.event_count.to_string());
    payload.insert(
        "button_event_count".to_string(),
        state.button_event_count.to_string(),
    );
    payload.insert(
        "button_press_count".to_string(),
        state.button_press_count.to_string(),
    );
    payload.insert(
        "button_release_count".to_string(),
        state.button_release_count.to_string(),
    );
    payload.insert(
        "axis_event_count".to_string(),
        state.axis_event_count.to_string(),
    );
    payload.insert(
        "trigger_event_count".to_string(),
        state.trigger_event_count.to_string(),
    );
}

fn upsert_gamepad_metadata(
    snapshot: &mut LocalControlDeviceSnapshot,
    gamepad_id: u8,
    name: &str,
    connected: bool,
) {
    if let Some(gamepad) = snapshot
        .gamepads
        .iter_mut()
        .find(|gamepad| gamepad.gamepad_id == gamepad_id)
    {
        gamepad.name = name.to_string();
        gamepad.connected = connected;
        gamepad.event_count = gamepad.event_count.saturating_add(1);
        gamepad.last_seen_ms = timestamp_ms_now();
        return;
    }

    snapshot.gamepads.push(LocalGamepadState {
        gamepad_id,
        name: name.to_string(),
        connected,
        buttons: Vec::new(),
        pressed_buttons: Vec::new(),
        last_button: None,
        left_stick_x: 0,
        left_stick_y: 0,
        right_stick_x: 0,
        right_stick_y: 0,
        left_trigger: 0,
        right_trigger: 0,
        event_count: 1,
        button_event_count: 0,
        button_press_count: 0,
        button_release_count: 0,
        axis_event_count: 0,
        trigger_event_count: 0,
        last_axis: None,
        last_seen_ms: timestamp_ms_now(),
    });
}

fn upsert_gamepad_state(snapshot: &mut LocalControlDeviceSnapshot, state: LocalGamepadState) {
    if let Some(existing) = snapshot
        .gamepads
        .iter_mut()
        .find(|gamepad| gamepad.gamepad_id == state.gamepad_id)
    {
        *existing = state;
    } else {
        snapshot.gamepads.push(state);
    }
}

#[cfg(windows)]
fn replace_recent_local_event(
    snapshot: &mut LocalControlDeviceSnapshot,
    event: LocalInputDiagnosticEvent,
) {
    if let Some(last) = snapshot
        .recent_events
        .iter_mut()
        .rev()
        .find(|candidate| candidate.sequence == event.sequence)
    {
        *last = event;
    }
}

#[cfg(windows)]
fn update_driver_device_from_event(
    snapshot: &mut LocalControlDeviceSnapshot,
    event: &rshare_platform::windows::WindowsDriverInputEvent,
    timestamp_ms: u64,
) {
    let (devices, fallback_name, capability) = match event.device_kind {
        rshare_platform::windows::WindowsDriverDeviceKind::Keyboard => (
            &mut snapshot.keyboard_devices,
            "Driver keyboard",
            "driver-capture",
        ),
        rshare_platform::windows::WindowsDriverDeviceKind::Mouse => (
            &mut snapshot.mouse_devices,
            "Driver mouse",
            "driver-capture",
        ),
        rshare_platform::windows::WindowsDriverDeviceKind::Gamepad => return,
    };

    if let Some(device) = devices.iter_mut().find(|device| {
        device.id == event.device_id
            || device.device_instance_id.as_deref() == Some(&event.device_instance_id)
    }) {
        device.connected = true;
        device.event_count = device.event_count.saturating_add(1);
        device.last_event_ms = timestamp_ms;
        if !device.capabilities.iter().any(|value| value == capability) {
            device.capabilities.push(capability.to_string());
        }
        return;
    }

    devices.push(rshare_core::LocalHardwareDevice {
        id: event.device_id.clone(),
        name: fallback_name.to_string(),
        source: "RShare KMDF filter".to_string(),
        connected: true,
        driver_detail: Some(event.device_instance_id.clone()),
        device_instance_id: Some(event.device_instance_id.clone()),
        capture_path: Some("rshare-filter".to_string()),
        event_count: 1,
        last_event_ms: timestamp_ms,
        capabilities: vec![capability.to_string()],
    });
}

fn backend_kind_from_resolved_mode(mode: ResolvedInputMode) -> BackendKind {
    match mode {
        ResolvedInputMode::Portable => BackendKind::Portable,
        #[cfg(windows)]
        ResolvedInputMode::WindowsNative => BackendKind::WindowsNative,
        #[cfg(windows)]
        ResolvedInputMode::VirtualHid => BackendKind::VirtualHid,
        #[cfg(target_os = "linux")]
        ResolvedInputMode::Evdev => BackendKind::Evdev,
        #[cfg(target_os = "linux")]
        ResolvedInputMode::UInput => BackendKind::UInput,
    }
}

#[cfg(windows)]
fn windows_driver_filter_capture_ready(driver: &rshare_core::LocalDriverDiagnosticState) -> bool {
    driver.status == "available"
        && driver.filter_active
        && driver.filter_keyboard_connects > 0
        && driver.filter_mouse_connects > 0
}

#[cfg(windows)]
fn windows_should_use_filter_capture(
    mode: Option<ResolvedInputMode>,
    driver: &rshare_core::LocalDriverDiagnosticState,
) -> bool {
    matches!(mode, Some(ResolvedInputMode::VirtualHid))
        && windows_driver_filter_capture_ready(driver)
}

fn timestamp_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

const LATENCY_HEALTHY_RTT_MS: u64 = 50;
const LATENCY_PROBE_TIMEOUT_MS: u64 = 1_500;

fn network_snapshot_from_connections(connections: &[ConnectionInfo]) -> NetworkTransportSnapshot {
    let mut snapshot = NetworkTransportSnapshot {
        datagram_available: connections
            .iter()
            .any(|connection| connection.datagram_available),
        realtime_degraded: connections.is_empty()
            || connections
                .iter()
                .any(|connection| !connection.datagram_available),
        rtt_ms: connections
            .iter()
            .filter_map(|connection| connection.rtt_ms)
            .min(),
        last_datagram_rx_ms: connections
            .iter()
            .filter_map(|connection| connection.last_datagram_rx_ms)
            .min(),
        datagram_tx_dropped: connections
            .iter()
            .map(|connection| connection.datagram_tx_dropped)
            .sum(),
        reliable_stream_reset_count: connections
            .iter()
            .map(|connection| connection.reliable_stream_reset_count)
            .sum(),
        cert_trust_state: None,
        ..NetworkTransportSnapshot::default()
    };

    if let Some(state) = connections
        .iter()
        .find_map(|connection| connection.cert_trust_state.clone())
    {
        snapshot.cert_trust_state = Some(state);
    }

    snapshot
}

fn connected_connection_count(connections: &[ConnectionInfo]) -> usize {
    connections
        .iter()
        .filter(|connection| connection.state == ConnectionState::Connected)
        .count()
}

fn transport_feedback_from_connections(
    network: &NetworkTransportSnapshot,
    connections: &[ConnectionInfo],
) -> TransportFeedback {
    let connected_count = connected_connection_count(connections);
    let status = if connected_count == 0 {
        LatencyFeedbackStatus::Unavailable
    } else if connections
        .iter()
        .filter(|connection| connection.state == ConnectionState::Connected)
        .any(|connection| {
            !connection.datagram_available
                || connection.datagram_tx_dropped > 0
                || connection.reliable_stream_reset_count > 0
                || connection.rtt_ms.is_none()
                || connection
                    .rtt_ms
                    .is_some_and(|rtt| rtt > LATENCY_HEALTHY_RTT_MS)
        })
    {
        LatencyFeedbackStatus::Degraded
    } else {
        LatencyFeedbackStatus::Healthy
    };

    TransportFeedback {
        status,
        transport: network.transport.clone(),
        datagram_available: network.datagram_available,
        realtime_degraded: network.realtime_degraded,
        rtt_ms: network.rtt_ms,
        last_datagram_rx_ms: network.last_datagram_rx_ms,
        datagram_tx_dropped: network.datagram_tx_dropped,
        reliable_stream_reset_count: network.reliable_stream_reset_count,
        cert_trust_state: network.cert_trust_state.clone(),
    }
}

fn is_latency_ack_event(event: &LocalInputDiagnosticEvent) -> bool {
    matches!(
        event.event_kind.as_str(),
        "latency_probe_ack" | "latency_endpoint_switch_ack"
    )
}

fn parse_latency_payload_u64(payload: &BTreeMap<String, String>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .filter_map(|key| payload.get(*key))
        .find_map(|value| value.parse::<u64>().ok())
}

fn latency_event_matches_target(event: &LocalInputDiagnosticEvent, target: DeviceId) -> bool {
    ["target_device_id", "origin_device_id"]
        .iter()
        .filter_map(|key| event.payload.get(*key))
        .filter_map(|value| DeviceId::parse_str(value).ok())
        .chain(
            event
                .device_id
                .as_deref()
                .and_then(|value| DeviceId::parse_str(value).ok()),
        )
        .any(|candidate| candidate == target)
}

fn latency_ack_probe_sequence(event: &LocalInputDiagnosticEvent) -> Option<u64> {
    if event.event_kind == "latency_endpoint_switch_ack" {
        return parse_latency_payload_u64(&event.payload, &["origin_probe_sequence"])
            .or_else(|| parse_latency_payload_u64(&event.payload, &["probe_sequence"]));
    }

    parse_latency_payload_u64(&event.payload, &["probe_sequence"])
}

fn latency_ack_completion_sequence_for_pending(
    event: &LocalInputDiagnosticEvent,
    pending: &PendingLatencyProbe,
) -> Option<u64> {
    if event.event_kind != "latency_endpoint_switch_ack" {
        return parse_latency_payload_u64(&event.payload, &["probe_sequence"]);
    }

    match pending.role {
        PendingLatencyProbeRole::LocalRequested => {
            parse_latency_payload_u64(&event.payload, &["origin_probe_sequence"])
                .or_else(|| parse_latency_payload_u64(&event.payload, &["probe_sequence"]))
        }
        PendingLatencyProbeRole::EndpointSwitchReport { .. } => {
            parse_latency_payload_u64(&event.payload, &["probe_sequence"])
        }
    }
}

fn latency_ack_order_key(event: &LocalInputDiagnosticEvent) -> (u64, u64) {
    (
        event.sequence,
        latency_ack_probe_sequence(event).unwrap_or(event.sequence),
    )
}

fn latency_feedback_status_priority(status: LatencyFeedbackStatus) -> u8 {
    match status {
        LatencyFeedbackStatus::Timeout => 5,
        LatencyFeedbackStatus::Degraded => 4,
        LatencyFeedbackStatus::Pending => 3,
        LatencyFeedbackStatus::Healthy => 2,
        LatencyFeedbackStatus::Idle => 1,
        LatencyFeedbackStatus::Unavailable => 0,
    }
}

fn publication_item_to_local_diagnostic(
    local_device_id: DeviceId,
    sequence: u64,
    item: &DiagnosticPublicationItem,
) -> LocalInputDiagnosticEvent {
    match &item.payload {
        DiagnosticPayload::Discrete(event) => event.clone(),
        DiagnosticPayload::Metrics(snapshot) => {
            let mut payload = BTreeMap::new();
            payload.insert("snapshot_json".to_string(), item.json.to_string());
            payload.insert("captured".to_string(), snapshot.captured.to_string());
            payload.insert("routed".to_string(), snapshot.routed.to_string());
            payload.insert(
                "realtime_replaced".to_string(),
                snapshot.realtime_replaced.to_string(),
            );
            payload.insert(
                "realtime_dropped".to_string(),
                snapshot.realtime_dropped.to_string(),
            );
            payload.insert(
                "reliable_overflow".to_string(),
                snapshot.reliable_overflow.to_string(),
            );
            LocalInputDiagnosticEvent {
                sequence,
                timestamp_ms: timestamp_ms_now(),
                device_kind: LocalInputDeviceKind::Backend,
                event_kind: "control_metrics_sample".to_string(),
                summary: "Sampled control-path metrics".to_string(),
                device_id: Some(local_device_id.to_string()),
                device_instance_id: None,
                capture_path: Some("diagnostics-runtime".to_string()),
                source: LocalInputEventSource::System,
                payload,
            }
        }
    }
}

async fn run_peer_diagnostics_forwarder(
    mut subscription: DiagnosticsSubscription,
    local_device_id: DeviceId,
    peer_id: DeviceId,
    mut try_send: impl FnMut(TelemetryFrame) -> std::result::Result<(), TransportSendError>,
) {
    while let Some(publication) = subscription.recv().await {
        for item in publication.items.iter() {
            let event =
                publication_item_to_local_diagnostic(local_device_id, publication.sequence, item);
            let message = Message::InputDiagnostic {
                device_id: local_device_id,
                event,
            };
            let frame = match ClassifiedMessage::try_from(message) {
                Ok(ClassifiedMessage::Telemetry(frame)) => frame,
                Ok(_) => unreachable!("input diagnostics must use the telemetry lane"),
                Err(error) => {
                    tracing::debug!(
                        "Failed to classify sampled diagnostics for {}: {}",
                        peer_id,
                        error
                    );
                    return;
                }
            };
            match try_send(frame) {
                Ok(()) => {}
                Err(TransportSendError::TelemetryLaneFull) => {
                    tracing::trace!(
                        "Dropped a stale sampled diagnostics item for {} because telemetry is full",
                        peer_id
                    );
                }
                Err(error) => {
                    tracing::debug!(
                        "Failed to send sampled diagnostics to {}: {}",
                        peer_id,
                        error
                    );
                    return;
                }
            }
        }
    }
}

fn spawn_peer_diagnostics_forwarder(
    diagnostics: &DiagnosticsHandle,
    input_registry: Arc<ConnectionRegistry>,
    local_device_id: DeviceId,
    subscriber_id: DiagnosticSubscriberId,
) {
    let Some(peer) = input_registry.peer(&subscriber_id.peer_id).filter(|peer| {
        diagnostic_generation_is_current(subscriber_id, Some(peer.auth.control_connection_id))
    }) else {
        return;
    };
    let Some(subscription) = diagnostics.subscribe_current(subscriber_id) else {
        return;
    };
    tokio::spawn(run_peer_diagnostics_forwarder(
        subscription,
        local_device_id,
        subscriber_id.peer_id,
        move |frame| peer.transport.try_send_telemetry(frame),
    ));
}

fn diagnostic_generation_is_current(
    subscriber_id: DiagnosticSubscriberId,
    current: Option<ControlConnectionId>,
) -> bool {
    current == Some(subscriber_id.control_connection_id)
}

async fn request_remote_endpoint_events(
    network_manager: &Arc<Mutex<NetworkManager>>,
    state: &Arc<RwLock<DaemonState>>,
    filter: &EndpointEventFilter,
) {
    let Some(endpoint_id) = filter.endpoint_id else {
        return;
    };
    let local_device_id = {
        let state = state.read().await;
        state.status.device_id
    };
    if endpoint_id == local_device_id {
        return;
    }
    let connected = {
        let state = state.read().await;
        is_device_connected(&state, endpoint_id)
    };
    if !connected {
        return;
    }
    let result = {
        let mut manager = network_manager.lock().await;
        manager
            .send_to(
                &endpoint_id,
                Message::EndpointEventSubscribe {
                    filter: filter.clone(),
                },
            )
            .await
    };
    if let Err(error) = result {
        tracing::debug!(
            "Failed to request endpoint events from {}: {}",
            endpoint_id,
            error
        );
    }
}

fn normalize_remote_diagnostic_event(
    from: DeviceId,
    mut event: LocalInputDiagnosticEvent,
) -> LocalInputDiagnosticEvent {
    if let Some(original_device_id) = event.device_id.replace(from.to_string()) {
        if original_device_id != from.to_string() {
            event
                .payload
                .entry("origin_event_device_id".to_string())
                .or_insert(original_device_id);
        }
    }
    event
        .payload
        .entry("remote_device_id".to_string())
        .or_insert_with(|| from.to_string());
    event
        .capture_path
        .get_or_insert_with(|| "remote-daemon".to_string());
    event
}

fn record_remote_diagnostic_event(
    state: &mut DaemonState,
    from: DeviceId,
    event: LocalInputDiagnosticEvent,
) -> LocalInputDiagnosticEvent {
    let mut event = normalize_remote_diagnostic_event(from, event);
    let sequence = state.local_controls.sequence.saturating_add(1);
    state.local_controls.sequence = sequence;
    event
        .payload
        .insert("remote_sequence".to_string(), event.sequence.to_string());
    event.sequence = sequence;
    push_recent_local_event(&mut state.local_controls, event.clone());
    event
}

fn record_latency_diagnostic_event(
    state: &mut DaemonState,
    target: DeviceId,
    event_kind: impl Into<String>,
    summary: impl Into<String>,
    mut payload: BTreeMap<String, String>,
) -> LocalInputDiagnosticEvent {
    let sequence = state.local_controls.sequence.saturating_add(1);
    state.local_controls.sequence = sequence;
    payload
        .entry("target_device_id".to_string())
        .or_insert_with(|| target.to_string());

    let event = LocalInputDiagnosticEvent {
        sequence,
        timestamp_ms: timestamp_ms_now(),
        device_kind: LocalInputDeviceKind::Backend,
        event_kind: event_kind.into(),
        summary: summary.into(),
        device_id: Some(target.to_string()),
        device_instance_id: None,
        capture_path: Some("rshare-net".to_string()),
        source: LocalInputEventSource::System,
        payload,
    };
    push_recent_local_event(&mut state.local_controls, event.clone());
    event
}

fn add_latency_measurement_payload(
    payload: &mut BTreeMap<String, String>,
    local_sent_at_ms: u64,
    sent_timestamp_ms: u64,
    remote_received_timestamp_ms: u64,
    remote_ack_timestamp_ms: u64,
    local_received_timestamp_ms: u64,
) -> (u64, u64) {
    let ack_timestamp_ms = if remote_ack_timestamp_ms == 0 {
        remote_received_timestamp_ms
    } else {
        remote_ack_timestamp_ms
    };
    let raw_round_trip_ms = local_received_timestamp_ms.saturating_sub(local_sent_at_ms);
    let remote_processing_ms = ack_timestamp_ms.saturating_sub(remote_received_timestamp_ms);
    let network_round_trip_ms = raw_round_trip_ms.saturating_sub(remote_processing_ms);
    let estimated_one_way_ms = network_round_trip_ms / 2;

    payload.insert("latency_ms".to_string(), network_round_trip_ms.to_string());
    payload.insert(
        "raw_round_trip_ms".to_string(),
        raw_round_trip_ms.to_string(),
    );
    payload.insert(
        "network_round_trip_ms".to_string(),
        network_round_trip_ms.to_string(),
    );
    payload.insert(
        "estimated_one_way_ms".to_string(),
        estimated_one_way_ms.to_string(),
    );
    payload.insert(
        "remote_processing_ms".to_string(),
        remote_processing_ms.to_string(),
    );
    payload.insert(
        "sent_timestamp_ms".to_string(),
        sent_timestamp_ms.to_string(),
    );
    payload.insert(
        "local_sent_timestamp_ms".to_string(),
        local_sent_at_ms.to_string(),
    );
    payload.insert(
        "remote_received_timestamp_ms".to_string(),
        remote_received_timestamp_ms.to_string(),
    );
    payload.insert(
        "remote_ack_timestamp_ms".to_string(),
        ack_timestamp_ms.to_string(),
    );
    payload.insert(
        "local_received_timestamp_ms".to_string(),
        local_received_timestamp_ms.to_string(),
    );

    let t0 = sent_timestamp_ms as i128;
    let t1 = remote_received_timestamp_ms as i128;
    let t2 = ack_timestamp_ms as i128;
    let t3 = local_received_timestamp_ms as i128;
    let clock_offset_estimate_ms = ((t1 - t0) + (t2 - t3)) / 2;
    let local_to_remote_estimate_ms = (t1 - t0 - clock_offset_estimate_ms).max(0) as u64;
    let remote_to_local_estimate_ms = (t3 - t2 + clock_offset_estimate_ms).max(0) as u64;
    payload.insert(
        "clock_offset_estimate_ms".to_string(),
        clock_offset_estimate_ms.to_string(),
    );
    payload.insert(
        "local_to_remote_estimate_ms".to_string(),
        local_to_remote_estimate_ms.to_string(),
    );
    payload.insert(
        "remote_to_local_estimate_ms".to_string(),
        remote_to_local_estimate_ms.to_string(),
    );

    (network_round_trip_ms, raw_round_trip_ms)
}

fn short_device_id(id: DeviceId) -> String {
    id.to_string().chars().take(8).collect()
}

fn record_audio_diagnostic_event(
    state: &mut DaemonState,
    event_kind: impl Into<String>,
    summary: impl Into<String>,
    mut payload: BTreeMap<String, String>,
) -> LocalInputDiagnosticEvent {
    let sequence = state.local_controls.sequence.saturating_add(1);
    state.local_controls.sequence = sequence;
    payload
        .entry("sample_rate".to_string())
        .or_insert_with(|| "48000".to_string());
    payload
        .entry("channels".to_string())
        .or_insert_with(|| "2".to_string());

    let event = LocalInputDiagnosticEvent {
        sequence,
        timestamp_ms: timestamp_ms_now(),
        device_kind: LocalInputDeviceKind::Audio,
        event_kind: event_kind.into(),
        summary: summary.into(),
        device_id: None,
        device_instance_id: None,
        capture_path: Some("core-audio".to_string()),
        source: LocalInputEventSource::System,
        payload,
    };
    push_recent_local_event(&mut state.local_controls, event.clone());
    event
}

fn record_usb_diagnostic_event(
    state: &mut DaemonState,
    device_id: Option<DeviceId>,
    event_kind: impl Into<String>,
    summary: impl Into<String>,
    mut payload: BTreeMap<String, String>,
) -> LocalInputDiagnosticEvent {
    let sequence = state.local_controls.sequence.saturating_add(1);
    state.local_controls.sequence = sequence;
    if let Some(device_id) = device_id {
        payload
            .entry("remote_device_id".to_string())
            .or_insert_with(|| device_id.to_string());
    }

    let event = LocalInputDiagnosticEvent {
        sequence,
        timestamp_ms: timestamp_ms_now(),
        device_kind: LocalInputDeviceKind::Usb,
        event_kind: event_kind.into(),
        summary: summary.into(),
        device_id: device_id.map(|value| value.to_string()),
        device_instance_id: None,
        capture_path: Some("usb-forwarding".to_string()),
        source: LocalInputEventSource::System,
        payload,
    };
    push_recent_local_event(&mut state.local_controls, event.clone());
    event
}

fn display_capture_response_from_result(
    _request: &rshare_core::DisplayCaptureRequest,
    result: Result<DisplayCaptureResult>,
) -> DaemonResponse {
    DaemonResponse::DisplayCapture(result.unwrap_or_else(|error| DisplayCaptureResult {
        request_id: DeviceId::new_v4(),
        status: DisplayOperationStatus::ApplyFailed,
        message: Some(error.to_string()),
        payload: None,
        blob: None,
    }))
}

fn prepare_display_capture_response(
    mut result: DisplayCaptureResult,
) -> (DisplayCaptureResult, Option<IpcFrame>) {
    let blob = result.blob.take();
    match (&result.status, result.payload.as_ref(), blob) {
        (DisplayOperationStatus::Success, Some(descriptor), Some(blob))
            if descriptor == &blob.descriptor =>
        {
            match rshare_core::encode_display_capture_binary(descriptor, blob.bytes) {
                Ok(payload) => (
                    result,
                    Some(IpcFrame {
                        kind: IpcEnvelopeKind::Binary,
                        payload,
                    }),
                ),
                Err(error) => (
                    display_capture_protocol_error(result.request_id, error.to_string()),
                    None,
                ),
            }
        }
        (DisplayOperationStatus::Success, _, _) => (
            display_capture_protocol_error(
                result.request_id,
                "display capture metadata/blob mismatch",
            ),
            None,
        ),
        (_, None, None) => {
            if result
                .message
                .as_deref()
                .is_none_or(|message| message.trim().is_empty())
            {
                result.message = Some("display capture failed".to_string());
            }
            (result, None)
        }
        _ => (
            display_capture_protocol_error(
                result.request_id,
                "invalid failed display capture payload",
            ),
            None,
        ),
    }
}

fn display_capture_protocol_error(
    request_id: DeviceId,
    message: impl Into<String>,
) -> DisplayCaptureResult {
    DisplayCaptureResult {
        request_id,
        status: DisplayOperationStatus::ApplyFailed,
        message: Some(message.into()),
        payload: None,
        blob: None,
    }
}

fn display_identify_response_from_result(result: Result<DisplayIdentifyResult>) -> DaemonResponse {
    DaemonResponse::DisplayIdentify(result.unwrap_or_else(|error| DisplayIdentifyResult {
        status: DisplayOperationStatus::ApplyFailed,
        message: Some(error.to_string()),
    }))
}

fn display_settings_update_response_from_result(
    state: &mut DaemonState,
    request: &rshare_core::DisplaySettingsUpdateRequest,
    result: Result<DisplaySettingsUpdateResult>,
    refreshed_display: Result<LocalDisplayState>,
) -> (DaemonResponse, LocalInputDiagnosticEvent) {
    let result = result.unwrap_or_else(|error| DisplaySettingsUpdateResult {
        status: DisplayOperationStatus::ApplyFailed,
        message: Some(error.to_string()),
    });

    apply_refreshed_display_state(state, refreshed_display);
    let layout_changed = state.reconcile_local_layout_geometry();
    let event = record_display_settings_update_event(state, request, &result, layout_changed);

    (DaemonResponse::DisplaySettingsUpdated(result), event)
}

fn apply_refreshed_display_state(
    state: &mut DaemonState,
    refreshed_display: Result<LocalDisplayState>,
) {
    match refreshed_display {
        Ok(display) if !display.displays.is_empty() => {
            state.local_controls.display = display;
            update_mouse_display_position(&mut state.local_controls);
        }
        Ok(_) => {}
        Err(error) => {
            state.local_controls.last_error = Some(format!("Display enumeration failed: {error}"));
        }
    }
}

fn record_display_settings_update_event(
    state: &mut DaemonState,
    request: &rshare_core::DisplaySettingsUpdateRequest,
    result: &DisplaySettingsUpdateResult,
    layout_changed: bool,
) -> LocalInputDiagnosticEvent {
    let sequence = state.local_controls.sequence.saturating_add(1);
    state.local_controls.sequence = sequence;

    let mut payload = BTreeMap::new();
    payload.insert("display_id".to_string(), request.display_id.clone());
    payload.insert("status".to_string(), format!("{:?}", result.status));
    payload.insert("layout_changed".to_string(), layout_changed.to_string());
    if let Some(message) = &result.message {
        payload.insert("message".to_string(), message.clone());
    }
    if let Some(width) = request.width {
        payload.insert("width".to_string(), width.to_string());
    }
    if let Some(height) = request.height {
        payload.insert("height".to_string(), height.to_string());
    }
    if let Some(refresh_rate_millihz) = request.refresh_rate_millihz {
        payload.insert(
            "refresh_rate_millihz".to_string(),
            refresh_rate_millihz.to_string(),
        );
    }
    if let Some(orientation) = request.orientation {
        payload.insert("orientation".to_string(), format!("{:?}", orientation));
    }
    if let Some(primary) = request.primary {
        payload.insert("primary".to_string(), primary.to_string());
    }
    if let Some(x) = request.x {
        payload.insert("x".to_string(), x.to_string());
    }
    if let Some(y) = request.y {
        payload.insert("y".to_string(), y.to_string());
    }
    if let Some(scale_percent) = request.scale_percent {
        payload.insert("scale_percent".to_string(), scale_percent.to_string());
    }

    let event = LocalInputDiagnosticEvent {
        sequence,
        timestamp_ms: timestamp_ms_now(),
        device_kind: LocalInputDeviceKind::Display,
        event_kind: "settings_update".to_string(),
        summary: format!(
            "Display settings update for {}: {:?}",
            request.display_id, result.status
        ),
        device_id: None,
        device_instance_id: Some(request.display_id.clone()),
        capture_path: Some("display-settings".to_string()),
        source: LocalInputEventSource::System,
        payload,
    };
    push_recent_local_event(&mut state.local_controls, event.clone());
    event
}

fn upsert_remote_usb_device(
    state: &mut DaemonState,
    from: DeviceId,
    device: UsbDeviceDescriptor,
) -> LocalInputDiagnosticEvent {
    let device_name = state.devices.get(&from).map(|device| device.name.clone());
    let connected = state
        .devices
        .get(&from)
        .map(|device| device.connected)
        .unwrap_or(true);
    if let Some(existing) = state
        .local_controls
        .remote_usb_devices
        .iter_mut()
        .find(|entry| entry.device_id == from && entry.device.bus_id == device.bus_id)
    {
        existing.device_name = device_name.clone();
        existing.connected = connected;
        existing.device = device.clone();
    } else {
        state
            .local_controls
            .remote_usb_devices
            .push(RemoteUsbDeviceSnapshot {
                device_id: from,
                device_name: device_name.clone(),
                connected,
                device: device.clone(),
            });
    }
    state
        .local_controls
        .remote_usb_devices
        .sort_by(|left, right| {
            left.device_id
                .cmp(&right.device_id)
                .then(left.device.bus_id.cmp(&right.device.bus_id))
        });

    let mut payload = BTreeMap::new();
    payload.insert("bus_id".to_string(), device.bus_id.clone());
    payload.insert("vendor_id".to_string(), format!("{:04x}", device.vendor_id));
    payload.insert(
        "product_id".to_string(),
        format!("{:04x}", device.product_id),
    );
    record_usb_diagnostic_event(
        state,
        Some(from),
        "remote_usb_attached",
        format!(
            "Remote USB device advertised by {}: {:04x}:{:04x}",
            short_device_id(from),
            device.vendor_id,
            device.product_id
        ),
        payload,
    )
}

fn remove_remote_usb_device(
    state: &mut DaemonState,
    from: DeviceId,
    bus_id: &str,
    reason: &str,
) -> LocalInputDiagnosticEvent {
    state
        .local_controls
        .remote_usb_devices
        .retain(|device| !(device.device_id == from && device.device.bus_id == bus_id));
    let mut payload = BTreeMap::new();
    payload.insert("bus_id".to_string(), bus_id.to_string());
    payload.insert("reason".to_string(), reason.to_string());
    record_usb_diagnostic_event(
        state,
        Some(from),
        "remote_usb_detached",
        format!(
            "Remote USB device detached from {}: {reason}",
            short_device_id(from)
        ),
        payload,
    )
}

fn fail_pending_usb_for_device(state: &mut DaemonState, target: DeviceId, reason: &str) {
    let claim_keys: Vec<u64> = state
        .pending_usb_claims
        .iter()
        .filter(|(_, pending)| pending.target == target)
        .map(|(key, _)| *key)
        .collect();
    for key in claim_keys {
        if let Some(pending) = state.pending_usb_claims.remove(&key) {
            let result = usb_probe_result(
                UsbDescriptorProbeStatus::DeviceUnavailable,
                reason.to_string(),
                pending.target,
                pending.bus_id,
                pending.request_id,
                pending.transfer_id,
                None,
                pending.started_at_ms,
                Vec::new(),
                None,
            );
            let _ = pending.result_tx.send(result);
        }
    }

    let transfer_keys: Vec<u64> = state
        .pending_usb_transfers
        .iter()
        .filter(|(_, pending)| pending.target == target)
        .map(|(key, _)| *key)
        .collect();
    for key in transfer_keys {
        if let Some(pending) = state.pending_usb_transfers.remove(&key) {
            let result = usb_probe_result(
                UsbDescriptorProbeStatus::DeviceUnavailable,
                reason.to_string(),
                pending.target,
                pending.bus_id,
                pending.request_id,
                pending.transfer_id,
                Some(pending.session_id),
                pending.started_at_ms,
                Vec::new(),
                None,
            );
            let _ = pending.result_tx.send(result);
        }
    }
}

fn audio_source_label(source: LocalAudioCaptureSource) -> &'static str {
    match source {
        LocalAudioCaptureSource::Microphone => "Microphone",
        LocalAudioCaptureSource::Loopback => "Loopback",
    }
}

fn audio_format_label(format: &AudioFormat) -> String {
    format!(
        "{} Hz / {} ch / {:?} / {} ms",
        format.sample_rate, format.channels, format.sample_format, format.frame_ms
    )
}

fn audio_input_name_for_endpoint(state: &DaemonState, endpoint_id: Option<&str>) -> Option<String> {
    let endpoint_id = endpoint_id?;
    state
        .local_controls
        .audio_inputs
        .iter()
        .find(|device| device.endpoint_id.as_deref() == Some(endpoint_id))
        .map(|device| device.name.clone())
}

/// Discover available backends and select the best one
fn discover_and_select_backend() -> (
    Option<ResolvedInputMode>,
    Vec<BackendKind>,
    BackendHealth,
    Option<String>,
) {
    let mut candidates = vec![];

    let portable_capture_health = PortableCaptureBackend::new_for_test()
        .map(|backend| backend.health())
        .unwrap_or(BackendHealth::Degraded {
            reason: BackendFailureReason::InitializationFailed,
        });
    let portable_inject_health = PortableInjectBackend::new_for_test()
        .map(|backend| backend.health())
        .unwrap_or(BackendHealth::Degraded {
            reason: BackendFailureReason::InitializationFailed,
        });
    candidates.push(candidate_from_component_health(
        BackendKind::Portable,
        portable_capture_health,
        portable_inject_health.clone(),
    ));

    #[cfg(target_os = "windows")]
    {
        use rshare_input::backend::{WindowsNativeCaptureBackend, WindowsNativeInjectBackend};

        let capture_health = WindowsNativeCaptureBackend::new_for_test()
            .map(|backend| backend.health())
            .unwrap_or(BackendHealth::Degraded {
                reason: BackendFailureReason::InitializationFailed,
            });
        let inject_health = WindowsNativeInjectBackend::new_for_test()
            .map(|backend| backend.health())
            .unwrap_or(BackendHealth::Degraded {
                reason: BackendFailureReason::InitializationFailed,
            });

        candidates.push(candidate_from_component_health(
            BackendKind::WindowsNative,
            capture_health,
            inject_health,
        ));
    }

    #[cfg(target_os = "windows")]
    {
        use rshare_input::backend::{VirtualHidCaptureBackend, VirtualHidInjectBackend};

        let capture_health = VirtualHidCaptureBackend::new_for_test()
            .map(|backend| backend.health())
            .unwrap_or(BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            });
        let inject_health = VirtualHidInjectBackend::new_for_test()
            .map(|backend| backend.health())
            .unwrap_or(BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            });

        candidates.push(candidate_from_component_health(
            BackendKind::VirtualHid,
            capture_health,
            inject_health,
        ));
    }

    #[cfg(target_os = "linux")]
    {
        use rshare_input::backend::UInputInjectBackend;

        // Check Evdev capture availability WITHOUT starting it (to avoid device grab)
        // The actual capture will be started in try_start_evdev_capture()
        let evdev_capture_health = check_evdev_devices_available();

        // Try to initialize UInput injection backend
        let uinput_inject_health = match UInputInjectBackend::new() {
            Ok(backend) => {
                tracing::info!("UInput inject backend available");
                backend.health()
            }
            Err(e) => {
                tracing::warn!("UInput inject backend failed: {:?}", e);
                BackendHealth::Degraded {
                    reason: if e.to_string().contains("permission")
                        || e.to_string().contains("denied")
                        || e.to_string().contains("Permiss")
                    {
                        BackendFailureReason::PermissionDenied
                    } else {
                        BackendFailureReason::InitializationFailed
                    },
                }
            }
        };

        // Add pure Evdev candidate (both capture and inject)
        candidates.push(candidate_from_component_health(
            BackendKind::Evdev,
            evdev_capture_health.clone(),
            uinput_inject_health.clone(),
        ));

        // Add hybrid candidate: Evdev capture + Portable inject
        // This allows kernel-level input capture even when UInput is unavailable
        let hybrid_health = match (&evdev_capture_health, &portable_inject_health) {
            (BackendHealth::Healthy, BackendHealth::Healthy) => BackendHealth::Healthy,
            (BackendHealth::Degraded { reason }, _) => {
                // If Evdev capture fails, the hybrid backend is degraded
                BackendHealth::Degraded {
                    reason: reason.clone(),
                }
            }
            (_, BackendHealth::Degraded { reason }) => {
                // If Portable inject fails, the hybrid backend is degraded but still usable for capture
                tracing::info!("Hybrid backend: Evdev capture with degraded injection");
                BackendHealth::Degraded {
                    reason: reason.clone(),
                }
            }
        };

        candidates.push(BackendCandidate {
            kind: BackendKind::Portable, // Use Portable as the kind for hybrid
            healthy: matches!(hybrid_health, BackendHealth::Healthy),
            failure_reason: match hybrid_health {
                BackendHealth::Healthy => None,
                BackendHealth::Degraded { reason } => Some(reason),
            },
            capabilities: rshare_input::backend::BackendCapabilities::default(),
        });

        tracing::info!("Linux backend candidates: Evdev capture={:?}, UInput inject={:?}, Portable inject={:?}",
            evdev_capture_health, uinput_inject_health, portable_inject_health);
    }

    resolve_backend_selection(&candidates)
}

fn candidate_from_component_health(
    kind: BackendKind,
    capture_health: BackendHealth,
    inject_health: BackendHealth,
) -> BackendCandidate {
    let first_failure = match (&capture_health, &inject_health) {
        (BackendHealth::Degraded { reason }, _) => Some(reason.clone()),
        (_, BackendHealth::Degraded { reason }) => Some(reason.clone()),
        _ => None,
    };

    if first_failure.is_none() {
        BackendCandidate::healthy(kind)
    } else {
        BackendCandidate::unhealthy(
            kind,
            first_failure.unwrap_or(BackendFailureReason::Unavailable),
        )
    }
}

fn resolve_backend_selection(
    candidates: &[BackendCandidate],
) -> (
    Option<ResolvedInputMode>,
    Vec<BackendKind>,
    BackendHealth,
    Option<String>,
) {
    let selector = BackendSelector::new();
    let available_kinds: Vec<_> = candidates
        .iter()
        .filter(|c| c.healthy)
        .map(|c| c.kind)
        .collect();

    match selector.select(&candidates) {
        Some(result) => {
            let mode = result
                .to_input_mode()
                .unwrap_or(ResolvedInputMode::Portable);
            (
                Some(mode),
                available_kinds,
                BackendHealth::Healthy,
                result.degradation_reason.clone(),
            )
        }
        None => (
            None,
            Vec::new(),
            BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
            Some("No input backend initialized successfully".to_string()),
        ),
    }
}

fn enrich_display_topology_from_layout(
    capabilities: &mut [EndpointCapabilitySnapshot],
    node: Option<&LayoutNode>,
) {
    let Some(node) = node else {
        return;
    };
    let Some(display_capability) = capabilities
        .iter_mut()
        .find(|capability| capability.kind == EndpointCapabilityKind::DisplayTopology)
    else {
        return;
    };

    if node.displays.is_empty() {
        return;
    }

    display_capability.state = CapabilityState::Available;
    display_capability
        .details
        .insert("display_count".to_string(), node.displays.len().to_string());
    if let Some(primary) = node.primary_display() {
        display_capability
            .details
            .insert("primary_display_id".to_string(), primary.display_id.clone());
        display_capability.details.insert(
            "primary_resolution".to_string(),
            format!("{}x{}", primary.width, primary.height),
        );
    }
    display_capability.details.insert(
        "display_geometries".to_string(),
        node.displays
            .iter()
            .map(|display| {
                format!(
                    "{}:{}x{}@{},{}{}",
                    display.display_id,
                    display.width,
                    display.height,
                    display.x,
                    display.y,
                    if display.primary { ":primary" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(";"),
    );
}

fn is_device_connected(state: &DaemonState, id: DeviceId) -> bool {
    state
        .devices
        .get(&id)
        .map(|device| device.connected)
        .unwrap_or(false)
}

fn take_current_generation_for_connection_error(
    current_generations: &std::sync::RwLock<HashMap<DeviceId, ControlConnectionId>>,
    device_id: DeviceId,
    reported_generation: Option<ControlConnectionId>,
) -> Option<ControlConnectionId> {
    // An outbound connection attempt can fail before authentication and has no
    // generation. It is diagnostic-only and must never evict an authenticated
    // connection that happens to share the peer id.
    let reported_generation = reported_generation?;
    let mut generations = current_generations
        .write()
        .expect("input generation registry poisoned");
    if generations.get(&device_id) == Some(&reported_generation) {
        generations.remove(&device_id);
        Some(reported_generation)
    } else {
        None
    }
}

#[cfg(windows)]
fn set_local_shortcut_suppression(enabled: bool) {
    rshare_platform::windows::set_local_input_suppressed(enabled);
}

#[cfg(not(windows))]
fn set_local_shortcut_suppression(_enabled: bool) {}

#[cfg(windows)]
fn local_shortcut_suppression_supported(controls: &LocalControlDeviceSnapshot) -> bool {
    // The filter driver is a capture path only. Suppression is provided by the
    // native low-level hook, so only advertise it when that is the active
    // resolved capture backend.
    controls.capture_backend.active
        && matches!(
            controls.capture_backend.mode,
            Some(ResolvedInputMode::WindowsNative)
        )
}

#[cfg(not(windows))]
fn local_shortcut_suppression_supported(_controls: &LocalControlDeviceSnapshot) -> bool {
    false
}

struct PlatformShortcutSuppressor;

impl LocalShortcutSuppressor for PlatformShortcutSuppressor {
    fn set_suppressed(&self, enabled: bool) {
        set_local_shortcut_suppression(enabled);
    }
}

fn sync_local_shortcut_suppression(state: &DaemonState) {
    let should_suppress = state.features.suppress_local_shortcuts_when_remote
        && matches!(
            state.session.state(),
            ControlSessionState::RemoteActive { .. }
        );
    set_local_shortcut_suppression(should_suppress);
}

fn create_inject_backend(mode: Option<ResolvedInputMode>) -> Result<Box<dyn InjectBackend>> {
    #[cfg(not(target_os = "windows"))]
    let _ = mode;

    #[cfg(target_os = "windows")]
    if matches!(mode, Some(ResolvedInputMode::VirtualHid)) {
        use rshare_input::backend::VirtualHidInjectBackend;
        return Ok(Box::new(VirtualHidInjectBackend::new()?));
    }

    #[cfg(target_os = "windows")]
    if matches!(mode, Some(ResolvedInputMode::WindowsNative)) {
        use rshare_input::backend::WindowsNativeInjectBackend;
        return Ok(Box::new(WindowsNativeInjectBackend::new()?));
    }

    Ok(Box::new(PortableInjectBackend::new()?))
}

#[derive(Debug)]
struct UnavailableInjectBackend {
    kind: BackendKind,
    health: BackendHealth,
    error: String,
}

impl UnavailableInjectBackend {
    fn new(kind: BackendKind, health: BackendHealth, error: String) -> Self {
        Self {
            kind,
            health,
            error,
        }
    }
}

impl InjectBackend for UnavailableInjectBackend {
    fn kind(&self) -> BackendKind {
        self.kind
    }

    fn health(&self) -> BackendHealth {
        self.health.clone()
    }

    fn inject(&mut self, _event: InputEvent) -> Result<()> {
        anyhow::bail!("Input injection backend unavailable: {}", self.error)
    }

    fn is_active(&self) -> bool {
        false
    }
}

fn backend_kind_for_mode(mode: Option<ResolvedInputMode>) -> BackendKind {
    match mode {
        #[cfg(target_os = "windows")]
        Some(ResolvedInputMode::WindowsNative) => BackendKind::WindowsNative,
        #[cfg(target_os = "windows")]
        Some(ResolvedInputMode::VirtualHid) => BackendKind::VirtualHid,
        #[cfg(target_os = "linux")]
        Some(ResolvedInputMode::Evdev) => BackendKind::Evdev,
        #[cfg(target_os = "linux")]
        Some(ResolvedInputMode::UInput) => BackendKind::UInput,
        Some(ResolvedInputMode::Portable) | None => BackendKind::Portable,
    }
}

fn inject_backend_failure_reason(error: &anyhow::Error) -> BackendFailureReason {
    let error_text = error.to_string().to_lowercase();
    if error_text.contains("permission") || error_text.contains("accessibility") {
        BackendFailureReason::PermissionDenied
    } else {
        BackendFailureReason::InitializationFailed
    }
}

fn build_inject_backend(
    mode: Option<ResolvedInputMode>,
) -> (Box<dyn InjectBackend>, BackendHealth, Option<String>) {
    match create_inject_backend(mode) {
        Ok(backend) => {
            let health = backend.health();
            (backend, health, None)
        }
        Err(error) => {
            let reason = inject_backend_failure_reason(&error);
            let health = BackendHealth::Degraded { reason };
            let error = error.to_string();
            tracing::warn!("Input injection backend unavailable: {}", error);
            (
                Box::new(UnavailableInjectBackend::new(
                    backend_kind_for_mode(mode),
                    health.clone(),
                    error.clone(),
                )),
                health,
                Some(error),
            )
        }
    }
}

fn input_test_failure_status(error: &anyhow::Error) -> LocalInputTestStatus {
    let error_text = error.to_string().to_lowercase();
    if error_text.contains("permission") || error_text.contains("accessibility") {
        LocalInputTestStatus::PermissionDenied
    } else if error_text.contains("unavailable") || error_text.contains("not active") {
        LocalInputTestStatus::BackendUnavailable
    } else {
        LocalInputTestStatus::Failed
    }
}

fn endpoint_inject_error(error: &anyhow::Error) -> EndpointInjectError {
    let error_text = error.to_string().to_lowercase();
    if error_text.contains("permission") || error_text.contains("accessibility") {
        EndpointInjectError::PermissionDenied
    } else if error_text.contains("unavailable") || error_text.contains("not active") {
        EndpointInjectError::BackendUnavailable
    } else if error_text.contains("unsupported") {
        EndpointInjectError::UnsupportedEvent
    } else {
        EndpointInjectError::Failed
    }
}

fn degraded_unavailable_health() -> BackendHealth {
    BackendHealth::Degraded {
        reason: BackendFailureReason::Unavailable,
    }
}

fn endpoint_inject_failure_result(
    target: EndpointInjectTarget,
    request: &EndpointInjectRequest,
    backend_kind: Option<BackendKind>,
    health: BackendHealth,
    elapsed_ms: u64,
    error: EndpointInjectError,
) -> EndpointInjectResult {
    EndpointInjectResult {
        correlation_id: request.correlation_id.clone(),
        target,
        accepted: false,
        backend_kind,
        health,
        elapsed_ms,
        loopback_event_id: None,
        error: Some(error),
    }
}

fn endpoint_payload_to_input_event(request: &EndpointInjectRequest) -> Result<InputEvent> {
    match (&request.device_kind, &request.payload) {
        (
            rshare_core::EndpointEventKind::Keyboard,
            rshare_core::EndpointEventPayload::Keyboard { key, state },
        ) => Ok(InputEvent::key(
            parse_key_code(key)?,
            parse_button_state(state)?,
        )),
        (
            rshare_core::EndpointEventKind::Keyboard,
            rshare_core::EndpointEventPayload::TextCommit { text },
        ) => Ok(InputEvent::text_commit(text.clone())),
        (
            rshare_core::EndpointEventKind::Mouse,
            rshare_core::EndpointEventPayload::MouseMove { x, y, .. },
        ) => Ok(InputEvent::mouse_move(*x, *y)),
        (
            rshare_core::EndpointEventKind::Mouse,
            rshare_core::EndpointEventPayload::MouseButton { button, state, .. },
        ) => Ok(InputEvent::mouse_button(
            parse_mouse_button(button)?,
            parse_button_state(state)?,
        )),
        (
            rshare_core::EndpointEventKind::Mouse,
            rshare_core::EndpointEventPayload::MouseWheel {
                delta_x, delta_y, ..
            },
        ) => Ok(InputEvent::mouse_wheel(*delta_x, *delta_y)),
        _ => anyhow::bail!(
            "Unsupported endpoint inject event: {:?} {:?}",
            request.device_kind,
            request.payload
        ),
    }
}

fn parse_button_state(value: &str) -> Result<rshare_input::ButtonState> {
    match value.to_ascii_lowercase().as_str() {
        "pressed" | "press" | "down" | "true" | "1" => Ok(rshare_input::ButtonState::Pressed),
        "released" | "release" | "up" | "false" | "0" => Ok(rshare_input::ButtonState::Released),
        other => anyhow::bail!("Unsupported button state: {other}"),
    }
}

fn parse_mouse_button(value: &str) -> Result<rshare_input::MouseButton> {
    let value = value.trim();
    match value.to_ascii_lowercase().as_str() {
        "left" => Ok(rshare_input::MouseButton::Left),
        "middle" => Ok(rshare_input::MouseButton::Middle),
        "right" => Ok(rshare_input::MouseButton::Right),
        "back" => Ok(rshare_input::MouseButton::Back),
        "forward" => Ok(rshare_input::MouseButton::Forward),
        other if other.starts_with("other(") && other.ends_with(')') => {
            let number = other
                .trim_start_matches("other(")
                .trim_end_matches(')')
                .parse::<u8>()?;
            Ok(rshare_input::MouseButton::Other(number))
        }
        other => anyhow::bail!("Unsupported mouse button: {other}"),
    }
}

fn parse_key_code(value: &str) -> Result<rshare_input::KeyCode> {
    let value = value.trim();
    let key = match value {
        "Escape" | "Esc" => rshare_input::KeyCode::Escape,
        "Enter" | "Return" => rshare_input::KeyCode::Enter,
        "Tab" => rshare_input::KeyCode::Tab,
        "Backspace" => rshare_input::KeyCode::Backspace,
        "Delete" | "Del" => rshare_input::KeyCode::Delete,
        "Insert" | "Ins" => rshare_input::KeyCode::Insert,
        "Home" => rshare_input::KeyCode::Home,
        "End" => rshare_input::KeyCode::End,
        "PageUp" | "PgUp" => rshare_input::KeyCode::PageUp,
        "PageDown" | "PgDn" => rshare_input::KeyCode::PageDown,
        "Up" => rshare_input::KeyCode::Up,
        "Down" => rshare_input::KeyCode::Down,
        "Left" => rshare_input::KeyCode::Left,
        "Right" => rshare_input::KeyCode::Right,
        "ShiftLeft" => rshare_input::KeyCode::ShiftLeft,
        "ShiftRight" => rshare_input::KeyCode::ShiftRight,
        "ControlLeft" | "CtrlLeft" => rshare_input::KeyCode::ControlLeft,
        "ControlRight" | "CtrlRight" => rshare_input::KeyCode::ControlRight,
        "AltLeft" => rshare_input::KeyCode::AltLeft,
        "AltRight" => rshare_input::KeyCode::AltRight,
        "SuperLeft" | "MetaLeft" | "WinLeft" => rshare_input::KeyCode::SuperLeft,
        "SuperRight" | "MetaRight" | "WinRight" => rshare_input::KeyCode::SuperRight,
        "Space" => rshare_input::KeyCode::Space,
        "CapsLock" => rshare_input::KeyCode::CapsLock,
        "NumLock" => rshare_input::KeyCode::NumLock,
        "F1" => rshare_input::KeyCode::F1,
        "F2" => rshare_input::KeyCode::F2,
        "F3" => rshare_input::KeyCode::F3,
        "F4" => rshare_input::KeyCode::F4,
        "F5" => rshare_input::KeyCode::F5,
        "F6" => rshare_input::KeyCode::F6,
        "F7" => rshare_input::KeyCode::F7,
        "F8" => rshare_input::KeyCode::F8,
        "F9" => rshare_input::KeyCode::F9,
        "F10" => rshare_input::KeyCode::F10,
        "F11" => rshare_input::KeyCode::F11,
        "F12" => rshare_input::KeyCode::F12,
        "Keypad0" => rshare_input::KeyCode::Keypad0,
        "Keypad1" => rshare_input::KeyCode::Keypad1,
        "Keypad2" => rshare_input::KeyCode::Keypad2,
        "Keypad3" => rshare_input::KeyCode::Keypad3,
        "Keypad4" => rshare_input::KeyCode::Keypad4,
        "Keypad5" => rshare_input::KeyCode::Keypad5,
        "Keypad6" => rshare_input::KeyCode::Keypad6,
        "Keypad7" => rshare_input::KeyCode::Keypad7,
        "Keypad8" => rshare_input::KeyCode::Keypad8,
        "Keypad9" => rshare_input::KeyCode::Keypad9,
        "KeypadAdd" => rshare_input::KeyCode::KeypadAdd,
        "KeypadSubtract" => rshare_input::KeyCode::KeypadSubtract,
        "KeypadMultiply" => rshare_input::KeyCode::KeypadMultiply,
        "KeypadDivide" => rshare_input::KeyCode::KeypadDivide,
        "KeypadDecimal" => rshare_input::KeyCode::KeypadDecimal,
        "KeypadEnter" => rshare_input::KeyCode::KeypadEnter,
        other if other.len() == 1 => {
            rshare_input::KeyCode::Char(other.as_bytes()[0].to_ascii_uppercase())
        }
        other if other.starts_with("Raw(") && other.ends_with(')') => {
            let number = other
                .trim_start_matches("Raw(")
                .trim_end_matches(')')
                .parse::<u32>()?;
            rshare_input::KeyCode::Raw(number)
        }
        other => anyhow::bail!("Unsupported key code: {other}"),
    };
    Ok(key)
}

async fn run_local_input_test(
    inject_backend: &InputInjectionHandle,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    test: LocalInputTestRequest,
) -> LocalInputTestResult {
    let mut diagnostic_payload = BTreeMap::new();
    if !inject_backend.backend_snapshot().active {
        return LocalInputTestResult::failed(
            LocalInputTestStatus::BackendUnavailable,
            "Input injection backend is not active.",
        );
    }
    let result = match test.kind {
        LocalInputTestKind::KeyboardShift => {
            match inject_backend
                .inject_trusted_local(InputEvent::key(
                    rshare_input::KeyCode::ShiftLeft,
                    rshare_input::ButtonState::Pressed,
                ))
                .await
            {
                Ok(()) => inject_backend
                    .inject_trusted_local(InputEvent::key(
                        rshare_input::KeyCode::ShiftLeft,
                        rshare_input::ButtonState::Released,
                    ))
                    .await
                    .map_err(anyhow::Error::new),
                Err(error) => Err(anyhow::Error::new(error)),
            }
        }
        LocalInputTestKind::MouseMove => {
            let (x, y) = {
                let state = state.read().await;
                (state.local_controls.mouse.x, state.local_controls.mouse.y)
            };
            let ((first_x, first_y), (second_x, second_y)) =
                ((x.saturating_add(8), y.saturating_add(8)), (x, y));
            diagnostic_payload.insert("x".to_string(), first_x.to_string());
            diagnostic_payload.insert("y".to_string(), first_y.to_string());
            diagnostic_payload.insert("return_x".to_string(), second_x.to_string());
            diagnostic_payload.insert("return_y".to_string(), second_y.to_string());
            match inject_backend
                .inject_trusted_local(InputEvent::mouse_move(first_x, first_y))
                .await
            {
                Ok(()) => inject_backend
                    .inject_trusted_local(InputEvent::mouse_move(second_x, second_y))
                    .await
                    .map_err(anyhow::Error::new),
                Err(error) => Err(anyhow::Error::new(error)),
            }
        }
        LocalInputTestKind::VirtualGamepadStatus => {
            return LocalInputTestResult::failed(
                LocalInputTestStatus::Unsupported,
                "Virtual HID gamepad injection is not implemented in this build.",
            );
        }
    };

    match result {
        Ok(()) => {
            let event = record_injected_test_event(state, test.kind, diagnostic_payload).await;
            let _ = local_events_tx.send(event);
            LocalInputTestResult::success("Local input injection test completed.")
        }
        Err(error) => {
            LocalInputTestResult::failed(input_test_failure_status(&error), error.to_string())
        }
    }
}

async fn record_endpoint_inject_event(
    state: &Arc<RwLock<DaemonState>>,
    request: &EndpointInjectRequest,
) -> (EndpointEvent, LocalInputDiagnosticEvent) {
    let mut state = state.write().await;
    let sequence = state.local_controls.sequence.saturating_add(1);
    state.local_controls.sequence = sequence;
    let timestamp_ms = timestamp_ms_now();
    let device_kind = match request.device_kind {
        rshare_core::EndpointEventKind::Keyboard => LocalInputDeviceKind::Keyboard,
        rshare_core::EndpointEventKind::Mouse => LocalInputDeviceKind::Mouse,
        rshare_core::EndpointEventKind::Gamepad => LocalInputDeviceKind::Gamepad,
        rshare_core::EndpointEventKind::Usb => LocalInputDeviceKind::Usb,
        rshare_core::EndpointEventKind::Display => LocalInputDeviceKind::Display,
        rshare_core::EndpointEventKind::Audio => LocalInputDeviceKind::Audio,
        rshare_core::EndpointEventKind::Backend => LocalInputDeviceKind::Backend,
        rshare_core::EndpointEventKind::Session => LocalInputDeviceKind::Backend,
    };
    let (event_kind, summary, mut payload) = endpoint_inject_diagnostic_payload(request);
    payload.insert("correlation_id".to_string(), request.correlation_id.clone());

    state.arm_injected_loopback(device_kind, timestamp_ms);
    let event = LocalInputDiagnosticEvent {
        sequence,
        timestamp_ms,
        device_kind,
        event_kind,
        summary,
        device_id: Some(format!("rshare-inject-{}", request.device_kind_slug())),
        device_instance_id: None,
        capture_path: Some("daemon-endpoint-inject".to_string()),
        source: LocalInputEventSource::InjectedLoopback,
        payload,
    };
    push_recent_local_event(&mut state.local_controls, event.clone());
    let endpoint_event = state.endpoint_event_from_local(event.clone());
    (endpoint_event, event)
}

fn endpoint_inject_diagnostic_payload(
    request: &EndpointInjectRequest,
) -> (String, String, BTreeMap<String, String>) {
    let mut payload = BTreeMap::new();
    match &request.payload {
        rshare_core::EndpointEventPayload::Keyboard { key, state } => {
            payload.insert("key".to_string(), key.clone());
            payload.insert("state".to_string(), state.clone());
            (
                "key".to_string(),
                format!("Injected {key} {state}"),
                payload,
            )
        }
        rshare_core::EndpointEventPayload::TextCommit { text } => {
            payload.insert("char_count".to_string(), text.chars().count().to_string());
            (
                "text".to_string(),
                format!("Injected text commit ({} chars)", text.chars().count()),
                payload,
            )
        }
        rshare_core::EndpointEventPayload::MouseMove { x, y, display_id } => {
            payload.insert("x".to_string(), x.to_string());
            payload.insert("y".to_string(), y.to_string());
            if let Some(display_id) = display_id {
                payload.insert("display_id".to_string(), display_id.clone());
            }
            (
                "move".to_string(),
                format!("Injected mouse move {x},{y}"),
                payload,
            )
        }
        rshare_core::EndpointEventPayload::MouseButton {
            button,
            state,
            x,
            y,
        } => {
            payload.insert("button".to_string(), button.clone());
            payload.insert("state".to_string(), state.clone());
            payload.insert("x".to_string(), x.to_string());
            payload.insert("y".to_string(), y.to_string());
            (
                "button".to_string(),
                format!("Injected mouse {button} {state}"),
                payload,
            )
        }
        rshare_core::EndpointEventPayload::MouseWheel {
            delta_x,
            delta_y,
            x,
            y,
        } => {
            payload.insert("delta_x".to_string(), delta_x.to_string());
            payload.insert("delta_y".to_string(), delta_y.to_string());
            payload.insert("x".to_string(), x.to_string());
            payload.insert("y".to_string(), y.to_string());
            (
                "wheel".to_string(),
                format!("Injected mouse wheel {delta_x},{delta_y}"),
                payload,
            )
        }
        _ => (
            "injected".to_string(),
            "Injected endpoint event".to_string(),
            payload,
        ),
    }
}

trait EndpointInjectRequestExt {
    fn device_kind_slug(&self) -> &'static str;
}

impl EndpointInjectRequestExt for EndpointInjectRequest {
    fn device_kind_slug(&self) -> &'static str {
        match self.device_kind {
            rshare_core::EndpointEventKind::Keyboard => "keyboard",
            rshare_core::EndpointEventKind::Mouse => "mouse",
            rshare_core::EndpointEventKind::Gamepad => "gamepad",
            rshare_core::EndpointEventKind::Usb => "usb",
            rshare_core::EndpointEventKind::Display => "display",
            rshare_core::EndpointEventKind::Audio => "audio",
            rshare_core::EndpointEventKind::Backend => "backend",
            rshare_core::EndpointEventKind::Session => "session",
        }
    }
}

async fn run_remote_latency_test(
    network_manager: &Arc<Mutex<NetworkManager>>,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    device_id: DeviceId,
) -> LocalInputTestResult {
    let now = timestamp_ms_now();
    let (event, endpoint_switch) = {
        let mut state = state.write().await;
        if !is_device_connected(&state, device_id) {
            return LocalInputTestResult::failed(
                LocalInputTestStatus::BackendUnavailable,
                format!("Device {} is not connected.", short_device_id(device_id)),
            );
        }

        let endpoint_switch = state.features.auto_endpoint_latency_probe;
        let sequence = state.local_controls.sequence.saturating_add(1);
        state.local_controls.sequence = sequence;
        state.pending_latency_probes.insert(
            sequence,
            PendingLatencyProbe {
                target: device_id,
                sent_at_ms: now,
                role: PendingLatencyProbeRole::LocalRequested,
            },
        );

        let mut payload = BTreeMap::new();
        payload.insert("probe_sequence".to_string(), sequence.to_string());
        payload.insert("sent_timestamp_ms".to_string(), now.to_string());
        payload.insert("endpoint_switch".to_string(), endpoint_switch.to_string());
        payload.insert("direction".to_string(), "local_to_remote".to_string());
        let event = LocalInputDiagnosticEvent {
            sequence,
            timestamp_ms: now,
            device_kind: LocalInputDeviceKind::Backend,
            event_kind: if endpoint_switch {
                "latency_endpoint_probe_sent".to_string()
            } else {
                "latency_probe_sent".to_string()
            },
            summary: if endpoint_switch {
                format!(
                    "Dual-end latency probe sent to {}",
                    short_device_id(device_id)
                )
            } else {
                format!("Latency probe sent to {}", short_device_id(device_id))
            },
            device_id: Some(device_id.to_string()),
            device_instance_id: None,
            capture_path: Some("rshare-net".to_string()),
            source: LocalInputEventSource::System,
            payload,
        };
        push_recent_local_event(&mut state.local_controls, event.clone());
        (event, endpoint_switch)
    };
    let sequence = event.sequence;
    let _ = local_events_tx.send(event);

    let result = {
        let mut manager = network_manager.lock().await;
        manager
            .send_to(
                &device_id,
                Message::LatencyProbe {
                    sequence,
                    timestamp_ms: now,
                    endpoint_switch,
                    origin_sequence: None,
                },
            )
            .await
    };

    match result {
        Ok(()) if endpoint_switch => LocalInputTestResult::success(format!(
            "Dual-end latency probe sent to {}.",
            short_device_id(device_id)
        )),
        Ok(()) => LocalInputTestResult::success(format!(
            "Latency probe sent to {}.",
            short_device_id(device_id)
        )),
        Err(error) => {
            state.write().await.pending_latency_probes.remove(&sequence);
            LocalInputTestResult::failed(LocalInputTestStatus::Failed, error.to_string())
        }
    }
}

async fn run_remote_usb_descriptor_probe(
    network_manager: &Arc<Mutex<NetworkManager>>,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    device_id: DeviceId,
    bus_id: String,
) -> UsbDescriptorProbeResult {
    let started_at_ms = timestamp_ms_now();
    let features = {
        let state = state.read().await;
        state.features.clone()
    };
    if !features.usb_forwarding_experimental {
        return usb_probe_result(
            UsbDescriptorProbeStatus::Failed,
            "Experimental USB forwarding is disabled in settings.".to_string(),
            device_id,
            bus_id,
            0,
            0,
            None,
            started_at_ms,
            Vec::new(),
            None,
        );
    }
    if !features.usb_descriptor_probe {
        return usb_probe_result(
            UsbDescriptorProbeStatus::Failed,
            "USB descriptor probes are disabled in settings.".to_string(),
            device_id,
            bus_id,
            0,
            0,
            None,
            started_at_ms,
            Vec::new(),
            None,
        );
    }
    let (request_id, transfer_id, result_rx, event) = {
        let mut state = state.write().await;
        if !is_device_connected(&state, device_id) {
            return usb_probe_result(
                UsbDescriptorProbeStatus::DeviceUnavailable,
                format!("Device {} is not connected.", short_device_id(device_id)),
                device_id,
                bus_id,
                0,
                0,
                None,
                started_at_ms,
                Vec::new(),
                None,
            );
        }

        let request_id = state.local_controls.sequence.saturating_add(1);
        let transfer_id = request_id.saturating_add(1);
        state.local_controls.sequence = transfer_id;
        let (result_tx, result_rx) = oneshot::channel();
        state.pending_usb_claims.insert(
            request_id,
            PendingUsbClaim {
                target: device_id,
                bus_id: bus_id.clone(),
                request_id,
                transfer_id,
                started_at_ms,
                result_tx,
            },
        );

        let mut payload = BTreeMap::new();
        payload.insert("request_id".to_string(), request_id.to_string());
        payload.insert("transfer_id".to_string(), transfer_id.to_string());
        payload.insert("bus_id".to_string(), bus_id.clone());
        let event = record_usb_diagnostic_event(
            &mut state,
            Some(device_id),
            "usb_descriptor_probe_sent",
            format!(
                "USB descriptor probe sent to {}",
                short_device_id(device_id)
            ),
            payload,
        );
        (request_id, transfer_id, result_rx, event)
    };
    let _ = local_events_tx.send(event);

    let claim_request = UsbDeviceClaimRequest {
        request_id,
        bus_id: bus_id.clone(),
        exclusive: false,
        configuration_value: None,
        interface_numbers: Vec::new(),
    };
    let send_result = {
        let mut manager = network_manager.lock().await;
        manager
            .send_to(
                &device_id,
                Message::UsbDeviceClaimRequest {
                    request: claim_request,
                },
            )
            .await
    };
    if let Err(error) = send_result {
        state.write().await.pending_usb_claims.remove(&request_id);
        return usb_probe_result(
            UsbDescriptorProbeStatus::Failed,
            error.to_string(),
            device_id,
            bus_id,
            request_id,
            transfer_id,
            None,
            started_at_ms,
            Vec::new(),
            None,
        );
    }

    match tokio::time::timeout(Duration::from_secs(5), result_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => usb_probe_result(
            UsbDescriptorProbeStatus::Failed,
            "USB descriptor probe was cancelled.".to_string(),
            device_id,
            bus_id,
            request_id,
            transfer_id,
            None,
            started_at_ms,
            Vec::new(),
            None,
        ),
        Err(_) => {
            let mut state = state.write().await;
            state.pending_usb_claims.remove(&request_id);
            state.pending_usb_transfers.remove(&transfer_id);
            usb_probe_result(
                UsbDescriptorProbeStatus::Timeout,
                "USB descriptor probe timed out.".to_string(),
                device_id,
                bus_id,
                request_id,
                transfer_id,
                None,
                started_at_ms,
                Vec::new(),
                None,
            )
        }
    }
}

fn usb_device_descriptor_probe_transfer(
    transfer_id: u64,
    bus_id: String,
    session_id: DeviceId,
) -> UsbTransferPayload {
    UsbTransferPayload {
        transfer_id,
        bus_id,
        session_id: Some(session_id),
        endpoint_address: 0,
        transfer_kind: UsbTransferKind::Control,
        direction: UsbTransferDirection::In,
        setup_packet: None,
        control_setup: Some(UsbControlSetupPacket {
            request_type: 0x80,
            request: 0x06,
            value: 0x0100,
            index: 0,
            length: 18,
        }),
        stream_id: None,
        expected_length: Some(18),
        flags: Vec::new(),
        iso_packets: Vec::new(),
        data: Vec::new(),
        timeout_ms: 1_000,
    }
}

fn usb_probe_result(
    status: UsbDescriptorProbeStatus,
    message: String,
    device_id: DeviceId,
    bus_id: String,
    request_id: u64,
    transfer_id: u64,
    session_id: Option<DeviceId>,
    started_at_ms: u64,
    descriptor_bytes: Vec<u8>,
    actual_length: Option<u32>,
) -> UsbDescriptorProbeResult {
    let descriptor = usb_device_descriptor_from_bytes(&bus_id, &descriptor_bytes);
    UsbDescriptorProbeResult {
        status,
        message,
        device_id,
        bus_id,
        request_id,
        transfer_id,
        session_id,
        elapsed_ms: Some(timestamp_ms_now().saturating_sub(started_at_ms)),
        actual_length,
        descriptor,
        descriptor_bytes,
    }
}

fn usb_device_descriptor_from_bytes(bus_id: &str, bytes: &[u8]) -> Option<UsbDeviceDescriptor> {
    if bytes.len() < 18 || bytes[0] < 18 || bytes[1] != 0x01 {
        return None;
    }
    Some(UsbDeviceDescriptor {
        bus_id: bus_id.to_string(),
        vendor_id: u16::from_le_bytes([bytes[8], bytes[9]]),
        product_id: u16::from_le_bytes([bytes[10], bytes[11]]),
        class_code: bytes[4],
        subclass_code: bytes[5],
        protocol_code: bytes[6],
        manufacturer: None,
        product: None,
        serial_number: None,
        usb_version_bcd: u16::from_le_bytes([bytes[2], bytes[3]]),
        device_version_bcd: u16::from_le_bytes([bytes[12], bytes[13]]),
        speed: UsbDeviceSpeed::Unknown,
        active_configuration: None,
        container_id: None,
        capture_exclusive_required: true,
        configurations: Vec::new(),
        endpoints: Vec::new(),
    })
}

async fn start_endpoint_switch_latency_probe(
    network_manager: &Arc<Mutex<NetworkManager>>,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    diagnostics: &DiagnosticsHandle,
    target: DeviceId,
    origin_sequence: u64,
) {
    let now = timestamp_ms_now();
    let Some((sequence, event)) = ({
        let mut state = state.write().await;
        if !is_device_connected(&state, target) {
            None
        } else {
            let sequence = state.local_controls.sequence.saturating_add(1);
            state.local_controls.sequence = sequence;
            state.pending_latency_probes.insert(
                sequence,
                PendingLatencyProbe {
                    target,
                    sent_at_ms: now,
                    role: PendingLatencyProbeRole::EndpointSwitchReport {
                        origin_device_id: target,
                        origin_sequence,
                    },
                },
            );

            let mut payload = BTreeMap::new();
            payload.insert("probe_sequence".to_string(), sequence.to_string());
            payload.insert(
                "origin_probe_sequence".to_string(),
                origin_sequence.to_string(),
            );
            payload.insert("sent_timestamp_ms".to_string(), now.to_string());
            payload.insert("endpoint_switch".to_string(), "false".to_string());
            payload.insert("direction".to_string(), "endpoint_to_origin".to_string());
            let event = LocalInputDiagnosticEvent {
                sequence,
                timestamp_ms: now,
                device_kind: LocalInputDeviceKind::Backend,
                event_kind: "latency_endpoint_switch_sent".to_string(),
                summary: format!(
                    "Endpoint switched latency probe sent to {}",
                    short_device_id(target)
                ),
                device_id: Some(target.to_string()),
                device_instance_id: None,
                capture_path: Some("rshare-net".to_string()),
                source: LocalInputEventSource::System,
                payload,
            };
            push_recent_local_event(&mut state.local_controls, event.clone());
            Some((sequence, event))
        }
    }) else {
        return;
    };

    let _ = local_events_tx.send(event.clone());
    diagnostics.record_discrete(event);

    let result = {
        let mut manager = network_manager.lock().await;
        manager
            .send_to(
                &target,
                Message::LatencyProbe {
                    sequence,
                    timestamp_ms: now,
                    endpoint_switch: false,
                    origin_sequence: Some(origin_sequence),
                },
            )
            .await
    };

    if let Err(error) = result {
        let event = {
            let mut state = state.write().await;
            state.pending_latency_probes.remove(&sequence);
            let mut payload = BTreeMap::new();
            payload.insert("probe_sequence".to_string(), sequence.to_string());
            payload.insert(
                "origin_probe_sequence".to_string(),
                origin_sequence.to_string(),
            );
            payload.insert("error".to_string(), error.to_string());
            record_latency_diagnostic_event(
                &mut state,
                target,
                "latency_endpoint_switch_failed",
                format!(
                    "Endpoint switched latency probe to {} failed: {}",
                    short_device_id(target),
                    error
                ),
                payload,
            )
        };
        let _ = local_events_tx.send(event.clone());
        diagnostics.record_discrete(event);
    }
}

async fn handle_network_message(
    state: &Arc<RwLock<DaemonState>>,
    network_manager: &Arc<Mutex<NetworkManager>>,
    input_registry: &Arc<ConnectionRegistry>,
    inject_backend: &InputInjectionHandle,
    audio_runtime: &audio_runtime::AudioRuntimeHandle,
    usb_runtime: &UsbHostRuntime,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    endpoint_events_tx: &broadcast::Sender<EndpointEvent>,
    diagnostics: &DiagnosticsHandle,
    from: DeviceId,
    control_connection_id: ControlConnectionId,
    message: Message,
) {
    match message {
        Message::InputDiagnostic { event, .. } => {
            let endpoint_event = {
                let mut event = EndpointEvent::from_local_diagnostic(from, event.clone());
                event.source = rshare_core::EndpointEventSource::RemoteMirror;
                event
            };
            let (event, mirrored) = {
                let mut state = state.write().await;
                let mirrored = state.mirror_remote_endpoint_event(from, endpoint_event);
                (
                    record_remote_diagnostic_event(&mut state, from, event),
                    mirrored,
                )
            };
            let _ = local_events_tx.send(event);
            let _ = endpoint_events_tx.send(mirrored);
        }
        Message::EndpointEventSubscribe { filter } => {
            let subscriber_id = DiagnosticSubscriberId {
                peer_id: from,
                control_connection_id,
            };
            let (events, local_device_id) = {
                let mut state = state.write().await;
                (
                    state.endpoint_events(&filter, None, Some(128)),
                    state.status.device_id,
                )
            };
            let result = input_registry
                .peer(&from)
                .filter(|peer| {
                    diagnostic_generation_is_current(
                        subscriber_id,
                        Some(peer.auth.control_connection_id),
                    )
                })
                .ok_or_else(|| "diagnostics connection generation is no longer current".to_string())
                .and_then(|peer| {
                    match ClassifiedMessage::try_from(Message::EndpointEventSnapshot { events }) {
                        Ok(ClassifiedMessage::Telemetry(frame)) => peer
                            .transport
                            .try_send_telemetry(frame)
                            .map_err(|error| error.to_string()),
                        Ok(_) => Err("endpoint snapshot did not select telemetry lane".to_string()),
                        Err(error) => Err(error.to_string()),
                    }
                });
            if let Err(error) = result {
                tracing::debug!(
                    "Failed to answer endpoint event subscription from {}: {}",
                    from,
                    error
                );
            } else {
                spawn_peer_diagnostics_forwarder(
                    diagnostics,
                    input_registry.clone(),
                    local_device_id,
                    subscriber_id,
                );
            };
        }
        Message::EndpointEventSnapshot { events } => {
            for event in events {
                let mirrored = {
                    let mut state = state.write().await;
                    state.mirror_remote_endpoint_event(from, event)
                };
                let _ = endpoint_events_tx.send(mirrored);
            }
        }
        Message::EndpointEventDelta { event } => {
            let mirrored = {
                let mut state = state.write().await;
                state.mirror_remote_endpoint_event(from, event)
            };
            let _ = endpoint_events_tx.send(mirrored);
        }
        Message::EndpointInjectRequest { request } => {
            let result = inject_endpoint_event(
                network_manager,
                inject_backend,
                state,
                local_events_tx,
                EndpointInjectTarget::Local,
                request,
            )
            .await;
            let send_result = {
                let mut manager = network_manager.lock().await;
                manager
                    .send_to(&from, Message::EndpointInjectResult { result })
                    .await
            };
            if let Err(error) = send_result {
                tracing::debug!(
                    "Failed to send endpoint inject result to {}: {}",
                    from,
                    error
                );
            }
        }
        Message::EndpointInjectResult { result } => {
            let completed = {
                let mut state = state.write().await;
                state.complete_pending_endpoint_inject(from, result)
            };
            if !completed {
                tracing::debug!(
                    "Received endpoint inject result from {} without pending request",
                    from
                );
            }
        }
        Message::LatencyProbe {
            sequence,
            timestamp_ms,
            endpoint_switch,
            origin_sequence,
        } => {
            let received_timestamp_ms = timestamp_ms_now();
            let ack_timestamp_ms = timestamp_ms_now();
            let result = {
                let mut manager = network_manager.lock().await;
                manager
                    .send_to(
                        &from,
                        Message::LatencyProbeAck {
                            sequence,
                            sent_timestamp_ms: timestamp_ms,
                            received_timestamp_ms,
                            ack_timestamp_ms,
                            origin_sequence,
                        },
                    )
                    .await
            };
            if let Err(error) = result {
                tracing::debug!("Failed to answer latency probe from {}: {}", from, error);
            }
            let allow_endpoint_switch = {
                let state = state.read().await;
                state.features.auto_endpoint_latency_probe
            };
            if endpoint_switch && allow_endpoint_switch {
                start_endpoint_switch_latency_probe(
                    network_manager,
                    state,
                    local_events_tx,
                    diagnostics,
                    from,
                    sequence,
                )
                .await;
            }
        }
        Message::LatencyProbeAck {
            sequence,
            sent_timestamp_ms,
            received_timestamp_ms,
            ack_timestamp_ms,
            origin_sequence,
        } => {
            let now = timestamp_ms_now();
            let (event, should_broadcast) = {
                let mut state = state.write().await;
                let pending = state.pending_latency_probes.remove(&sequence);
                let target = pending.as_ref().map(|probe| probe.target).unwrap_or(from);
                let local_sent_at_ms = pending
                    .as_ref()
                    .map(|probe| probe.sent_at_ms)
                    .unwrap_or(sent_timestamp_ms);
                let mut payload = BTreeMap::new();
                payload.insert("probe_sequence".to_string(), sequence.to_string());
                if let Some(origin_sequence) = origin_sequence {
                    payload.insert(
                        "origin_probe_sequence".to_string(),
                        origin_sequence.to_string(),
                    );
                }
                let (network_round_trip_ms, raw_round_trip_ms) = add_latency_measurement_payload(
                    &mut payload,
                    local_sent_at_ms,
                    sent_timestamp_ms,
                    received_timestamp_ms,
                    ack_timestamp_ms,
                    now,
                );
                let estimated_one_way_ms = network_round_trip_ms / 2;

                let (event_kind, direction, summary, should_broadcast) = match pending
                    .as_ref()
                    .map(|probe| &probe.role)
                {
                    Some(PendingLatencyProbeRole::EndpointSwitchReport {
                        origin_device_id,
                        origin_sequence,
                    }) => {
                        payload
                            .insert("origin_device_id".to_string(), origin_device_id.to_string());
                        payload.insert(
                            "origin_probe_sequence".to_string(),
                            origin_sequence.to_string(),
                        );
                        (
                            "latency_endpoint_switch_ack",
                            "endpoint_to_origin",
                            format!(
                                "Endpoint-side latency to {}: {} ms RTT / ~{} ms one-way",
                                short_device_id(*origin_device_id),
                                network_round_trip_ms,
                                estimated_one_way_ms
                            ),
                            true,
                        )
                    }
                    _ => (
                        "latency_probe_ack",
                        "origin_to_endpoint",
                        format!(
                            "Latency to {}: {} ms RTT / ~{} ms one-way",
                            short_device_id(target),
                            network_round_trip_ms,
                            estimated_one_way_ms
                        ),
                        false,
                    ),
                };
                payload.insert("direction".to_string(), direction.to_string());
                payload.insert("raw_latency_ms".to_string(), raw_round_trip_ms.to_string());
                let event = record_latency_diagnostic_event(
                    &mut state, target, event_kind, summary, payload,
                );
                (event, should_broadcast)
            };
            let _ = local_events_tx.send(event.clone());
            if should_broadcast {
                diagnostics.record_discrete(event);
            }
        }
        Message::AudioStreamStart {
            stream_id,
            source_device_id,
            format,
        } => {
            let audio_forwarding_enabled = {
                let state = state.read().await;
                state.features.audio_forwarding
            };
            if !audio_forwarding_enabled {
                let message = "Audio forwarding is disabled in settings.".to_string();
                let event = {
                    let mut state = state.write().await;
                    state.local_controls.audio_stream_state.active = false;
                    state.local_controls.audio_stream_state.last_error = Some(message.clone());
                    let mut payload = BTreeMap::new();
                    payload.insert("stream_id".to_string(), stream_id.to_string());
                    payload.insert("source_device_id".to_string(), source_device_id.to_string());
                    record_audio_diagnostic_event(
                        &mut state,
                        "render_disabled",
                        message.clone(),
                        payload,
                    )
                };
                let _ = local_events_tx.send(event);
                let _ = network_manager
                    .lock()
                    .await
                    .send_to(&from, Message::AudioStreamError { stream_id, message })
                    .await;
                return;
            }
            let render_result = audio_runtime.start_render(stream_id, format.clone());
            let event = {
                let mut state = state.write().await;
                match render_result {
                    Ok(stats) => {
                        state.local_controls.audio_stream_state.active = true;
                        state.local_controls.audio_stream_state.target_device_id =
                            Some(source_device_id.to_string());
                        state.local_controls.audio_stream_state.stream_id =
                            Some(stream_id.to_string());
                        state.local_controls.audio_stream_state.frames_received =
                            stats.frames_received;
                        state.local_controls.audio_stream_state.underruns = stats.underruns;
                        state.local_controls.audio_stream_state.overruns = stats.overruns;
                        state.local_controls.audio_stream_state.latency_ms =
                            Some(stats.buffer_depth_ms);
                        state.local_controls.audio_stream_state.last_error = None;
                        let mut payload = BTreeMap::new();
                        payload.insert("stream_id".to_string(), stream_id.to_string());
                        payload
                            .insert("source_device_id".to_string(), source_device_id.to_string());
                        payload.insert("format".to_string(), audio_format_label(&format));
                        record_audio_diagnostic_event(
                            &mut state,
                            "render_start",
                            format!(
                                "Audio render stream started from {}",
                                short_device_id(source_device_id)
                            ),
                            payload,
                        )
                    }
                    Err(error) => {
                        state.local_controls.audio_stream_state.active = false;
                        state.local_controls.audio_stream_state.last_error =
                            Some(error.to_string());
                        let mut payload = BTreeMap::new();
                        payload.insert("stream_id".to_string(), stream_id.to_string());
                        payload
                            .insert("source_device_id".to_string(), source_device_id.to_string());
                        payload.insert("error".to_string(), error.to_string());
                        record_audio_diagnostic_event(
                            &mut state,
                            "render_error",
                            format!("Audio render stream failed: {error}"),
                            payload,
                        )
                    }
                }
            };
            let _ = local_events_tx.send(event);
        }
        Message::AudioFrame { frame } => {
            let audio_forwarding_enabled = {
                let state = state.read().await;
                state.features.audio_forwarding
            };
            if !audio_forwarding_enabled {
                return;
            }
            let render_result = audio_runtime.push_frame(&frame);
            let event = {
                let mut state = state.write().await;
                match render_result {
                    Ok(stats) => {
                        state.local_controls.audio_stream_state.active = true;
                        state.local_controls.audio_stream_state.stream_id =
                            Some(frame.stream_id.to_string());
                        state.local_controls.audio_stream_state.frames_received =
                            stats.frames_received;
                        state.local_controls.audio_stream_state.underruns = stats.underruns;
                        state.local_controls.audio_stream_state.overruns = stats.overruns;
                        state.local_controls.audio_stream_state.latency_ms =
                            Some(stats.buffer_depth_ms);
                        state.local_controls.audio_stream_state.last_error = None;

                        if stats.frames_received % 10 == 0 {
                            let mut payload = BTreeMap::new();
                            payload.insert("stream_id".to_string(), frame.stream_id.to_string());
                            payload.insert(
                                "frames_received".to_string(),
                                stats.frames_received.to_string(),
                            );
                            payload.insert(
                                "buffer_depth_ms".to_string(),
                                stats.buffer_depth_ms.to_string(),
                            );
                            payload.insert("underruns".to_string(), stats.underruns.to_string());
                            payload.insert("overruns".to_string(), stats.overruns.to_string());
                            Some(record_audio_diagnostic_event(
                                &mut state,
                                "render_frame",
                                format!("Audio render received {} frames", stats.frames_received),
                                payload,
                            ))
                        } else {
                            None
                        }
                    }
                    Err(error) => {
                        state.local_controls.audio_stream_state.last_error =
                            Some(error.to_string());
                        let mut payload = BTreeMap::new();
                        payload.insert("stream_id".to_string(), frame.stream_id.to_string());
                        payload.insert("error".to_string(), error.to_string());
                        Some(record_audio_diagnostic_event(
                            &mut state,
                            "render_error",
                            format!("Audio frame render failed: {error}"),
                            payload,
                        ))
                    }
                }
            };
            if let Some(event) = event {
                let _ = local_events_tx.send(event);
            }
        }
        Message::AudioStreamStop { stream_id, reason } => {
            audio_runtime.stop_render();
            let event = {
                let mut state = state.write().await;
                state.local_controls.audio_stream_state.active = false;
                state.local_controls.audio_stream_state.stream_id = None;
                let mut payload = BTreeMap::new();
                payload.insert("stream_id".to_string(), stream_id.to_string());
                payload.insert("reason".to_string(), reason.clone());
                record_audio_diagnostic_event(
                    &mut state,
                    "render_stop",
                    format!("Audio render stream stopped: {reason}"),
                    payload,
                )
            };
            let _ = local_events_tx.send(event);
        }
        Message::AudioStreamError { stream_id, message } => {
            let event = {
                let mut state = state.write().await;
                state.local_controls.audio_stream_state.last_error = Some(message.clone());
                let mut payload = BTreeMap::new();
                payload.insert("stream_id".to_string(), stream_id.to_string());
                payload.insert("error".to_string(), message.clone());
                record_audio_diagnostic_event(
                    &mut state,
                    "render_error",
                    format!("Remote audio stream error: {message}"),
                    payload,
                )
            };
            let _ = local_events_tx.send(event);
        }
        Message::UsbDeviceAttached { device } => {
            let accept_usb_advertisements = {
                let state = state.read().await;
                state.features.usb_advertising_enabled()
            };
            if !accept_usb_advertisements {
                return;
            }
            let event = {
                let mut state = state.write().await;
                upsert_remote_usb_device(&mut state, from, device)
            };
            let _ = local_events_tx.send(event);
        }
        Message::UsbDeviceDetached { bus_id, reason } => {
            let accept_usb_advertisements = {
                let state = state.read().await;
                state.features.usb_advertising_enabled()
            };
            if !accept_usb_advertisements {
                return;
            }
            let event = {
                let mut state = state.write().await;
                remove_remote_usb_device(&mut state, from, &bus_id, &reason)
            };
            let _ = local_events_tx.send(event);
        }
        Message::UsbTransfer { transfer } => {
            let usb_enabled = {
                let state = state.read().await;
                state.features.usb_forwarding_experimental
            };
            if !usb_enabled {
                send_usb_error(
                    network_manager,
                    from,
                    Some(transfer.bus_id),
                    "Experimental USB forwarding is disabled in settings.".to_string(),
                )
                .await;
                return;
            }
            let result = {
                let mut runtime = usb_runtime.lock().await;
                runtime.submit_transfer(&transfer)
            };
            match result {
                Ok(completion) => {
                    let message = Message::UsbTransferComplete {
                        transfer_id: completion.transfer_id,
                        bus_id: completion.bus_id,
                        status: completion.status,
                        transfer_status: completion.transfer_status,
                        endpoint_address: completion.endpoint_address,
                        transfer_kind: completion.transfer_kind,
                        actual_length: completion.actual_length,
                        data: completion.data,
                        iso_packets: Vec::new(),
                    };
                    if let Err(error) = network_manager.lock().await.send_to(&from, message).await {
                        tracing::warn!(
                            "Failed to send USB transfer completion to {}: {}",
                            from,
                            error
                        );
                    }
                }
                Err(error) => {
                    send_usb_error(
                        network_manager,
                        from,
                        Some(transfer.bus_id),
                        error.to_string(),
                    )
                    .await;
                }
            }
        }
        Message::UsbTransferComplete {
            transfer_id,
            bus_id,
            status,
            transfer_status,
            actual_length,
            data,
            ..
        } => {
            let pending = {
                let mut state = state.write().await;
                state.pending_usb_transfers.remove(&transfer_id)
            };
            if let Some(pending) = pending {
                if pending.target == from {
                    let probe_status =
                        if status == 0 && matches!(transfer_status, UsbTransferStatus::Completed) {
                            UsbDescriptorProbeStatus::Success
                        } else {
                            UsbDescriptorProbeStatus::TransferFailed
                        };
                    let result = usb_probe_result(
                        probe_status,
                        format!(
                            "USB descriptor probe completed with status {} ({:?}).",
                            status, transfer_status
                        ),
                        from,
                        bus_id.clone(),
                        pending.request_id,
                        pending.transfer_id,
                        Some(pending.session_id),
                        pending.started_at_ms,
                        data,
                        actual_length,
                    );
                    let _ = pending.result_tx.send(result);
                    let release = Message::UsbDeviceRelease {
                        session_id: pending.session_id,
                        bus_id,
                        reason: "descriptor_probe_complete".to_string(),
                    };
                    if let Err(error) = network_manager.lock().await.send_to(&from, release).await {
                        tracing::debug!(
                            "Failed to release USB descriptor probe session: {}",
                            error
                        );
                    }
                }
            } else {
                tracing::debug!(
                    "Received experimental USB transfer completion {} from {} for {} with status {}",
                    transfer_id,
                    from,
                    bus_id,
                    status
                );
            }
        }
        Message::UsbForwardingError { bus_id, message } => {
            complete_pending_usb_error(state, local_events_tx, from, bus_id, message).await;
        }
        Message::UsbDeviceClaimRequest { request } => {
            let usb_enabled = {
                let state = state.read().await;
                state.features.usb_forwarding_experimental
            };
            if !usb_enabled {
                send_usb_error(
                    network_manager,
                    from,
                    Some(request.bus_id),
                    "Experimental USB forwarding is disabled in settings.".to_string(),
                )
                .await;
                return;
            }
            let response = {
                let mut runtime = usb_runtime.lock().await;
                runtime.claim_device(request)
            };
            let flow = if response.accepted {
                let runtime = usb_runtime.lock().await;
                Some(runtime.flow_control(response.bus_id.clone(), response.session_id))
            } else {
                None
            };
            let accepted = response.accepted;
            let bus_id = response.bus_id.clone();
            if let Err(error) = network_manager
                .lock()
                .await
                .send_to(&from, Message::UsbDeviceClaimResponse { response })
                .await
            {
                tracing::warn!("Failed to send USB claim response to {}: {}", from, error);
            }
            if let Some(flow) = flow {
                if let Err(error) = network_manager
                    .lock()
                    .await
                    .send_to(&from, Message::UsbFlowControl { flow })
                    .await
                {
                    tracing::warn!("Failed to send USB flow control to {}: {}", from, error);
                }
            }
            tracing::debug!(
                "Processed experimental USB claim request from {} for {} accepted={}",
                from,
                bus_id,
                accepted
            );
        }
        Message::UsbDeviceClaimResponse { response } => {
            let action = {
                let mut state = state.write().await;
                let pending = state.pending_usb_claims.remove(&response.request_id);
                match pending {
                    Some(pending) if pending.target == from => {
                        if response.accepted {
                            match response.session_id {
                                Some(session_id) => {
                                    let transfer = usb_device_descriptor_probe_transfer(
                                        pending.transfer_id,
                                        response.bus_id.clone(),
                                        session_id,
                                    );
                                    state.pending_usb_transfers.insert(
                                        pending.transfer_id,
                                        PendingUsbTransfer {
                                            target: pending.target,
                                            bus_id: pending.bus_id,
                                            request_id: pending.request_id,
                                            transfer_id: pending.transfer_id,
                                            session_id,
                                            started_at_ms: pending.started_at_ms,
                                            result_tx: pending.result_tx,
                                        },
                                    );
                                    Some((pending.target, transfer, pending.transfer_id))
                                }
                                None => {
                                    let result = usb_probe_result(
                                        UsbDescriptorProbeStatus::ClaimRejected,
                                        "USB claim accepted without a session id.".to_string(),
                                        pending.target,
                                        pending.bus_id,
                                        pending.request_id,
                                        pending.transfer_id,
                                        None,
                                        pending.started_at_ms,
                                        Vec::new(),
                                        None,
                                    );
                                    let _ = pending.result_tx.send(result);
                                    None
                                }
                            }
                        } else {
                            let result = usb_probe_result(
                                UsbDescriptorProbeStatus::ClaimRejected,
                                response.message.unwrap_or_else(|| {
                                    "USB device claim was rejected.".to_string()
                                }),
                                pending.target,
                                pending.bus_id,
                                pending.request_id,
                                pending.transfer_id,
                                None,
                                pending.started_at_ms,
                                Vec::new(),
                                None,
                            );
                            let _ = pending.result_tx.send(result);
                            None
                        }
                    }
                    Some(pending) => {
                        state.pending_usb_claims.insert(pending.request_id, pending);
                        None
                    }
                    None => None,
                }
            };
            if let Some((target, transfer, transfer_id)) = action {
                let send_result = {
                    let mut manager = network_manager.lock().await;
                    manager
                        .send_to(&target, Message::UsbTransfer { transfer })
                        .await
                };
                if let Err(error) = send_result {
                    let pending = {
                        let mut state = state.write().await;
                        state.pending_usb_transfers.remove(&transfer_id)
                    };
                    if let Some(pending) = pending {
                        let result = usb_probe_result(
                            UsbDescriptorProbeStatus::Failed,
                            error.to_string(),
                            target,
                            pending.bus_id,
                            pending.request_id,
                            pending.transfer_id,
                            Some(pending.session_id),
                            pending.started_at_ms,
                            Vec::new(),
                            None,
                        );
                        let _ = pending.result_tx.send(result);
                    }
                }
            }
        }
        Message::UsbDeviceRelease {
            session_id,
            bus_id,
            reason,
        } => {
            let usb_enabled = {
                let state = state.read().await;
                state.features.usb_forwarding_experimental
            };
            if !usb_enabled {
                send_usb_error(
                    network_manager,
                    from,
                    Some(bus_id),
                    "Experimental USB forwarding is disabled in settings.".to_string(),
                )
                .await;
                return;
            }
            let result = {
                let mut runtime = usb_runtime.lock().await;
                runtime.release_device(session_id)
            };
            match result {
                Ok(()) => tracing::debug!(
                    "Released experimental USB session {} for {} from {}: {}",
                    session_id,
                    bus_id,
                    from,
                    reason
                ),
                Err(error) => {
                    send_usb_error(network_manager, from, Some(bus_id), error.to_string()).await;
                }
            }
        }
        Message::UsbDeviceReset {
            session_id,
            bus_id,
            reset_kind,
        } => {
            let usb_enabled = {
                let state = state.read().await;
                state.features.usb_forwarding_experimental
            };
            if !usb_enabled {
                send_usb_error(
                    network_manager,
                    from,
                    Some(bus_id),
                    "Experimental USB forwarding is disabled in settings.".to_string(),
                )
                .await;
                return;
            }
            let result = {
                let mut runtime = usb_runtime.lock().await;
                runtime.reset_device(session_id, &bus_id, reset_kind)
            };
            if let Err(error) = result {
                send_usb_error(network_manager, from, Some(bus_id), error.to_string()).await;
            }
        }
        Message::UsbTransferCancel {
            transfer_id,
            bus_id,
            reason,
        } => {
            let usb_enabled = {
                let state = state.read().await;
                state.features.usb_forwarding_experimental
            };
            if !usb_enabled {
                send_usb_error(
                    network_manager,
                    from,
                    Some(bus_id),
                    "Experimental USB forwarding is disabled in settings.".to_string(),
                )
                .await;
                return;
            }
            let result = {
                let mut runtime = usb_runtime.lock().await;
                runtime.cancel_transfer(transfer_id, &bus_id)
            };
            match result {
                Ok(()) => tracing::debug!(
                    "Cancelled experimental USB transfer {} from {} for {}: {}",
                    transfer_id,
                    from,
                    bus_id,
                    reason
                ),
                Err(error) => {
                    send_usb_error(network_manager, from, Some(bus_id), error.to_string()).await;
                }
            }
        }
        Message::UsbFlowControl { flow } => {
            tracing::debug!(
                "Received experimental USB flow control from {} for {}: {} bytes, {} transfers",
                from,
                flow.bus_id,
                flow.available_window_bytes,
                flow.max_in_flight_transfers
            );
        }
        other => tracing::debug!(
            "Ignoring unsupported control-plane message from {}: {:?}",
            from,
            other
        ),
    }
}

async fn send_usb_error(
    network_manager: &Arc<Mutex<NetworkManager>>,
    target: DeviceId,
    bus_id: Option<String>,
    message: String,
) {
    let result = {
        let mut manager = network_manager.lock().await;
        manager
            .send_to(&target, Message::UsbForwardingError { bus_id, message })
            .await
    };
    if let Err(error) = result {
        tracing::warn!(
            "Failed to send USB forwarding error to {}: {}",
            target,
            error
        );
    }
}

async fn advertise_usb_devices_to(
    network_manager: &Arc<Mutex<NetworkManager>>,
    usb_runtime: &UsbHostRuntime,
    target: DeviceId,
) {
    let devices = {
        let runtime = usb_runtime.lock().await;
        runtime.enumerate_devices()
    };
    let devices = match devices {
        Ok(devices) => devices,
        Err(error) => {
            tracing::debug!("USB advertisement enumeration failed: {}", error);
            return;
        }
    };

    for device in devices {
        let result = {
            let mut manager = network_manager.lock().await;
            manager
                .send_to(&target, Message::UsbDeviceAttached { device })
                .await
        };
        if let Err(error) = result {
            tracing::debug!("Failed to advertise USB device to {}: {}", target, error);
            break;
        }
    }
}

async fn complete_pending_usb_error(
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    from: DeviceId,
    bus_id: Option<String>,
    message: String,
) {
    let (claim, transfer, event) = {
        let mut state = state.write().await;
        let claim_key = state
            .pending_usb_claims
            .iter()
            .find(|(_, pending)| {
                pending.target == from
                    && bus_id
                        .as_deref()
                        .map(|bus_id| pending.bus_id == bus_id)
                        .unwrap_or(true)
            })
            .map(|(key, _)| *key);
        let transfer_key = state
            .pending_usb_transfers
            .iter()
            .find(|(_, pending)| {
                pending.target == from
                    && bus_id
                        .as_deref()
                        .map(|bus_id| pending.bus_id == bus_id)
                        .unwrap_or(true)
            })
            .map(|(key, _)| *key);
        let claim = claim_key.and_then(|key| state.pending_usb_claims.remove(&key));
        let transfer = transfer_key.and_then(|key| state.pending_usb_transfers.remove(&key));
        let mut payload = BTreeMap::new();
        if let Some(bus_id) = &bus_id {
            payload.insert("bus_id".to_string(), bus_id.clone());
        }
        payload.insert("error".to_string(), message.clone());
        let event = record_usb_diagnostic_event(
            &mut state,
            Some(from),
            "usb_forwarding_error",
            format!(
                "USB forwarding error from {}: {message}",
                short_device_id(from)
            ),
            payload,
        );
        (claim, transfer, event)
    };
    let _ = local_events_tx.send(event);

    if let Some(pending) = claim {
        let result = usb_probe_result(
            UsbDescriptorProbeStatus::ClaimRejected,
            message.clone(),
            pending.target,
            pending.bus_id,
            pending.request_id,
            pending.transfer_id,
            None,
            pending.started_at_ms,
            Vec::new(),
            None,
        );
        let _ = pending.result_tx.send(result);
    }
    if let Some(pending) = transfer {
        let result = usb_probe_result(
            UsbDescriptorProbeStatus::TransferFailed,
            message,
            pending.target,
            pending.bus_id,
            pending.request_id,
            pending.transfer_id,
            Some(pending.session_id),
            pending.started_at_ms,
            Vec::new(),
            None,
        );
        let _ = pending.result_tx.send(result);
    }
}

async fn record_injected_test_event(
    state: &Arc<RwLock<DaemonState>>,
    kind: LocalInputTestKind,
    payload: BTreeMap<String, String>,
) -> LocalInputDiagnosticEvent {
    let mut state = state.write().await;
    let sequence = state.local_controls.sequence.saturating_add(1);
    state.local_controls.sequence = sequence;
    let timestamp_ms = timestamp_ms_now();
    let (device_kind, event_kind, summary) = match kind {
        LocalInputTestKind::KeyboardShift => (
            LocalInputDeviceKind::Keyboard,
            "injected_test".to_string(),
            "Injected Shift key test".to_string(),
        ),
        LocalInputTestKind::MouseMove => (
            LocalInputDeviceKind::Mouse,
            "injected_test".to_string(),
            "Injected mouse move test".to_string(),
        ),
        LocalInputTestKind::VirtualGamepadStatus => (
            LocalInputDeviceKind::Gamepad,
            "virtual_gamepad_status".to_string(),
            "Virtual gamepad injection is not implemented".to_string(),
        ),
    };
    state.arm_injected_loopback(device_kind, timestamp_ms);
    let event = LocalInputDiagnosticEvent {
        sequence,
        timestamp_ms,
        device_kind,
        event_kind,
        summary,
        device_id: Some("rshare-injection-test".to_string()),
        device_instance_id: None,
        capture_path: Some("daemon-injection-test".to_string()),
        source: LocalInputEventSource::InjectedLoopback,
        payload,
    };
    push_recent_local_event(&mut state.local_controls, event.clone());
    event
}

async fn set_audio_output_volume(
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    endpoint_id: String,
    volume_percent: u8,
) -> DaemonResponse {
    #[cfg(windows)]
    let result = rshare_platform::windows::set_audio_output_volume(&endpoint_id, volume_percent);
    #[cfg(not(windows))]
    let result: anyhow::Result<()> = Err(anyhow::anyhow!(
        "Audio output volume control is only implemented on Windows in this build"
    ));

    match result {
        Ok(()) => {
            let event = {
                let mut state = state.write().await;
                state.refresh_local_controls_platform();
                let mut payload = BTreeMap::new();
                payload.insert("endpoint_id".to_string(), endpoint_id);
                payload.insert(
                    "volume_percent".to_string(),
                    volume_percent.min(100).to_string(),
                );
                record_audio_diagnostic_event(
                    &mut state,
                    "output_volume",
                    "Audio output volume changed",
                    payload,
                )
            };
            let _ = local_events_tx.send(event);
            DaemonResponse::Ack
        }
        Err(error) => DaemonResponse::Error(error.to_string()),
    }
}

async fn set_audio_output_mute(
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    endpoint_id: String,
    muted: bool,
) -> DaemonResponse {
    #[cfg(windows)]
    let result = rshare_platform::windows::set_audio_output_mute(&endpoint_id, muted);
    #[cfg(not(windows))]
    let result: anyhow::Result<()> = Err(anyhow::anyhow!(
        "Audio output mute control is only implemented on Windows in this build"
    ));

    match result {
        Ok(()) => {
            let event = {
                let mut state = state.write().await;
                state.refresh_local_controls_platform();
                let mut payload = BTreeMap::new();
                payload.insert("endpoint_id".to_string(), endpoint_id);
                payload.insert("muted".to_string(), muted.to_string());
                record_audio_diagnostic_event(
                    &mut state,
                    "output_mute",
                    "Audio output mute changed",
                    payload,
                )
            };
            let _ = local_events_tx.send(event);
            DaemonResponse::Ack
        }
        Err(error) => DaemonResponse::Error(error.to_string()),
    }
}

async fn set_default_audio_output(
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    endpoint_id: String,
) -> DaemonResponse {
    #[cfg(windows)]
    let result = rshare_platform::windows::set_default_audio_output(&endpoint_id);
    #[cfg(not(windows))]
    let result: anyhow::Result<()> = Err(anyhow::anyhow!(
        "Default audio output switching is only implemented on Windows in this build"
    ));

    match result {
        Ok(()) => {
            let event = {
                let mut state = state.write().await;
                state.refresh_local_controls_platform();
                let mut payload = BTreeMap::new();
                payload.insert("endpoint_id".to_string(), endpoint_id);
                record_audio_diagnostic_event(
                    &mut state,
                    "default_output",
                    "Default audio output changed",
                    payload,
                )
            };
            let _ = local_events_tx.send(event);
            DaemonResponse::Ack
        }
        Err(error) => DaemonResponse::Error(error.to_string()),
    }
}

async fn run_local_audio_capture_status_loop(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<audio_runtime::CapturedAudioFrame>,
    state: Arc<RwLock<DaemonState>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    source: LocalAudioCaptureSource,
    endpoint_id: Option<String>,
) {
    let mut last_event_ms = 0;
    let mut frames_seen = 0u64;
    while let Some(captured) = rx.recv().await {
        frames_seen = frames_seen.saturating_add(1);
        let now = timestamp_ms_now();
        let event = {
            let mut state = state.write().await;
            state.local_controls.audio_capture_state.level_peak = captured.level_peak;
            state.local_controls.audio_capture_state.level_rms = captured.level_rms;
            state.local_controls.audio_capture_state.sample_rate =
                Some(captured.frame.format.sample_rate);
            state.local_controls.audio_capture_state.channel_count =
                Some(captured.frame.format.channels as u32);

            if now.saturating_sub(last_event_ms) < 250 {
                None
            } else {
                last_event_ms = now;
                let mut payload = BTreeMap::new();
                payload.insert("source".to_string(), audio_source_label(source).to_string());
                payload.insert("frames_seen".to_string(), frames_seen.to_string());
                payload.insert("level_peak".to_string(), captured.level_peak.to_string());
                payload.insert("level_rms".to_string(), captured.level_rms.to_string());
                payload.insert(
                    "format".to_string(),
                    audio_format_label(&captured.frame.format),
                );
                if let Some(endpoint_id) = endpoint_id.as_ref() {
                    payload.insert("endpoint_id".to_string(), endpoint_id.clone());
                }
                Some(record_audio_diagnostic_event(
                    &mut state,
                    "capture_level",
                    format!(
                        "Audio capture level peak={} rms={}",
                        captured.level_peak, captured.level_rms
                    ),
                    payload,
                ))
            }
        };
        if let Some(event) = event {
            let _ = local_events_tx.send(event);
        }
    }
}

async fn run_audio_forwarding_loop(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<audio_runtime::CapturedAudioFrame>,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    target: DeviceId,
    source: LocalAudioCaptureSource,
    endpoint_id: Option<String>,
) {
    let mut last_event_ms = 0;
    while let Some(captured) = rx.recv().await {
        let frame_sequence = captured.frame.sequence;
        let send_result = {
            let mut manager = network_manager.lock().await;
            manager
                .send_to(
                    &target,
                    Message::AudioFrame {
                        frame: captured.frame.clone(),
                    },
                )
                .await
        };

        let now = timestamp_ms_now();
        let event = {
            let mut state = state.write().await;
            state.local_controls.audio_capture_state.level_peak = captured.level_peak;
            state.local_controls.audio_capture_state.level_rms = captured.level_rms;
            state.local_controls.audio_capture_state.sample_rate =
                Some(captured.frame.format.sample_rate);
            state.local_controls.audio_capture_state.channel_count =
                Some(captured.frame.format.channels as u32);
            state.local_controls.audio_stream_state.frames_sent = frame_sequence;

            if let Err(error) = send_result {
                state.local_controls.audio_stream_state.last_error = Some(error.to_string());
                let mut payload = BTreeMap::new();
                payload.insert("target_device_id".to_string(), target.to_string());
                payload.insert("error".to_string(), error.to_string());
                Some(record_audio_diagnostic_event(
                    &mut state,
                    "forwarding_error",
                    format!("Audio frame forwarding failed: {error}"),
                    payload,
                ))
            } else if now.saturating_sub(last_event_ms) >= 500 {
                last_event_ms = now;
                let mut payload = BTreeMap::new();
                payload.insert("source".to_string(), audio_source_label(source).to_string());
                payload.insert("target_device_id".to_string(), target.to_string());
                payload.insert("frames_sent".to_string(), frame_sequence.to_string());
                payload.insert("level_peak".to_string(), captured.level_peak.to_string());
                payload.insert("level_rms".to_string(), captured.level_rms.to_string());
                payload.insert(
                    "format".to_string(),
                    audio_format_label(&captured.frame.format),
                );
                if let Some(endpoint_id) = endpoint_id.as_ref() {
                    payload.insert("endpoint_id".to_string(), endpoint_id.clone());
                }
                Some(record_audio_diagnostic_event(
                    &mut state,
                    "forwarding_level",
                    format!(
                        "Audio forwarding {} frames to {}",
                        frame_sequence,
                        short_device_id(target)
                    ),
                    payload,
                ))
            } else {
                None
            }
        };

        if let Some(event) = event {
            let _ = local_events_tx.send(event);
        }
    }
}

async fn start_audio_capture(
    audio_runtime: &audio_runtime::AudioRuntimeHandle,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    source: LocalAudioCaptureSource,
    endpoint_id: Option<String>,
) -> DaemonResponse {
    let audio_capture_enabled = {
        let state = state.read().await;
        state.features.audio_capture
    };
    if !audio_capture_enabled {
        return DaemonResponse::Error("Audio capture is disabled in settings.".to_string());
    }

    let stream_id = DeviceId::new_v4();
    let endpoint_name = {
        let state = state.read().await;
        audio_input_name_for_endpoint(&state, endpoint_id.as_deref())
    };
    let capture = audio_runtime.start_capture(source, endpoint_name.as_deref(), stream_id);
    let started = match capture {
        Ok(capture) => capture,
        Err(error) => {
            let event = {
                let mut state = state.write().await;
                state.local_controls.audio_capture_state.status = LocalAudioCaptureStatus::Error;
                state.local_controls.audio_capture_state.last_error = Some(error.to_string());
                let mut payload = BTreeMap::new();
                payload.insert("source".to_string(), audio_source_label(source).to_string());
                payload.insert("error".to_string(), error.to_string());
                record_audio_diagnostic_event(
                    &mut state,
                    "capture_error",
                    format!("Audio capture failed: {error}"),
                    payload,
                )
            };
            let _ = local_events_tx.send(event);
            return DaemonResponse::Error(error.to_string());
        }
    };
    let format = started.format.clone();
    tokio::spawn(run_local_audio_capture_status_loop(
        started.rx,
        state.clone(),
        local_events_tx.clone(),
        source,
        endpoint_id.clone(),
    ));

    let event = {
        let mut state = state.write().await;
        state.local_controls.audio_capture_state.status = LocalAudioCaptureStatus::CapturingLocal;
        state.local_controls.audio_capture_state.source = Some(source);
        state.local_controls.audio_capture_state.endpoint_id = endpoint_id.clone();
        state.local_controls.audio_capture_state.started_at_ms = Some(timestamp_ms_now());
        state.local_controls.audio_capture_state.sample_rate = Some(format.sample_rate);
        state.local_controls.audio_capture_state.channel_count = Some(format.channels as u32);
        state.local_controls.audio_capture_state.last_error = None;
        state.local_controls.audio_stream_state.active = false;
        let mut payload = BTreeMap::new();
        payload.insert("source".to_string(), audio_source_label(source).to_string());
        payload.insert("stream_id".to_string(), stream_id.to_string());
        payload.insert("format".to_string(), audio_format_label(&format));
        if let Some(endpoint_id) = endpoint_id {
            payload.insert("endpoint_id".to_string(), endpoint_id);
        }
        record_audio_diagnostic_event(
            &mut state,
            "capture_start",
            "Audio local capture state started",
            payload,
        )
    };
    let _ = local_events_tx.send(event);
    DaemonResponse::Ack
}

async fn stop_audio_capture(
    audio_runtime: &audio_runtime::AudioRuntimeHandle,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
) -> DaemonResponse {
    audio_runtime.stop_capture();
    let event = {
        let mut state = state.write().await;
        state.local_controls.audio_capture_state.status = LocalAudioCaptureStatus::Idle;
        state.local_controls.audio_capture_state.source = None;
        state.local_controls.audio_capture_state.endpoint_id = None;
        state.local_controls.audio_capture_state.started_at_ms = None;
        state.local_controls.audio_stream_state.active = false;
        state.local_controls.audio_stream_state.target_device_id = None;
        state.local_controls.audio_stream_state.stream_id = None;
        record_audio_diagnostic_event(
            &mut state,
            "capture_stop",
            "Audio local capture state stopped",
            BTreeMap::new(),
        )
    };
    let _ = local_events_tx.send(event);
    DaemonResponse::Ack
}

async fn start_audio_forwarding(
    audio_runtime: &audio_runtime::AudioRuntimeHandle,
    state: &Arc<RwLock<DaemonState>>,
    network_manager: &Arc<Mutex<NetworkManager>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    source: LocalAudioCaptureSource,
    endpoint_id: Option<String>,
) -> DaemonResponse {
    let features = {
        let state = state.read().await;
        state.features.clone()
    };
    if !features.audio_capture {
        return DaemonResponse::Error("Audio capture is disabled in settings.".to_string());
    }
    if !features.audio_forwarding {
        return DaemonResponse::Error("Audio forwarding is disabled in settings.".to_string());
    }

    let (target, source_device_id) = {
        let state = state.read().await;
        (
            state
                .session
                .active_target()
                .filter(|target| is_device_connected(&state, *target)),
            state.status.device_id,
        )
    };
    let Some(target) = target else {
        let mut state = state.write().await;
        state.local_controls.audio_capture_state.status = LocalAudioCaptureStatus::Error;
        state.local_controls.audio_capture_state.last_error =
            Some("No connected RemoteActive target for audio forwarding.".to_string());
        state.local_controls.audio_stream_state.active = false;
        state.local_controls.audio_stream_state.last_error =
            Some("No connected RemoteActive target for audio forwarding.".to_string());
        return DaemonResponse::Error(
            "No connected RemoteActive target for audio forwarding.".to_string(),
        );
    };

    let stream_id = DeviceId::new_v4();
    let endpoint_name = {
        let state = state.read().await;
        audio_input_name_for_endpoint(&state, endpoint_id.as_deref())
    };
    let capture = audio_runtime.start_capture(source, endpoint_name.as_deref(), stream_id);
    let started = match capture {
        Ok(capture) => capture,
        Err(error) => {
            let event = {
                let mut state = state.write().await;
                state.local_controls.audio_capture_state.status = LocalAudioCaptureStatus::Error;
                state.local_controls.audio_capture_state.last_error = Some(error.to_string());
                state.local_controls.audio_stream_state.active = false;
                state.local_controls.audio_stream_state.last_error = Some(error.to_string());
                let mut payload = BTreeMap::new();
                payload.insert("source".to_string(), audio_source_label(source).to_string());
                payload.insert("target_device_id".to_string(), target.to_string());
                payload.insert("error".to_string(), error.to_string());
                record_audio_diagnostic_event(
                    &mut state,
                    "forwarding_error",
                    format!("Audio forwarding capture failed: {error}"),
                    payload,
                )
            };
            let _ = local_events_tx.send(event);
            return DaemonResponse::Error(error.to_string());
        }
    };
    let format = started.format.clone();

    let start_result = {
        let mut manager = network_manager.lock().await;
        manager
            .send_to(
                &target,
                Message::AudioStreamStart {
                    stream_id,
                    source_device_id,
                    format: format.clone(),
                },
            )
            .await
    };
    if let Err(error) = start_result {
        audio_runtime.stop_capture();
        let event = {
            let mut state = state.write().await;
            state.local_controls.audio_capture_state.status = LocalAudioCaptureStatus::Error;
            state.local_controls.audio_capture_state.last_error = Some(error.to_string());
            state.local_controls.audio_stream_state.active = false;
            state.local_controls.audio_stream_state.last_error = Some(error.to_string());
            let mut payload = BTreeMap::new();
            payload.insert("target_device_id".to_string(), target.to_string());
            payload.insert("stream_id".to_string(), stream_id.to_string());
            payload.insert("error".to_string(), error.to_string());
            record_audio_diagnostic_event(
                &mut state,
                "forwarding_error",
                format!("Audio stream start failed: {error}"),
                payload,
            )
        };
        let _ = local_events_tx.send(event);
        return DaemonResponse::Error(error.to_string());
    }

    tokio::spawn(run_audio_forwarding_loop(
        started.rx,
        state.clone(),
        network_manager.clone(),
        local_events_tx.clone(),
        target,
        source,
        endpoint_id.clone(),
    ));

    let event = {
        let mut state = state.write().await;
        state.local_controls.audio_capture_state.status = LocalAudioCaptureStatus::ForwardingRemote;
        state.local_controls.audio_capture_state.source = Some(source);
        state.local_controls.audio_capture_state.endpoint_id = endpoint_id.clone();
        state.local_controls.audio_capture_state.started_at_ms = Some(timestamp_ms_now());
        state.local_controls.audio_capture_state.sample_rate = Some(format.sample_rate);
        state.local_controls.audio_capture_state.channel_count = Some(format.channels as u32);
        state.local_controls.audio_capture_state.last_error = None;
        state.local_controls.audio_stream_state.active = true;
        state.local_controls.audio_stream_state.target_device_id = Some(target.to_string());
        state.local_controls.audio_stream_state.stream_id = Some(stream_id.to_string());
        state.local_controls.audio_stream_state.frames_sent = 0;
        state.local_controls.audio_stream_state.latency_ms = Some(0);
        state.local_controls.audio_stream_state.last_error = None;
        let mut payload = BTreeMap::new();
        payload.insert("source".to_string(), audio_source_label(source).to_string());
        payload.insert("target_device_id".to_string(), target.to_string());
        payload.insert("stream_id".to_string(), stream_id.to_string());
        payload.insert("format".to_string(), audio_format_label(&format));
        if let Some(endpoint_id) = endpoint_id {
            payload.insert("endpoint_id".to_string(), endpoint_id);
        }
        record_audio_diagnostic_event(
            &mut state,
            "forwarding_start",
            "Audio forwarding state started",
            payload,
        )
    };
    let _ = local_events_tx.send(event);
    DaemonResponse::Ack
}

async fn stop_audio_forwarding(
    audio_runtime: &audio_runtime::AudioRuntimeHandle,
    state: &Arc<RwLock<DaemonState>>,
    network_manager: &Arc<Mutex<NetworkManager>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
) -> DaemonResponse {
    audio_runtime.stop_capture();
    let stream_target = {
        let state = state.read().await;
        state
            .local_controls
            .audio_stream_state
            .target_device_id
            .as_deref()
            .and_then(|target| DeviceId::parse_str(target).ok())
            .zip(
                state
                    .local_controls
                    .audio_stream_state
                    .stream_id
                    .as_deref()
                    .and_then(|stream_id| DeviceId::parse_str(stream_id).ok()),
            )
    };
    if let Some((target, stream_id)) = stream_target {
        let result = {
            let mut manager = network_manager.lock().await;
            manager
                .send_to(
                    &target,
                    Message::AudioStreamStop {
                        stream_id,
                        reason: "local stop".to_string(),
                    },
                )
                .await
        };
        if let Err(error) = result {
            tracing::debug!("Failed to send audio stream stop to {}: {}", target, error);
        }
    }

    let event = {
        let mut state = state.write().await;
        state.local_controls.audio_capture_state.status = LocalAudioCaptureStatus::Idle;
        state.local_controls.audio_stream_state.active = false;
        state.local_controls.audio_stream_state.target_device_id = None;
        state.local_controls.audio_stream_state.stream_id = None;
        record_audio_diagnostic_event(
            &mut state,
            "forwarding_stop",
            "Audio forwarding state stopped",
            BTreeMap::new(),
        )
    };
    let _ = local_events_tx.send(event);
    DaemonResponse::Ack
}

async fn run_audio_test(state: &Arc<RwLock<DaemonState>>) -> LocalAudioTestResult {
    let (input_count, output_count) = {
        let mut state = state.write().await;
        if !state.features.audio_capture && !state.features.audio_forwarding {
            return LocalAudioTestResult::failed(
                LocalAudioTestStatus::DeviceUnavailable,
                "Audio capture and forwarding are disabled in settings.",
            );
        }
        state.refresh_local_controls_platform();
        (
            state.local_controls.audio_inputs.len(),
            state.local_controls.audio_outputs.len(),
        )
    };

    if input_count == 0 && output_count == 0 {
        LocalAudioTestResult::failed(
            LocalAudioTestStatus::DeviceUnavailable,
            "No audio input or output endpoint is available.",
        )
    } else {
        LocalAudioTestResult::success(format!(
            "Audio endpoints available: {input_count} input, {output_count} output."
        ))
    }
}

#[cfg(windows)]
fn input_event_from_windows_driver_event(
    event: &rshare_platform::windows::WindowsDriverInputEvent,
) -> Option<InputEvent> {
    use rshare_platform::windows::{WindowsDriverDeviceKind, WindowsDriverEventKind};

    if let Some(input_event) = rshare_input::InputEvent::from_windows_driver_event(event.clone()) {
        return Some(input_event);
    }

    match (event.device_kind, event.event_kind) {
        (WindowsDriverDeviceKind::Keyboard, WindowsDriverEventKind::Synthetic) => {
            Some(InputEvent::key(
                rshare_input::KeyCode::Raw(event.value0 as u32),
                if event.value1 != 0 {
                    rshare_input::ButtonState::Pressed
                } else {
                    rshare_input::ButtonState::Released
                },
            ))
        }
        (WindowsDriverDeviceKind::Mouse, WindowsDriverEventKind::Synthetic) => {
            Some(InputEvent::mouse_move(event.value0, event.value1))
        }
        (WindowsDriverDeviceKind::Mouse, WindowsDriverEventKind::MouseButton) => {
            Some(InputEvent::mouse_button(
                rshare_input::MouseButton::from_code(event.value0 as u8),
                if event.value1 != 0 {
                    rshare_input::ButtonState::Pressed
                } else {
                    rshare_input::ButtonState::Released
                },
            ))
        }
        (WindowsDriverDeviceKind::Mouse, WindowsDriverEventKind::MouseWheel) => {
            Some(InputEvent::mouse_wheel(event.value0, event.value1))
        }
        _ => None,
    }
}

#[cfg(windows)]
fn input_event_from_windows_driver_event_with_pointer(
    event: &rshare_platform::windows::WindowsDriverInputEvent,
    current_x: i32,
    current_y: i32,
    display: &LocalDisplayState,
) -> Option<InputEvent> {
    use rshare_platform::windows::{WindowsDriverDeviceKind, WindowsDriverEventKind};

    const MOUSE_MOVE_ABSOLUTE: u32 = 0x0001;

    if event.device_kind == WindowsDriverDeviceKind::Mouse
        && event.event_kind == WindowsDriverEventKind::MouseMove
        && event.flags & MOUSE_MOVE_ABSOLUTE == 0
    {
        let (x, y) = clamp_windows_driver_mouse_delta(
            current_x,
            current_y,
            event.value0,
            event.value1,
            display,
        );
        return Some(InputEvent::mouse_move(x, y));
    }

    input_event_from_windows_driver_event(event)
}

#[cfg(windows)]
fn clamp_windows_driver_mouse_delta(
    current_x: i32,
    current_y: i32,
    delta_x: i32,
    delta_y: i32,
    display: &LocalDisplayState,
) -> (i32, i32) {
    let min_x = display.virtual_x;
    let min_y = display.virtual_y;
    let width = i32::try_from(display.layout_width.max(1)).unwrap_or(i32::MAX);
    let height = i32::try_from(display.layout_height.max(1)).unwrap_or(i32::MAX);
    let max_x = min_x.saturating_add(width.saturating_sub(1));
    let max_y = min_y.saturating_add(height.saturating_sub(1));

    (
        current_x.saturating_add(delta_x).clamp(min_x, max_x),
        current_y.saturating_add(delta_y).clamp(min_y, max_y),
    )
}

#[cfg(windows)]
fn local_source_from_windows_driver_event(
    source: rshare_platform::windows::WindowsDriverEventSource,
) -> LocalInputEventSource {
    match source {
        rshare_platform::windows::WindowsDriverEventSource::Hardware => {
            LocalInputEventSource::Hardware
        }
        rshare_platform::windows::WindowsDriverEventSource::InjectedLoopback => {
            LocalInputEventSource::InjectedLoopback
        }
        rshare_platform::windows::WindowsDriverEventSource::DriverTest => {
            LocalInputEventSource::DriverTest
        }
        rshare_platform::windows::WindowsDriverEventSource::VirtualDevice => {
            LocalInputEventSource::VirtualDevice
        }
    }
}

#[cfg(windows)]
fn resolve_windows_driver_local_source(
    recorded_source: LocalInputEventSource,
    driver_source: LocalInputEventSource,
) -> LocalInputEventSource {
    if matches!(recorded_source, LocalInputEventSource::InjectedLoopback) {
        recorded_source
    } else {
        driver_source
    }
}

#[cfg(windows)]
fn publish_windows_driver_event(
    producer: &SemanticInputProducer,
    driver_event: rshare_platform::windows::WindowsDriverInputEvent,
) {
    use rshare_platform::windows::{WindowsDriverDeviceKind, WindowsDriverEventKind};
    const MOUSE_MOVE_ABSOLUTE: u32 = 0x0001;
    let payload = if driver_event.device_kind == WindowsDriverDeviceKind::Mouse
        && driver_event.event_kind == WindowsDriverEventKind::MouseMove
        && driver_event.flags & MOUSE_MOVE_ABSOLUTE == 0
    {
        CapturedInputPayload::Continuous(ContinuousInput::Pointer(PointerSample::Relative {
            dx: driver_event.value0,
            dy: driver_event.value1,
            observed_x: None,
            observed_y: None,
        }))
    } else {
        let Some(event) = input_event_from_windows_driver_event(&driver_event) else {
            return;
        };
        CapturedInputPayload::from_input_event(event)
    };
    let origin = CaptureOrigin {
        source: CaptureSource::WindowsFilter,
        device_token: driver_event.device_id.parse().unwrap_or(0),
        instance_token: 0,
    };
    let _ = producer.try_push(producer.capture(origin, payload));
}

#[cfg(windows)]
#[derive(Debug)]
struct WindowsDriverCapture {
    running: Arc<std::sync::atomic::AtomicBool>,
    cancel: rshare_platform::windows::WindowsDriverEventStreamCancel,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
enum WindowsCaptureSelection<Driver, Hook> {
    Filter(Driver),
    Hook {
        listener: Hook,
        filter_error: Option<String>,
    },
}

#[cfg(windows)]
fn start_windows_capture_with_fallback<Driver, Hook, StartDriver, StartHook>(
    use_filter: bool,
    start_driver: StartDriver,
    start_hook: StartHook,
) -> Result<WindowsCaptureSelection<Driver, Hook>>
where
    StartDriver: FnOnce() -> Result<Driver>,
    StartHook: FnOnce() -> Result<Hook>,
{
    if use_filter {
        match start_driver() {
            Ok(driver) => return Ok(WindowsCaptureSelection::Filter(driver)),
            Err(error) => {
                let filter_error = error.to_string();
                let listener = start_hook()?;
                return Ok(WindowsCaptureSelection::Hook {
                    listener,
                    filter_error: Some(filter_error),
                });
            }
        }
    }
    Ok(WindowsCaptureSelection::Hook {
        listener: start_hook()?,
        filter_error: None,
    })
}

#[cfg(windows)]
impl WindowsDriverCapture {
    fn start(producer: SemanticInputProducer) -> Result<Self> {
        Self::start_with_opener(
            producer,
            rshare_platform::windows::WindowsDriverEventStream::open_filter,
        )
    }

    fn start_with_opener<Open>(producer: SemanticInputProducer, open: Open) -> Result<Self>
    where
        Open: FnOnce() -> Result<(
                rshare_platform::windows::WindowsDriverEventStream,
                rshare_platform::windows::WindowsDriverEventStreamCancel,
            )> + Send
            + 'static,
    {
        Self::start_with_opener_timeout(producer, Duration::from_secs(2), open)
    }

    fn start_with_opener_timeout<Open>(
        producer: SemanticInputProducer,
        startup_timeout: Duration,
        open: Open,
    ) -> Result<Self>
    where
        Open: FnOnce() -> Result<(
                rshare_platform::windows::WindowsDriverEventStream,
                rshare_platform::windows::WindowsDriverEventStreamCancel,
            )> + Send
            + 'static,
    {
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker_running = running.clone();
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(0);
        let thread = std::thread::Builder::new()
            .name("rshare-windows-filter-capture".to_owned())
            .spawn(move || {
                let (mut stream, cancel) = match open() {
                    Ok(stream) => stream,
                    Err(error) => {
                        let _ = startup_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                if startup_tx.send(Ok(cancel)).is_err() {
                    return;
                }
                while worker_running.load(std::sync::atomic::Ordering::Acquire) {
                    match stream
                        .wait_event()
                        .map(rshare_input::backend::adapt_windows_filter_capture_event)
                    {
                        Ok(rshare_input::backend::WindowsFilterCaptureOutput::Input(event)) => {
                            publish_windows_driver_event(&producer, event)
                        }
                        Ok(rshare_input::backend::WindowsFilterCaptureOutput::Fault(fault)) => {
                            producer.report_fault(fault);
                        }
                        Err(rshare_platform::windows::DriverWaitError::Cancelled)
                            if !worker_running.load(std::sync::atomic::Ordering::Acquire) =>
                        {
                            break;
                        }
                        Err(error) => {
                            tracing::warn!("RShare Windows driver event wait failed: {error}");
                            break;
                        }
                    }
                }
            })
            .map_err(|error| anyhow::anyhow!("failed to spawn Windows filter capture: {error}"))?;
        match startup_rx.recv_timeout(startup_timeout) {
            Ok(Ok(cancel)) => Ok(Self {
                running,
                cancel,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(anyhow::anyhow!(
                    "failed to open Windows filter capture: {error}"
                ))
            }
            Err(error) => {
                running.store(false, std::sync::atomic::Ordering::Release);
                drop(thread);
                Err(anyhow::anyhow!(
                    "Windows filter capture startup handshake failed: {error}"
                ))
            }
        }
    }

    fn stop(&mut self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Release);
        if let Err(error) = self.cancel.cancel() {
            tracing::warn!("failed to cancel Windows filter capture wait: {error}");
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsDriverCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn get_log_file_path() -> PathBuf {
    if let Some(config_dir) = dirs::config_dir().map(|path| path.join("rshare")) {
        if fs::create_dir_all(&config_dir).is_ok() {
            let log_path = config_dir.join("rshare-daemon.log");
            if fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .is_ok()
            {
                return log_path;
            }
        }
    }

    let fallback_dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("target");
    let _ = fs::create_dir_all(&fallback_dir);
    fallback_dir.join("rshare-daemon.log")
}

/// Check if Evdev input devices are available without actually opening/grabbing them.
/// This is used for health checking during backend selection.
#[cfg(target_os = "linux")]
fn check_evdev_devices_available() -> BackendHealth {
    use std::path::Path;

    let input_dir = Path::new("/dev/input");
    if !input_dir.exists() {
        tracing::warn!("/dev/input directory not found");
        return BackendHealth::Degraded {
            reason: BackendFailureReason::Unavailable,
        };
    }

    // Check if there are any event devices
    let mut device_count = 0;
    if let Ok(entries) = input_dir.read_dir() {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("event"))
                .unwrap_or(false)
            {
                // Try to open the device read-only to check accessibility
                match std::fs::File::open(&path) {
                    Ok(_file) => {
                        device_count += 1;
                        // Check if device is readable (has input group permission)
                        // We don't actually query the device to avoid triggering udev rules
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        tracing::warn!("Permission denied accessing {:?}", path);
                        return BackendHealth::Degraded {
                            reason: BackendFailureReason::PermissionDenied,
                        };
                    }
                    Err(_) => {
                        // Device not accessible, skip it
                    }
                }
            }
        }
    }

    if device_count == 0 {
        tracing::warn!("No accessible input devices found in /dev/input");
        return BackendHealth::Degraded {
            reason: BackendFailureReason::Unavailable,
        };
    }

    tracing::info!(
        "Found {} accessible input devices in /dev/input",
        device_count
    );
    BackendHealth::Healthy
}

/// Try to start Evdev capture backend for kernel-level input capture on Linux.
/// Returns Ok(task handle) if Evdev capture is available and started successfully.
#[cfg(target_os = "linux")]
fn try_start_evdev_capture(producer: SemanticInputProducer) -> Result<tokio::task::JoinHandle<()>> {
    use rshare_platform::EvdevDriverEvent;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    let (started_tx, started_rx) = mpsc::sync_channel(1);

    // Spawn a thread to read events from Evdev and send them to the channel
    let handle = tokio::task::spawn_blocking(move || {
        // Create an EvdevInputListener to capture events
        let mut listener = rshare_platform::EvdevInputListener::new();

        // Callback to convert EvdevDriverEvent to InputEvent and send to channel
        let callback = move |evdev_event: EvdevDriverEvent| {
            if !running_clone.load(Ordering::Relaxed) {
                return;
            }

            // Log the raw evdev event for debugging
            tracing::debug!("Evdev event: {:?}", evdev_event);

            let Some(input_event) = input_event_from_evdev_driver_event(evdev_event) else {
                return;
            };
            let captured = producer.capture(
                CaptureOrigin {
                    source: CaptureSource::Evdev,
                    device_token: 0,
                    instance_token: 0,
                },
                CapturedInputPayload::from_input_event(input_event),
            );
            let _ = producer.try_push(captured);
        };

        // Start the listener
        match listener.start(callback) {
            Ok(()) => {
                let _ = started_tx.send(Ok(()));
            }
            Err(error) => {
                let message = error.to_string();
                tracing::error!("Evdev listener error: {message}");
                let _ = started_tx.send(Err(message));
                return;
            }
        }

        // Keep the thread alive until shutdown
        while running.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }

        let _ = listener.stop();
    });

    match started_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(message)) => anyhow::bail!(message),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(handle),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            anyhow::bail!("Evdev listener exited before startup completed")
        }
    }
}

#[cfg(target_os = "linux")]
fn input_event_from_evdev_driver_event(
    event: rshare_platform::EvdevDriverEvent,
) -> Option<InputEvent> {
    use rshare_input::{ButtonState, KeyCode, MouseButton};
    use rshare_platform::EvdevDriverEvent;

    let (event, device_kind, _device_path) = match event {
        EvdevDriverEvent::MouseMove { x, y, device_path } => (
            InputEvent::MouseMove { x, y },
            LocalInputDeviceKind::Mouse,
            device_path,
        ),
        EvdevDriverEvent::MouseButton {
            button,
            pressed,
            device_path,
        } => {
            let state = if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            (
                InputEvent::MouseButton {
                    button: MouseButton::from_code(button as u8),
                    state,
                },
                LocalInputDeviceKind::Mouse,
                device_path,
            )
        }
        EvdevDriverEvent::MouseWheel {
            delta_x,
            delta_y,
            device_path,
        } => (
            InputEvent::MouseWheel { delta_x, delta_y },
            LocalInputDeviceKind::Mouse,
            device_path,
        ),
        EvdevDriverEvent::Key {
            keycode,
            pressed,
            device_path,
        } => {
            let state = if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            (
                InputEvent::Key {
                    keycode: KeyCode::Raw(keycode),
                    state,
                },
                LocalInputDeviceKind::Keyboard,
                device_path,
            )
        }
    };

    match device_kind {
        LocalInputDeviceKind::Keyboard | LocalInputDeviceKind::Mouse => {}
        _ => return None,
    }
    Some(event)
}

fn flatten_daemon_task_result(
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    match result {
        Ok(result) => result,
        Err(error) => Err(error.into()),
    }
}

fn into_authenticated_network_message(
    event: NetworkEvent,
) -> std::result::Result<(DeviceId, ControlConnectionId, Message), NetworkEvent> {
    match event {
        NetworkEvent::ControlReceived { auth, frame } => Ok((
            auth.peer_id,
            auth.control_connection_id,
            frame.into_message(),
        )),
        NetworkEvent::TelemetryReceived { auth, frame } => Ok((
            auth.peer_id,
            auth.control_connection_id,
            frame.into_message(),
        )),
        NetworkEvent::BulkReceived { auth, frame } => Ok((
            auth.peer_id,
            auth.control_connection_id,
            frame.into_message(),
        )),
        event => Err(event),
    }
}

async fn enqueue_router_command(
    sender: &tokio::sync::mpsc::Sender<RouterCommand>,
    command: RouterCommand,
) -> Result<()> {
    sender
        .send(command)
        .await
        .map_err(|_| anyhow::anyhow!("input router command channel is closed"))
}

/// Persist and broadcast each canonical runtime layout update before any
/// subsequent connectivity command can cause the router to resolve an edge.
async fn persist_and_publish_layout(
    layout: &LayoutGraph,
    layout_path: &Path,
    input_command_tx: &tokio::sync::mpsc::Sender<RouterCommand>,
) -> Result<()> {
    save_layout_to_path(layout, layout_path)?;
    enqueue_router_command(
        input_command_tx,
        RouterCommand::LayoutChanged(layout.clone()),
    )
    .await
}

async fn persist_and_publish_connected_peer(
    layout: &LayoutGraph,
    layout_path: &Path,
    input_command_tx: &tokio::sync::mpsc::Sender<RouterCommand>,
    peer: DeviceId,
) -> Result<()> {
    persist_and_publish_layout(layout, layout_path, input_command_tx).await?;
    enqueue_router_command(
        input_command_tx,
        RouterCommand::ConnectivityChanged {
            peer,
            connected: true,
        },
    )
    .await
}

async fn complete_mobile_gateway_shutdown(
    shutdown_tx: &broadcast::Sender<()>,
    mobile_gateway_task: Option<&mut tokio::task::JoinHandle<Result<()>>>,
) -> Result<()> {
    let _ = shutdown_tx.send(());
    match mobile_gateway_task {
        Some(task) => flatten_daemon_task_result(task.await),
        None => Ok(()),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let log_file = get_log_file_path();
    let file_appender =
        tracing_appender::rolling::never(log_file.parent().unwrap(), log_file.file_name().unwrap());

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout)
                .with_ansi(true),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(file_appender)
                .with_ansi(false)
                .with_target(true),
        )
        .with(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("R-ShareMouse daemon starting...");
    tracing::info!("Log file: {}", log_file.display());

    let config = load_config_with_env_overrides()?;

    // Configure firewall on Windows to allow discovery and service ports
    #[cfg(windows)]
    {
        match firewall::configure_firewall(config.features.mobile_gateway_enabled) {
            Ok(result) => {
                if result.is_success() {
                    tracing::info!("Firewall configured successfully for R-ShareMouse");
                } else {
                    tracing::warn!("Firewall configuration incomplete: {:?}", result);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to configure firewall: {}", e);
                tracing::warn!("Device discovery may not work. Please run as administrator or add firewall rules manually.");
            }
        }
    }

    let hostname = hostname::get()
        .unwrap_or_else(|_| "unknown".into())
        .to_string_lossy()
        .to_string();
    let device_id = rshare_core::service::load_or_create_local_device_id()?;
    let device_name = format!("{}-R-ShareMouse", hostname);
    let bind_address = format!("{}:{}", config.network.bind_address, config.network.port);
    let layout_path = rshare_core::service::layout_graph_path()?;

    let mut network_manager = NetworkManager::new(device_id, device_name.clone(), hostname.clone())
        .with_config(NetworkManagerConfig {
            bind_address: bind_address.clone(),
            mdns_enabled: config.network.mdns_enabled,
            // Discovery may only initiate connections to peers with a persisted
            // operator-approved QUIC certificate pin.
            auto_connect: true,
            ..Default::default()
        });

    let receivers = network_manager.receivers();
    let mut events = receivers.events;
    let authenticated_peers = receivers.authenticated_peers;
    let input_registry = network_manager.input_registry();
    let ui_network_projection = network_manager.connection_snapshot_reader();
    let mut terminal_releases = network_manager
        .terminal_release_events()
        .await
        .expect("new network manager must expose terminal release events");
    let network_manager = Arc::new(Mutex::new(network_manager));
    {
        let mut manager = network_manager.lock().await;
        manager.start().await?;
    }

    let mut service_manager = rshare_core::service::ServiceManager::new()?;
    let _service_handle = service_manager.start().await?;
    let pid = std::process::id();

    // Discover and select backend
    let (input_mode, available_backends, backend_health, backend_error) =
        discover_and_select_backend();

    tracing::info!(
        "Backend selected: {:?} (available: {:?})",
        input_mode,
        available_backends
    );

    let mut daemon_state = DaemonState::new_with_features(
        ServiceStatusSnapshot::new(
            device_id,
            device_name.clone(),
            hostname.clone(),
            bind_address.clone(),
            27432,
            pid,
        ),
        RuntimeFeatureConfig::from_config(&config),
    );
    daemon_state.refresh_local_controls_platform();
    daemon_state.layout = load_layout_from_path(device_id, &layout_path)?;
    let should_save_runtime_layout = daemon_state.reconcile_local_layout_geometry();
    if should_save_runtime_layout {
        save_layout_to_path(&daemon_state.layout, &layout_path)?;
    }
    let state = Arc::new(RwLock::new(daemon_state));

    let (inject_backend, inject_health, inject_error) = build_inject_backend(input_mode);
    let inject_kind = inject_backend.kind();
    let text_commit_supported = inject_backend.supports_text_commit();
    let last_backend_error = inject_error.or(backend_error);
    let input_backend_healthy = matches!(backend_health, BackendHealth::Healthy)
        && matches!(inject_health, BackendHealth::Healthy);

    // Initialize backend state
    {
        let mut s = state.write().await;
        s.update_backend_state(
            input_mode,
            available_backends,
            backend_health.clone(), // capture health
            inject_kind,
            inject_health,
            text_commit_supported,
            last_backend_error,
        );
    }
    let injection = InputInjectionHandle::spawn(inject_backend, InjectionActorConfig::default())?;
    let inject_backend = injection.clone();

    let ipc_listener = TcpListener::bind(default_ipc_addr()).await?;
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(8);
    let (local_events_tx, _) = broadcast::channel::<LocalInputDiagnosticEvent>(256);
    let (endpoint_events_tx, _) = broadcast::channel::<EndpointEvent>(256);
    let audio_runtime = audio_runtime::AudioRuntimeHandle::start()?;
    let usb_runtime = Arc::new(Mutex::new(
        rshare_platform::ExperimentalUsbHostRuntime::new(),
    ));

    let (capture_producer, input_consumer) = SemanticInputIngress::new(256);
    #[cfg(not(windows))]
    let portable_origin = CaptureOrigin {
        source: CaptureSource::PortableHook,
        device_token: 0,
        instance_token: 0,
    };

    // Every capture backend publishes into the same bounded semantic ingress.
    #[cfg(target_os = "linux")]
    let mut input_channel = {
        match try_start_evdev_capture(capture_producer.clone()) {
            Ok(_evdev_task) => {
                tracing::info!("Using Evdev backend for input capture (kernel-level)");
                None
            }
            Err(e) => {
                tracing::warn!(
                    "Evdev capture unavailable: {:?}, using RDev (Portable) backend",
                    e
                );
                Some(RDevInputListener::from_producer(
                    capture_producer.clone(),
                    portable_origin,
                ))
            }
        }
    };

    #[cfg(windows)]
    let (_input_listener, mut windows_driver_capture) = {
        let windows_hook_origin = CaptureOrigin {
            source: CaptureSource::WindowsHook,
            device_token: 0,
            instance_token: 0,
        };
        set_local_shortcut_suppression(false);
        let use_windows_filter_capture = {
            let state = state.read().await;
            windows_should_use_filter_capture(input_mode, &state.local_controls.driver)
        };

        let selection = start_windows_capture_with_fallback(
            use_windows_filter_capture,
            {
                let producer = capture_producer.clone();
                move || WindowsDriverCapture::start(producer)
            },
            {
                let producer = capture_producer.clone();
                move || {
                    let mut input_listener = DefaultInputListener::new();
                    input_listener.start(Box::new(move |event| {
                        let _ = producer.try_push_event(windows_hook_origin, event);
                    }))?;
                    Ok(input_listener)
                }
            },
        )?;
        match selection {
            WindowsCaptureSelection::Filter(capture) => {
                tracing::info!("Using RShare Windows filter driver input capture");
                (None, Some(capture))
            }
            WindowsCaptureSelection::Hook {
                listener,
                filter_error,
            } => {
                if let Some(error) = filter_error {
                    tracing::warn!(
                        "Windows filter capture failed; using low-level hook fallback: {error}"
                    );
                    let mut state = state.write().await;
                    state.local_controls.capture_backend.mode =
                        Some(ResolvedInputMode::WindowsNative);
                    state.local_controls.capture_backend.kind = Some(BackendKind::WindowsNative);
                    state.local_controls.capture_backend.health = Some(BackendHealth::Healthy);
                    state.local_controls.capture_backend.active = true;
                    state.local_controls.last_error =
                        Some(format!("Filter capture fallback: {error}"));
                } else {
                    tracing::info!("Using native Windows low-level hook input capture");
                }
                (Some(listener), None)
            }
        }
    };

    #[cfg(all(not(target_os = "linux"), not(windows)))]
    let mut input_channel = Some(RDevInputListener::from_producer(
        capture_producer.clone(),
        portable_origin,
    ));

    let mut gamepad_listener_config = GamepadListenerConfig::from(&config.gamepad);
    gamepad_listener_config.enabled = true;
    let mut gamepad_listener = GilrsGamepadListener::new(
        capture_producer.clone(),
        CaptureOrigin {
            source: CaptureSource::Gamepad,
            device_token: 0,
            instance_token: 0,
        },
        gamepad_listener_config,
    );
    gamepad_listener.start()?;

    // Start RDev listener if we're using it
    #[cfg(target_os = "linux")]
    if let Some(ref listener) = input_channel {
        listener.start().await?;
        tracing::info!("Using RDev fallback input capture on Linux");
    }

    #[cfg(all(not(target_os = "linux"), not(windows)))]
    if let Some(ref listener) = input_channel {
        listener.start().await?;
    }

    tracing::info!("Daemon started as device {} ({})", device_name, device_id);
    tracing::info!("Listening for connections on {}", bind_address);
    tracing::info!("Device discovery on port 27432");
    tracing::info!("Local IPC listening on {}", default_ipc_addr());

    let layout_path = Arc::new(layout_path);
    let (input_command_tx, input_command_rx) = tokio::sync::mpsc::channel(64);
    let (input_state, input_feeds) = input_state_channel(32);
    let input_metrics = Arc::new(ControlMetrics::default());
    let diagnostics_runtime =
        DiagnosticsRuntime::new(input_metrics.clone(), DIAGNOSTICS_HISTORY_CAPACITY);
    let diagnostics_samples = diagnostics_runtime.latest_receiver();
    let diagnostics = diagnostics_runtime.handle();
    let (diagnostics_shutdown_tx, diagnostics_shutdown_rx) = tokio::sync::mpsc::channel(1);
    let _diagnostics_task = tokio::spawn(diagnostics_runtime.run(diagnostics_shutdown_rx));
    let ui_network_changes = ui_network_projection.changes();
    let initial_ui_snapshot = {
        let connection_infos = ui_network_projection.connection_infos();
        let state = state.read().await;
        state.ui_snapshot_for_connections(&connection_infos)
    };
    let ui_state = StateAggregator::try_with_projection_and_diagnostics(
        initial_ui_snapshot,
        256,
        input_feeds,
        Arc::new(DaemonUiProjectionSource {
            state: state.clone(),
            network: Arc::new(ui_network_projection),
            diagnostics: Some(diagnostics_samples.clone()),
        }),
        diagnostics_samples,
        ui_network_changes,
    )?;

    let ipc_task = tokio::spawn(run_ipc_server(
        ipc_listener,
        ui_state.clone(),
        state.clone(),
        network_manager.clone(),
        input_command_tx.clone(),
        inject_backend.clone(),
        audio_runtime.clone(),
        usb_runtime.clone(),
        local_events_tx.clone(),
        endpoint_events_tx.clone(),
        layout_path.clone(),
        shutdown_tx.clone(),
    ));
    let local_controls_snapshot_state = state.clone();
    let local_controls_snapshot_ui_state = ui_state.clone();
    let local_controls_feed = LocalControlsFeed::new(
        Arc::new(move || -> LocalControlsSnapshotFuture {
            let state = local_controls_snapshot_state.clone();
            let ui_state = local_controls_snapshot_ui_state.clone();
            Box::pin(async move { local_controls_fallback_snapshot(&state, &ui_state).await })
        }),
        local_events_tx.clone(),
    );
    let ui_state_ws_task = tokio::spawn(run_ui_state_server(
        default_local_controls_ws_addr(),
        ui_state.clone(),
        local_controls_feed,
        shutdown_tx.subscribe(),
    ));
    let (mobile_gateway_enabled, mobile_access) = {
        let state = state.read().await;
        (
            state.features.mobile_gateway_enabled,
            state.mobile_access.clone(),
        )
    };
    let mobile_gateway_state = state.clone();
    let mobile_gateway_network_manager = network_manager.clone();
    let mobile_gateway_inject_backend = inject_backend.clone();
    let mobile_gateway_local_events_tx = local_events_tx.clone();
    let mut mobile_gateway_shutdown_rx = shutdown_tx.subscribe();
    let mut mobile_gateway_task = tokio::spawn(async move {
        if mobile_gateway_enabled {
            mobile_gateway::run_mobile_gateway_server(
                mobile_access,
                mobile_gateway_state,
                mobile_gateway_network_manager,
                mobile_gateway_inject_backend,
                mobile_gateway_local_events_tx,
                mobile_gateway_shutdown_rx,
            )
            .await
        } else {
            tracing::info!("Mobile gateway disabled by configuration");
            let _ = mobile_gateway_shutdown_rx.recv().await;
            Ok(())
        }
    });

    let (router_layout, router_geometry, connected_peers) = {
        let state = state.read().await;
        (
            state.layout.clone(),
            VirtualDesktopGeometry::from(&state.local_controls.display),
            state
                .devices
                .values()
                .filter(|peer| peer.connected)
                .map(|peer| peer.id)
                .collect::<Vec<_>>(),
        )
    };
    let input_router = InputRouter::new(device_id, router_layout, router_geometry, connected_peers);
    if !input_backend_healthy {
        enqueue_router_command(&input_command_tx, RouterCommand::BackendDegraded).await?;
    }
    let forwarding_policy = {
        let state = state.read().await;
        InputForwardingPolicy {
            automatic_input_forwarding: state.features.automatic_input_forwarding,
            suppress_local_shortcuts_when_remote: state
                .features
                .suppress_local_shortcuts_when_remote,
            shortcut_suppression_supported: local_shortcut_suppression_supported(
                &state.local_controls,
            ),
        }
    };
    if !forwarding_policy.admits_remote_input() {
        tracing::warn!(
            "automatic input forwarding is unavailable: local shortcut suppression is required but unsupported"
        );
    }
    let input_runtime = InputRuntime::new(
        input_consumer,
        input_router,
        input_registry.clone(),
        input_state,
        input_metrics.clone(),
        injection.clone(),
    )
    .with_forwarding_policy(forwarding_policy, Arc::new(PlatformShortcutSuppressor));
    let (system_safety_tx, system_safety_rx) = tokio::sync::mpsc::unbounded_channel();
    let system_safety_injection = injection.clone();
    let system_safety_watcher = rshare_platform::SystemSafetyWatcher::start(move |event| {
        let _ = dispatch_system_safety_event(&system_safety_injection, &system_safety_tx, event);
    })?;
    if system_safety_watcher.is_supported() {
        tracing::info!("Native session-lock and system-suspend safety watcher active");
    }
    let input_forwarding_task =
        tokio::spawn(input_runtime.run_with_safety(input_command_rx, system_safety_rx));
    let mut authenticated_input_task = tokio::spawn(run_authenticated_input_peers(
        authenticated_peers,
        injection.clone(),
        Duration::from_secs(2),
        shutdown_tx.subscribe(),
    ));
    let current_generations = Arc::new(std::sync::RwLock::new(HashMap::new()));
    let terminal_generation_view = current_generations.clone();
    let terminal_injection = injection.clone();
    let _terminal_release_task = tokio::spawn(async move {
        while let Some(release) = terminal_releases.recv().await {
            let current = terminal_generation_view
                .read()
                .expect("input generation registry poisoned")
                .get(&release.auth.peer_id)
                .copied();
            if current == Some(release.auth.control_connection_id) {
                terminal_injection.request_release_through(
                    rshare_core::AuthenticatedInputOwner {
                        peer_id: release.auth.peer_id,
                        control_connection_id: release.auth.control_connection_id,
                    },
                    release.epoch,
                    release.reason,
                );
            }
        }
    });

    let event_task = {
        let state = state.clone();
        let ui_state = ui_state.clone();
        let inject_backend = inject_backend.clone();
        let network_manager = network_manager.clone();
        let audio_runtime = audio_runtime.clone();
        let usb_runtime = usb_runtime.clone();
        let local_events_tx = local_events_tx.clone();
        let endpoint_events_tx = endpoint_events_tx.clone();
        let layout_path = layout_path.clone();
        let input_command_tx = input_command_tx.clone();
        let injection = injection.clone();
        let diagnostics = diagnostics.clone();
        let current_generations = current_generations.clone();
        tokio::spawn(async move {
            tracing::info!("Event task: starting to wait for events");
            while let Some(event) = events.recv().await {
                let event = match into_authenticated_network_message(event) {
                    Ok((from, control_connection_id, message)) => {
                        let reconcile = network_message_may_mutate_ui(&message);
                        handle_network_message(
                            &state,
                            &network_manager,
                            &input_registry,
                            &inject_backend,
                            &audio_runtime,
                            &usb_runtime,
                            &local_events_tx,
                            &endpoint_events_tx,
                            &diagnostics,
                            from,
                            control_connection_id,
                            message,
                        )
                        .await;
                        if let Err(error) = reconcile_ui_state_if(&ui_state, reconcile).await {
                            tracing::error!(
                                "Failed to reconcile UI state after peer message: {error:#}"
                            );
                            break;
                        }
                        continue;
                    }
                    Err(event) => event,
                };
                match event {
                    NetworkEvent::DeviceFound(device) => {
                        let layout_to_publish = {
                            let mut state = state.write().await;
                            state
                                .upsert_discovered(device)
                                .then(|| state.layout.clone())
                        };
                        if let Some(layout) = layout_to_publish {
                            if let Err(err) = persist_and_publish_layout(
                                &layout,
                                layout_path.as_ref(),
                                &input_command_tx,
                            )
                            .await
                            {
                                tracing::warn!("Failed to publish discovered-device layout: {err}");
                            }
                        }
                    }
                    NetworkEvent::DeviceConnected(auth) => {
                        let id = auth.peer_id;
                        diagnostics.activate_generation(DiagnosticSubscriberId {
                            peer_id: id,
                            control_connection_id: auth.control_connection_id,
                        });
                        current_generations
                            .write()
                            .expect("input generation registry poisoned")
                            .insert(id, auth.control_connection_id);
                        let (should_advertise_usb, layout) = {
                            let mut state = state.write().await;
                            state.mark_connected(&id, true);
                            (
                                state.features.usb_advertising_enabled(),
                                state.layout.clone(),
                            )
                        };
                        if let Err(error) = persist_and_publish_connected_peer(
                            &layout,
                            layout_path.as_ref(),
                            &input_command_tx,
                            id,
                        )
                        .await
                        {
                            tracing::error!(
                                "Failed to publish layout before connected snapshot: {error}"
                            );
                            injection.request_release_all_sources(
                                rshare_core::ReleaseAllReason::BackendFailure,
                            );
                            break;
                        }
                        if should_advertise_usb {
                            advertise_usb_devices_to(&network_manager, &usb_runtime, id).await;
                        }
                    }
                    NetworkEvent::DeviceDisconnected {
                        peer_id: id,
                        control_connection_id,
                    } => {
                        if current_generations
                            .read()
                            .expect("input generation registry poisoned")
                            .get(&id)
                            != Some(&control_connection_id)
                        {
                            continue;
                        }
                        diagnostics.clear_generation(DiagnosticSubscriberId {
                            peer_id: id,
                            control_connection_id,
                        });
                        current_generations
                            .write()
                            .expect("input generation registry poisoned")
                            .remove(&id);
                        injection.request_release_through(
                            rshare_core::AuthenticatedInputOwner {
                                peer_id: id,
                                control_connection_id,
                            },
                            rshare_core::SessionEpoch(u64::MAX),
                            rshare_core::ReleaseAllReason::SessionEnded,
                        );
                        if let Err(error) = enqueue_router_command(
                            &input_command_tx,
                            RouterCommand::ConnectivityChanged {
                                peer: id,
                                connected: false,
                            },
                        )
                        .await
                        {
                            tracing::error!("Failed to queue disconnected snapshot: {error}");
                            injection.request_release_all_sources(
                                rshare_core::ReleaseAllReason::BackendFailure,
                            );
                            break;
                        }
                        let mut state = state.write().await;
                        // Notify session state machine of target disconnection
                        state.session.on_target_disconnect(id);
                        fail_pending_usb_for_device(
                            &mut state,
                            id,
                            "USB probe target disconnected.",
                        );
                        state.mark_disconnected(&id);
                        sync_local_shortcut_suppression(&state);
                    }
                    NetworkEvent::DeviceLost(id) => {
                        let mut state = state.write().await;
                        state.mark_discovery_lost(&id);
                    }
                    NetworkEvent::ControlReceived { .. }
                    | NetworkEvent::TelemetryReceived { .. }
                    | NetworkEvent::BulkReceived { .. } => {
                        unreachable!("authenticated messages are handled before lifecycle events")
                    }
                    NetworkEvent::ConnectionError {
                        peer_id,
                        control_connection_id,
                        error,
                    } => {
                        tracing::warn!(
                            "Connection error to {:?} generation {:?}: {}",
                            peer_id,
                            control_connection_id,
                            error
                        );
                        if let Some(device_id) = peer_id {
                            let Some(control_connection_id) =
                                take_current_generation_for_connection_error(
                                    current_generations.as_ref(),
                                    device_id,
                                    control_connection_id,
                                )
                            else {
                                continue;
                            };
                            diagnostics.clear_generation(DiagnosticSubscriberId {
                                peer_id: device_id,
                                control_connection_id,
                            });
                            injection.request_release_through(
                                rshare_core::AuthenticatedInputOwner {
                                    peer_id: device_id,
                                    control_connection_id,
                                },
                                rshare_core::SessionEpoch(u64::MAX),
                                rshare_core::ReleaseAllReason::SessionEnded,
                            );
                            if let Err(command_error) = enqueue_router_command(
                                &input_command_tx,
                                RouterCommand::ConnectivityChanged {
                                    peer: device_id,
                                    connected: false,
                                },
                            )
                            .await
                            {
                                tracing::error!(
                                    "Failed to queue connection-error snapshot: {command_error}"
                                );
                                injection.request_release_all_sources(
                                    rshare_core::ReleaseAllReason::BackendFailure,
                                );
                                break;
                            }
                            let mut state = state.write().await;
                            state.session.on_target_disconnect(device_id);
                            fail_pending_usb_for_device(
                                &mut state,
                                device_id,
                                "USB probe target connection failed.",
                            );
                            state.mark_disconnected(&device_id);
                            sync_local_shortcut_suppression(&state);
                        }
                    }
                }
                if let Err(error) = ui_state.reconcile_from_projection().await {
                    tracing::error!("Failed to reconcile UI state after network event: {error:#}");
                    break;
                }
            }
            tracing::debug!("Event task: events channel closed");
        })
    };

    tracing::info!("Entering tokio::select! loop");
    let mut mobile_gateway_finished = false;
    let shutdown_reason: Result<()> = tokio::select! {
        result = signal::ctrl_c() => {
            match result {
                Ok(()) => tracing::info!("Shutdown signal received"),
                Err(e) => tracing::warn!("Ctrl-C handler error: {}", e),
            }
            Ok(())
        }
        _ = shutdown_rx.recv() => {
            tracing::info!("Shutdown requested over IPC");
            Ok(())
        }
        result = ipc_task => {
            tracing::info!("IPC task completed");
            flatten_daemon_task_result(result)
        }
        result = ui_state_ws_task => {
            tracing::info!("UI state websocket task completed");
            flatten_daemon_task_result(result)
        }
        result = &mut mobile_gateway_task => {
            tracing::info!("Mobile gateway task completed");
            mobile_gateway_finished = true;
            flatten_daemon_task_result(result)
        }
        result = event_task => {
            tracing::info!("Event task completed");
            result.map_err(Into::into)
        }
        result = input_forwarding_task => {
            tracing::info!("Input forwarding task completed");
            result.map_err(Into::into)
        }
        result = &mut authenticated_input_task => {
            tracing::info!("Authenticated input task completed");
            result.map_err(Into::into)
        }
    };

    let mobile_shutdown_result = complete_mobile_gateway_shutdown(
        &shutdown_tx,
        if mobile_gateway_finished {
            None
        } else {
            Some(&mut mobile_gateway_task)
        },
    )
    .await;

    tracing::info!("tokio::select! exited, cleaning up");
    let _ = diagnostics_shutdown_tx.try_send(());
    let _ = enqueue_router_command(&input_command_tx, RouterCommand::Shutdown).await;
    injection.request_release_all_sources(rshare_core::ReleaseAllReason::SessionEnded);
    set_local_shortcut_suppression(false);
    audio_runtime.stop_capture();
    audio_runtime.stop_render();
    audio_runtime.shutdown();
    drop(system_safety_watcher);
    #[cfg(windows)]
    if let Some(capture) = windows_driver_capture.as_mut() {
        capture.stop();
    }
    // Input listener cleanup is handled automatically by task drops
    let network_stop_result = network_manager.lock().await.stop().await;
    let injection_stop_result = tokio::task::spawn_blocking(move || injection.shutdown()).await?;

    shutdown_reason?;
    mobile_shutdown_result?;
    network_stop_result?;
    injection_stop_result?;

    tracing::info!("R-ShareMouse daemon stopped");
    std::process::exit(0);
}

async fn run_ipc_server(
    listener: TcpListener,
    ui_state: StateAggregatorHandle,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    input_command_tx: tokio::sync::mpsc::Sender<RouterCommand>,
    inject_backend: InputInjectionHandle,
    audio_runtime: audio_runtime::AudioRuntimeHandle,
    usb_runtime: UsbHostRuntime,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    endpoint_events_tx: broadcast::Sender<EndpointEvent>,
    layout_path: Arc<PathBuf>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let ui_state = ui_state.clone();
        let state = state.clone();
        let network_manager = network_manager.clone();
        let input_command_tx = input_command_tx.clone();
        let inject_backend = inject_backend.clone();
        let audio_runtime = audio_runtime.clone();
        let usb_runtime = usb_runtime.clone();
        let local_events_tx = local_events_tx.clone();
        let endpoint_events_tx = endpoint_events_tx.clone();
        let layout_path = layout_path.clone();
        let shutdown_tx = shutdown_tx.clone();

        tokio::spawn(async move {
            if let Err(err) = handle_ipc_client(
                stream,
                ui_state,
                state,
                network_manager,
                input_command_tx,
                inject_backend,
                audio_runtime,
                usb_runtime,
                local_events_tx,
                endpoint_events_tx,
                layout_path,
                shutdown_tx,
            )
            .await
            {
                tracing::debug!("IPC client error: {}", err);
            }
        });
    }
}

async fn handle_ipc_client(
    mut stream: TcpStream,
    ui_state: StateAggregatorHandle,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    input_command_tx: tokio::sync::mpsc::Sender<RouterCommand>,
    inject_backend: InputInjectionHandle,
    audio_runtime: audio_runtime::AudioRuntimeHandle,
    usb_runtime: UsbHostRuntime,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    endpoint_events_tx: broadcast::Sender<EndpointEvent>,
    layout_path: Arc<PathBuf>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    let Some(request) = read_json_request(&mut stream).await? else {
        return Ok(());
    };

    if let Some(subscriber) = ui_state_subscriber_for_request(&request, &ui_state).await? {
        return stream_ui_state(&mut stream, subscriber).await;
    }

    if matches!(request, DaemonRequest::SubscribeLocalControls) {
        write_local_controls_fallback_snapshot(&mut stream, &state, &ui_state).await?;
        let mut events = local_events_tx.subscribe();
        loop {
            match events.recv().await {
                Ok(event) => {
                    write_json_response(&mut stream, &DaemonResponse::LocalControlEvent(event))
                        .await?;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        return Ok(());
    }

    if let DaemonRequest::SubscribeEndpointEvents { filter } = &request {
        request_remote_endpoint_events(&network_manager, &state, filter).await;
        let events = {
            let mut state = state.write().await;
            state.endpoint_events(filter, None, Some(128))
        };
        write_json_response(&mut stream, &DaemonResponse::EndpointEvents(events)).await?;
        let mut local_events = local_events_tx.subscribe();
        let mut endpoint_events = endpoint_events_tx.subscribe();
        loop {
            tokio::select! {
                event = local_events.recv() => {
                    match event {
                        Ok(event) => {
                            let endpoint_event = {
                                let mut state = state.write().await;
                                state.endpoint_event_from_local(event)
                            };
                            if filter.matches(&endpoint_event) {
                                write_json_response(
                                    &mut stream,
                                    &DaemonResponse::EndpointEvent(endpoint_event),
                                )
                                .await?;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                event = endpoint_events.recv() => {
                    match event {
                        Ok(endpoint_event) => {
                            if filter.matches(&endpoint_event) {
                                write_json_response(
                                    &mut stream,
                                    &DaemonResponse::EndpointEvent(endpoint_event),
                                )
                                .await?;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
        return Ok(());
    }

    if let DaemonRequest::CaptureDisplay(capture_request) = &request {
        let result = static_capture::capture_display(capture_request.clone())
            .await
            .unwrap_or_else(|error| DisplayCaptureResult {
                request_id: DeviceId::new_v4(),
                status: DisplayOperationStatus::ApplyFailed,
                message: Some(error.to_string()),
                payload: None,
                blob: None,
            });
        let (result, binary_frame) = prepare_display_capture_response(result);
        write_json_response(&mut stream, &DaemonResponse::DisplayCapture(result.clone())).await?;
        if let Some(frame) = binary_frame {
            IpcFrameCodec::default()
                .write_frame(&mut stream, frame.kind, &frame.payload)
                .await?;
        }
        return Ok(());
    }

    handle_persistent_json_connection_with_first(stream, request, move |request| {
        let ui_state = ui_state.clone();
        let state = Arc::clone(&state);
        let network_manager = Arc::clone(&network_manager);
        let input_command_tx = input_command_tx.clone();
        let inject_backend = inject_backend.clone();
        let audio_runtime = audio_runtime.clone();
        let usb_runtime = Arc::clone(&usb_runtime);
        let local_events_tx = local_events_tx.clone();
        let layout_path = Arc::clone(&layout_path);
        let shutdown_tx = shutdown_tx.clone();
        async move {
            let reconcile = request_may_mutate_ui(&request);
            let response = dispatch_ipc_request(
                request,
                state,
                network_manager,
                input_command_tx,
                inject_backend,
                audio_runtime,
                usb_runtime,
                local_events_tx,
                layout_path,
                shutdown_tx,
            )
            .await?;
            if reconcile {
                ui_state
                    .reconcile_from_projection()
                    .await
                    .context("failed to reconcile UI state after daemon command")?;
            }
            Ok(overlay_stream_truth_onto_fallback_response(
                response,
                ui_state.latest_snapshot().as_ref(),
            ))
        }
    })
    .await
}

fn network_message_may_mutate_ui(message: &Message) -> bool {
    match message {
        Message::Hello { .. }
        | Message::HelloBack { .. }
        | Message::HelloRejected { .. }
        | Message::Goodbye { .. }
        | Message::EndpointEventSnapshot { .. }
        | Message::EndpointEventDelta { .. }
        | Message::EndpointInjectResult { .. }
        | Message::LatencyProbeAck { .. }
        | Message::AudioStreamStart { .. }
        | Message::AudioStreamStop { .. }
        | Message::AudioStreamError { .. }
        | Message::UsbDeviceAttached { .. }
        | Message::UsbDeviceDetached { .. }
        | Message::UsbForwardingError { .. }
        | Message::UsbDeviceClaimRequest { .. }
        | Message::UsbDeviceClaimResponse { .. }
        | Message::UsbDeviceRelease { .. }
        | Message::UsbDeviceReset { .. }
        | Message::UsbTransferCancel { .. }
        | Message::ScreenUpdate { .. } => true,
        Message::InputDiagnostic { .. }
        | Message::EndpointEventSubscribe { .. }
        | Message::EndpointInjectRequest { .. }
        | Message::LatencyProbe { .. }
        | Message::AudioFrame { .. }
        | Message::UsbTransfer { .. }
        | Message::UsbTransferComplete { .. }
        | Message::UsbFlowControl { .. }
        | Message::ClipboardData { .. }
        | Message::ClipboardRequest
        | Message::ClipboardResponse { .. }
        | Message::Heartbeat { .. }
        | Message::Ack { .. }
        | Message::Error { .. } => false,
    }
}

async fn reconcile_ui_state_if(state: &StateAggregatorHandle, required: bool) -> Result<()> {
    if required {
        state.reconcile_from_projection().await?;
    }
    Ok(())
}

async fn write_local_controls_fallback_snapshot<S>(
    stream: &mut S,
    state: &Arc<RwLock<DaemonState>>,
    ui_state: &StateAggregatorHandle,
) -> Result<()>
where
    S: tokio::io::AsyncWrite + Unpin,
{
    let local_controls = local_controls_fallback_snapshot(state, ui_state).await?;
    write_json_response(stream, &DaemonResponse::LocalControls(local_controls)).await
}

async fn local_controls_fallback_snapshot(
    state: &Arc<RwLock<DaemonState>>,
    ui_state: &StateAggregatorHandle,
) -> Result<LocalControlDeviceSnapshot> {
    let local_controls = {
        let mut state = state.write().await;
        state.refresh_local_controls_platform();
        state.local_control_snapshot()
    };
    ui_state
        .reconcile_from_projection()
        .await
        .context("failed to reconcile UI state for local-controls fallback")?;
    let response = overlay_stream_truth_onto_fallback_response(
        DaemonResponse::LocalControls(local_controls),
        ui_state.latest_snapshot().as_ref(),
    );
    match response {
        DaemonResponse::LocalControls(snapshot) => Ok(snapshot),
        _ => unreachable!("local-controls overlay preserves the response variant"),
    }
}

fn overlay_stream_truth_onto_fallback_response(
    response: DaemonResponse,
    snapshot: &UiSnapshot,
) -> DaemonResponse {
    match response {
        DaemonResponse::Status(mut status) => {
            status.session_state = snapshot.active_sessions.control.clone();
            status.active_target = snapshot
                .active_sessions
                .control
                .as_ref()
                .and_then(|session| match session {
                    ControlSessionState::TransitioningToRemote { target, .. }
                    | ControlSessionState::RemoteActive { target, .. } => Some(*target),
                    ControlSessionState::LocalReady
                    | ControlSessionState::ReturningLocal { .. }
                    | ControlSessionState::Suspended { .. } => None,
                });
            DaemonResponse::Status(status)
        }
        DaemonResponse::LocalControls(mut controls) => {
            if let Some(pointer) = &snapshot.dynamic_state.pointer {
                controls.mouse.x = pointer.x;
                controls.mouse.y = pointer.y;
                controls.mouse.current_display_id = pointer.display_id.clone();
            }
            controls.keyboard.pressed_keys = snapshot
                .dynamic_state
                .pressed_keys
                .iter()
                .map(|keycode| format!("{:?}", rshare_input::KeyCode::Raw(*keycode)))
                .collect();
            controls.mouse.pressed_buttons = snapshot
                .dynamic_state
                .pressed_mouse_buttons
                .iter()
                .map(|button| format!("{button:?}"))
                .collect();
            controls.gamepads = snapshot.dynamic_state.gamepads.clone();
            controls.display = snapshot.display_inventory.clone();
            DaemonResponse::LocalControls(controls)
        }
        other => other,
    }
}

fn request_may_mutate_ui(request: &DaemonRequest) -> bool {
    matches!(
        request,
        DaemonRequest::Capabilities { .. }
            | DaemonRequest::LocalControls
            | DaemonRequest::Connect { .. }
            | DaemonRequest::Disconnect { .. }
            | DaemonRequest::ApprovePeer { .. }
            | DaemonRequest::SetLayout { .. }
            | DaemonRequest::InjectEndpointEvent { .. }
            | DaemonRequest::RunLocalInputTest { .. }
            | DaemonRequest::RunRemoteLatencyTest { .. }
            | DaemonRequest::RunRemoteUsbDescriptorProbe { .. }
            | DaemonRequest::SetAudioDefaultOutput { .. }
            | DaemonRequest::SetAudioOutputVolume { .. }
            | DaemonRequest::SetAudioOutputMute { .. }
            | DaemonRequest::StartAudioCapture { .. }
            | DaemonRequest::StopAudioCapture
            | DaemonRequest::StartAudioForwarding { .. }
            | DaemonRequest::StopAudioForwarding
            | DaemonRequest::RunAudioTest { .. }
            | DaemonRequest::UpdateDisplaySettings(_)
            | DaemonRequest::ListVirtualDisplays
            | DaemonRequest::CreateVirtualDisplay(_)
            | DaemonRequest::RemoveVirtualDisplay(_)
    )
}

async fn dispatch_ipc_request(
    request: DaemonRequest,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    input_command_tx: tokio::sync::mpsc::Sender<RouterCommand>,
    inject_backend: InputInjectionHandle,
    audio_runtime: audio_runtime::AudioRuntimeHandle,
    usb_runtime: UsbHostRuntime,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    layout_path: Arc<PathBuf>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<DaemonResponse> {
    let response = match request {
        DaemonRequest::Status => {
            let connection_infos = {
                let manager = network_manager.lock().await;
                manager.connection_infos().await
            };
            let state = state.read().await;
            DaemonResponse::Status(state.status_snapshot_for_connections(&connection_infos))
        }
        DaemonRequest::Devices => {
            let state = state.read().await;
            DaemonResponse::Devices(state.device_snapshots())
        }
        DaemonRequest::Capabilities { device_id } => {
            let connection_infos = {
                let manager = network_manager.lock().await;
                manager.connection_infos().await
            };
            let network = network_snapshot_from_connections(&connection_infos);
            let mut state = state.write().await;
            state.refresh_local_controls_platform();
            DaemonResponse::Capabilities(state.capability_registry_snapshot(&network, device_id))
        }
        DaemonRequest::Connect { device_id } => {
            let address = {
                let state = state.read().await;
                state
                    .devices
                    .get(&device_id)
                    .and_then(|device| device.addresses.first().cloned())
            };

            match address {
                Some(address) => {
                    let result = {
                        let mut manager = network_manager.lock().await;
                        manager.connect_to(device_id, &address).await
                    };

                    match result {
                        Ok(_) => {
                            let layout_to_publish = {
                                let mut state = state.write().await;
                                state
                                    .mark_connected(&device_id, true)
                                    .then(|| state.layout.clone())
                            };
                            if let Some(layout) = layout_to_publish {
                                if let Err(err) = persist_and_publish_layout(
                                    &layout,
                                    layout_path.as_ref(),
                                    &input_command_tx,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        "Failed to publish connected-device layout: {err}"
                                    );
                                }
                            }
                            DaemonResponse::Ack
                        }
                        Err(err) => DaemonResponse::Error(err.to_string()),
                    }
                }
                None => DaemonResponse::Error(format!("No known address for device {}", device_id)),
            }
        }
        DaemonRequest::Disconnect { device_id } => {
            let result = {
                let mut manager = network_manager.lock().await;
                manager.disconnect_from(&device_id).await
            };

            match result {
                Ok(_) => {
                    let mut state = state.write().await;
                    state.session.on_target_disconnect(device_id);
                    state.mark_disconnected(&device_id);
                    sync_local_shortcut_suppression(&state);
                    DaemonResponse::Ack
                }
                Err(err) => DaemonResponse::Error(err.to_string()),
            }
        }
        DaemonRequest::ListPendingPeerApprovals => {
            let manager = network_manager.lock().await;
            DaemonResponse::PendingPeerApprovals(manager.pending_peer_approvals().await)
        }
        DaemonRequest::ApprovePeer { approval_id } => {
            let manager = network_manager.lock().await;
            if manager.approve_peer(&approval_id).await {
                DaemonResponse::Ack
            } else {
                DaemonResponse::Error("Unknown, expired, or already-used peer approval".to_string())
            }
        }
        DaemonRequest::GetLayout => {
            let state = state.read().await;
            DaemonResponse::Layout(state.layout.clone())
        }
        DaemonRequest::SetLayout { layout } => {
            let mut canonical_layout = layout;
            let (local_device_id, previous_layout) = {
                let state = state.read().await;
                (state.status.device_id, state.layout.clone())
            };
            canonical_layout.canonicalize_local_device(local_device_id);

            match persist_and_publish_layout(
                &canonical_layout,
                layout_path.as_ref(),
                &input_command_tx,
            )
            .await
            {
                Ok(()) => {
                    state.write().await.layout = canonical_layout;
                    DaemonResponse::Ack
                }
                Err(error) => {
                    inject_backend
                        .request_release_all_sources(rshare_core::ReleaseAllReason::BackendFailure);
                    if let Err(rollback_error) =
                        save_layout_to_path(&previous_layout, layout_path.as_ref())
                    {
                        tracing::error!(
                                "Failed to roll back layout after router channel closed: {rollback_error}"
                            );
                    }
                    DaemonResponse::Error(error.to_string())
                }
            }
        }
        DaemonRequest::ListUsbDevices => {
            let usb_enabled = {
                let state = state.read().await;
                state.features.usb_forwarding_experimental
            };
            if !usb_enabled {
                DaemonResponse::Error(
                    "Experimental USB forwarding is disabled in settings.".to_string(),
                )
            } else {
                match usb_runtime.lock().await.enumerate_devices() {
                    Ok(devices) => DaemonResponse::UsbDevices(devices),
                    Err(error) => DaemonResponse::Error(error.to_string()),
                }
            }
        }
        DaemonRequest::LocalControls => {
            let mut state = state.write().await;
            state.refresh_local_controls_platform();
            DaemonResponse::LocalControls(state.local_control_snapshot())
        }
        DaemonRequest::EndpointEvents {
            filter,
            after_sequence,
            limit,
        } => {
            request_remote_endpoint_events(&network_manager, &state, &filter).await;
            let mut state = state.write().await;
            DaemonResponse::EndpointEvents(state.endpoint_events(&filter, after_sequence, limit))
        }
        DaemonRequest::MobileAccess => {
            let state = state.read().await;
            DaemonResponse::MobileAccess(state.mobile_access.snapshot())
        }
        DaemonRequest::InjectEndpointEvent { target, request } => {
            DaemonResponse::EndpointInjectResult(
                inject_endpoint_event(
                    &network_manager,
                    &inject_backend,
                    &state,
                    &local_events_tx,
                    target,
                    request,
                )
                .await,
            )
        }
        DaemonRequest::RunLocalInputTest { test } => {
            let result =
                run_local_input_test(&inject_backend, &state, &local_events_tx, test).await;
            DaemonResponse::LocalInputTest(result)
        }
        DaemonRequest::RunRemoteLatencyTest { device_id } => {
            let result =
                run_remote_latency_test(&network_manager, &state, &local_events_tx, device_id)
                    .await;
            DaemonResponse::LocalInputTest(result)
        }
        DaemonRequest::RunRemoteUsbDescriptorProbe { device_id, bus_id } => {
            let result = run_remote_usb_descriptor_probe(
                &network_manager,
                &state,
                &local_events_tx,
                device_id,
                bus_id,
            )
            .await;
            DaemonResponse::UsbDescriptorProbe(result)
        }
        DaemonRequest::SetAudioDefaultOutput { endpoint_id } => {
            set_default_audio_output(&state, &local_events_tx, endpoint_id).await
        }
        DaemonRequest::SetAudioOutputVolume {
            endpoint_id,
            volume_percent,
        } => set_audio_output_volume(&state, &local_events_tx, endpoint_id, volume_percent).await,
        DaemonRequest::SetAudioOutputMute { endpoint_id, muted } => {
            set_audio_output_mute(&state, &local_events_tx, endpoint_id, muted).await
        }
        DaemonRequest::StartAudioCapture {
            source,
            endpoint_id,
        } => {
            start_audio_capture(
                &audio_runtime,
                &state,
                &local_events_tx,
                source,
                endpoint_id,
            )
            .await
        }
        DaemonRequest::StopAudioCapture => {
            stop_audio_capture(&audio_runtime, &state, &local_events_tx).await
        }
        DaemonRequest::StartAudioForwarding {
            source,
            endpoint_id,
        } => {
            start_audio_forwarding(
                &audio_runtime,
                &state,
                &network_manager,
                &local_events_tx,
                source,
                endpoint_id,
            )
            .await
        }
        DaemonRequest::StopAudioForwarding => {
            stop_audio_forwarding(&audio_runtime, &state, &network_manager, &local_events_tx).await
        }
        DaemonRequest::RunAudioTest { test: _ } => {
            DaemonResponse::LocalAudioTest(run_audio_test(&state).await)
        }
        DaemonRequest::CaptureDisplay(request) => {
            let result = rshare_platform::display::capture_display(&request);
            display_capture_response_from_result(&request, result)
        }
        DaemonRequest::IdentifyDisplays(request) => {
            let result = rshare_platform::display::identify_displays(&request);
            display_identify_response_from_result(result)
        }
        DaemonRequest::UpdateDisplaySettings(request) => {
            let result = rshare_platform::display::update_display_settings(&request);
            let refreshed_display = rshare_platform::display::query_display_state();
            let (response, event) = {
                let mut state = state.write().await;
                display_settings_update_response_from_result(
                    &mut state,
                    &request,
                    result,
                    refreshed_display,
                )
            };
            let _ = local_events_tx.send(event);
            response
        }
        DaemonRequest::OpenDisplaySettings => {
            match rshare_platform::display::open_display_settings() {
                Ok(()) => DaemonResponse::Ack,
                Err(error) => DaemonResponse::Error(error.to_string()),
            }
        }
        DaemonRequest::ListVirtualDisplays => {
            let platform_displays = rshare_platform::virtual_display::list_virtual_displays();
            let mut state = state.write().await;
            if let Ok(displays) = platform_displays {
                if state.virtual_displays.sync_platform_displays(displays) {
                    state.refresh_local_controls_platform();
                }
            }
            DaemonResponse::VirtualDisplays(state.virtual_displays.list())
        }
        DaemonRequest::CreateVirtualDisplay(request) => {
            let mut state = state.write().await;
            let result = state.virtual_displays.create(request);
            state.refresh_local_controls_platform();
            DaemonResponse::VirtualDisplayOperation(result)
        }
        DaemonRequest::RemoveVirtualDisplay(request) => {
            let mut state = state.write().await;
            let result = state.virtual_displays.remove(request);
            state.refresh_local_controls_platform();
            DaemonResponse::VirtualDisplayOperation(result)
        }
        DaemonRequest::SubscribeLocalControls
        | DaemonRequest::SubscribeEndpointEvents { .. }
        | DaemonRequest::SubscribeUiState { .. } => DaemonResponse::Error(
            "streaming subscriptions must be the first request on a dedicated connection"
                .to_string(),
        ),
        DaemonRequest::Shutdown => {
            let _ = shutdown_tx.send(());
            DaemonResponse::Ack
        }
    };

    Ok(response)
}

#[cfg(test)]
fn apply_layout_update(state: &mut DaemonState, mut layout: LayoutGraph) {
    layout.canonicalize_local_device(state.status.device_id);
    state.layout = layout;
}

#[cfg(all(test, windows))]
#[test]
fn windows_daemon_filter_capture_uses_cancellable_wait_event() {
    let source = include_str!("main.rs");
    let start = source
        .find("impl WindowsDriverCapture")
        .expect("missing Windows driver capture implementation");
    let end = source[start..]
        .find("impl Drop for WindowsDriverCapture")
        .map(|offset| start + offset)
        .expect("missing Windows capture drop implementation");
    let capture = &source[start..end];

    assert!(capture.contains("WindowsDriverEventStream::open_filter"));
    assert!(capture.contains(".wait_event()"));
    assert!(capture.contains(".cancel()"));
    assert!(!capture.contains(".read_event()"));
    assert!(!capture.contains("Duration::from_millis(1)"));
    assert!(capture.contains("sync_channel(0)"));
}

#[cfg(all(test, windows))]
#[test]
fn windows_capture_startup_rendezvous_disconnect_prevents_wait_entry() {
    let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let (open_complete_tx, open_complete_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let wait_entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_wait_entered = wait_entered.clone();
    let worker = std::thread::spawn(move || {
        open_complete_rx.recv().unwrap();
        if startup_tx.send(()).is_err() {
            return;
        }
        worker_wait_entered.store(true, std::sync::atomic::Ordering::Release);
    });

    drop(startup_rx);
    open_complete_tx.send(()).unwrap();
    worker.join().unwrap();
    assert!(!wait_entered.load(std::sync::atomic::Ordering::Acquire));
}

fn current_primary_screen_info() -> ScreenInfo {
    #[cfg(windows)]
    {
        if let Ok(display) = rshare_platform::display::query_display_state() {
            if let Some(primary) = display
                .displays
                .iter()
                .find(|display| display.primary)
                .or_else(|| display.displays.first())
            {
                return ScreenInfo::new(0, 0, primary.width, primary.height);
            }
        }
        let screen = rshare_platform::WindowsInputListener::get_screen_info();
        return ScreenInfo::new(0, 0, screen.width, screen.height);
    }

    #[cfg(target_os = "macos")]
    {
        let screen = rshare_platform::get_screen_info();
        return ScreenInfo::new(0, 0, screen.width, screen.height);
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        ScreenInfo::primary()
    }
}

fn default_local_only_layout(local_device: DeviceId) -> LayoutGraph {
    let local_screen = current_primary_screen_info();
    let mut layout = LayoutGraph::new(local_device);
    layout.add_node(LayoutNode::new(
        local_device,
        0,
        0,
        local_screen.width,
        local_screen.height,
    ));
    layout
}

fn invalid_layout_backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.invalid")
}

fn preserve_invalid_layout_file(path: &Path) {
    let backup_path = invalid_layout_backup_path(path);
    if backup_path.exists() {
        let _ = fs::remove_file(&backup_path);
    }

    if let Err(error) = fs::rename(path, &backup_path) {
        tracing::warn!(
            "Failed to preserve invalid layout file {} as {}: {}",
            path.display(),
            backup_path.display(),
            error
        );
    }
}

fn load_layout_from_path(local_device: DeviceId, path: impl AsRef<Path>) -> Result<LayoutGraph> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(default_local_only_layout(local_device));
    }

    let loaded = fs::read_to_string(path)
        .map_err(anyhow::Error::from)
        .and_then(|content| {
            serde_json::from_str::<LayoutGraph>(&content).map_err(anyhow::Error::from)
        });

    match loaded {
        Ok(mut layout) => {
            layout.canonicalize_local_device(local_device);
            Ok(layout)
        }
        Err(error) => {
            tracing::warn!(
                "Failed to load persisted layout from {}: {}. Falling back to local-only layout.",
                path.display(),
                error
            );
            preserve_invalid_layout_file(path);
            Ok(default_local_only_layout(local_device))
        }
    }
}

fn save_layout_to_path(layout: &LayoutGraph, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let encoded = serde_json::to_string_pretty(layout)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let tmp_path = path.with_extension(format!("json.tmp-{}", nanos));
    fs::write(&tmp_path, encoded)?;
    if let Err(error) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error.into());
    }
    Ok(())
}

fn load_config_with_env_overrides() -> Result<Config> {
    let mut config = load_config_or_fail_closed(Config::load);

    if let Ok(bind) = std::env::var("RSHARE_BIND") {
        config.network.bind_address = bind;
    }

    if let Ok(port) = std::env::var("RSHARE_PORT") {
        config.network.port = port.parse()?;
    }

    Ok(config)
}

fn load_config_or_fail_closed(load: impl FnOnce() -> Result<Config>) -> Config {
    match load() {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(
                "Failed to load persisted config: {}. Disabling automatic input and mobile gateway access.",
                error
            );
            let mut config = Config::default();
            config.features.automatic_input_forwarding = false;
            config.features.mobile_gateway_enabled = false;
            config
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct CountingUiProjection {
        calls: Arc<AtomicUsize>,
        snapshot: UiSnapshot,
    }

    impl UiProjectionSource for CountingUiProjection {
        fn project(&self) -> BoxFuture<'_, Result<UiSnapshot>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let snapshot = self.snapshot.clone();
            Box::pin(async move { Ok(snapshot) })
        }
    }

    #[derive(Clone)]
    struct FixedDaemonNetworkProjection {
        connections: Vec<ConnectionInfo>,
    }

    impl DaemonNetworkProjection for FixedDaemonNetworkProjection {
        fn connection_infos(&self) -> Vec<ConnectionInfo> {
            self.connections.clone()
        }
    }

    #[derive(Clone)]
    struct MutableDaemonNetworkProjection {
        connections: Arc<std::sync::RwLock<Vec<ConnectionInfo>>>,
        revision: Arc<AtomicUsize>,
        changed: watch::Sender<u64>,
    }

    impl MutableDaemonNetworkProjection {
        fn new(connections: Vec<ConnectionInfo>) -> Self {
            let (changed, _) = watch::channel(0);
            Self {
                connections: Arc::new(std::sync::RwLock::new(connections)),
                revision: Arc::new(AtomicUsize::new(0)),
                changed,
            }
        }

        fn changes(&self) -> watch::Receiver<u64> {
            self.changed.subscribe()
        }

        fn replace(&self, connections: Vec<ConnectionInfo>) {
            *self.connections.write().unwrap() = connections;
            let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
            self.changed.send_replace(revision as u64);
        }
    }

    impl DaemonNetworkProjection for MutableDaemonNetworkProjection {
        fn connection_infos(&self) -> Vec<ConnectionInfo> {
            self.connections.read().unwrap().clone()
        }
    }

    fn zero_control_metrics() -> ControlMetricSnapshot {
        ControlMetricSnapshot {
            captured: 0,
            routed: 0,
            realtime_replaced: 0,
            realtime_dropped: 0,
            reliable_overflow: 0,
        }
    }

    fn test_daemon_state() -> DaemonState {
        DaemonState::new(ServiceStatusSnapshot::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
            "0.0.0.0:27431".to_string(),
            27432,
            42,
        ))
    }

    fn test_injection_handle(backend: impl InjectBackend + 'static) -> InputInjectionHandle {
        InputInjectionHandle::spawn(Box::new(backend), InjectionActorConfig::default()).unwrap()
    }

    #[test]
    fn hot_input_messages_bypass_full_projection_reconcile() {
        let input = Message::InputDiagnostic {
            device_id: DeviceId::from_u128(7),
            event: LocalInputDiagnosticEvent {
                sequence: 1,
                timestamp_ms: 2,
                device_kind: LocalInputDeviceKind::Mouse,
                event_kind: "move".into(),
                summary: "pointer".into(),
                device_id: None,
                device_instance_id: None,
                capture_path: None,
                source: LocalInputEventSource::Hardware,
                payload: BTreeMap::new(),
            },
        };
        assert!(!network_message_may_mutate_ui(&input));
        assert!(network_message_may_mutate_ui(&Message::ScreenUpdate {
            screen_info: ScreenInfo::new(0, 0, 1920, 1080),
        }));
        assert!(request_may_mutate_ui(&DaemonRequest::Capabilities {
            device_id: None,
        }));
        assert!(request_may_mutate_ui(&DaemonRequest::LocalControls));
        assert!(request_may_mutate_ui(&DaemonRequest::ListVirtualDisplays));
    }

    #[tokio::test]
    async fn hot_input_classification_does_not_call_projection_but_low_frequency_change_does() {
        let snapshot = test_daemon_state().ui_snapshot();
        let calls = Arc::new(AtomicUsize::new(0));
        let projection = Arc::new(CountingUiProjection {
            calls: calls.clone(),
            snapshot: snapshot.clone(),
        });
        let (_input, feeds) = input_state_channel(4);
        let aggregator = StateAggregator::with_projection(snapshot, 8, feeds, projection);
        let input = Message::InputDiagnostic {
            device_id: DeviceId::from_u128(7),
            event: LocalInputDiagnosticEvent {
                sequence: 1,
                timestamp_ms: 2,
                device_kind: LocalInputDeviceKind::Mouse,
                event_kind: "move".into(),
                summary: "pointer".into(),
                device_id: None,
                device_instance_id: None,
                capture_path: None,
                source: LocalInputEventSource::Hardware,
                payload: BTreeMap::new(),
            },
        };
        reconcile_ui_state_if(&aggregator, network_message_may_mutate_ui(&input))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        let display = Message::ScreenUpdate {
            screen_info: ScreenInfo::new(0, 0, 1920, 1080),
        };
        reconcile_ui_state_if(&aggregator, network_message_may_mutate_ui(&display))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn fallback_status_and_local_controls_overlay_latest_input_stream_truth() {
        let state = test_daemon_state();
        let initial = state.ui_snapshot();
        let (input, feeds) = input_state_channel(8);
        let aggregator = StateAggregator::with_input(initial, 8, feeds);
        let target = DeviceId::from_u128(55);

        input.publish_pointer(rshare_daemon::input_state::InputPointerProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            x: 321,
            y: -42,
        });
        input.publish_discrete(rshare_daemon::input_state::InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            pressed_keys: vec![0x41],
            pressed_buttons: vec![rshare_core::MouseButton::Left],
        });
        input.publish_session(ControlSessionState::RemoteActive {
            target,
            entered_via: Direction::Right,
        });
        let latest = tokio::time::timeout(Duration::from_secs(1), aggregator.wait_for_revision(4))
            .await
            .expect("input projection did not reach fallback view")
            .unwrap();

        let DaemonResponse::LocalControls(controls) = overlay_stream_truth_onto_fallback_response(
            DaemonResponse::LocalControls(state.local_control_snapshot()),
            latest.as_ref(),
        ) else {
            panic!("expected fallback local-controls response");
        };
        assert_eq!((controls.mouse.x, controls.mouse.y), (321, -42));
        assert_eq!(controls.keyboard.pressed_keys, vec!["Raw(65)"]);
        assert_eq!(controls.mouse.pressed_buttons, vec!["Left"]);

        let DaemonResponse::Status(status) = overlay_stream_truth_onto_fallback_response(
            DaemonResponse::Status(state.status_snapshot()),
            latest.as_ref(),
        ) else {
            panic!("expected fallback status response");
        };
        assert_eq!(
            status.session_state,
            Some(ControlSessionState::RemoteActive {
                target,
                entered_via: Direction::Right,
            })
        );
        assert_eq!(status.active_target, Some(target));
    }

    #[tokio::test]
    async fn dedicated_local_controls_subscription_writes_latest_input_truth_first() {
        let state = Arc::new(RwLock::new(test_daemon_state()));
        let initial = state.read().await.ui_snapshot();
        let (input, feeds) = input_state_channel(8);
        let aggregator = StateAggregator::with_projection(
            initial,
            8,
            feeds,
            Arc::new(DaemonUiProjectionSource {
                state: state.clone(),
                network: Arc::new(FixedDaemonNetworkProjection {
                    connections: Vec::new(),
                }),
                diagnostics: None,
            }),
        );
        input.publish_pointer(rshare_daemon::input_state::InputPointerProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            x: 88,
            y: 99,
        });
        input.publish_discrete(rshare_daemon::input_state::InputDiscreteProjection {
            session_epoch: rshare_core::SessionEpoch(1),
            pressed_keys: vec![0x42],
            pressed_buttons: Vec::new(),
        });
        tokio::time::timeout(Duration::from_secs(1), aggregator.wait_for_revision(2))
            .await
            .expect("input projection did not reach dedicated fallback")
            .unwrap();

        let (mut server, mut client) = tokio::io::duplex(64 * 1024);
        write_local_controls_fallback_snapshot(&mut server, &state, &aggregator)
            .await
            .unwrap();
        let response: DaemonResponse = rshare_core::read_json_frame(&mut client).await.unwrap();
        let DaemonResponse::LocalControls(controls) = response else {
            panic!("dedicated subscription must begin with local controls");
        };
        assert_eq!((controls.mouse.x, controls.mouse.y), (88, 99));
        assert_eq!(controls.keyboard.pressed_keys, vec!["Raw(66)"]);
    }

    #[test]
    fn default_daemon_state_disables_mobile_access_without_credentials() {
        let snapshot = test_daemon_state().mobile_access.snapshot();

        assert!(!snapshot.enabled);
        assert_eq!(snapshot.page_url, "不可用");
        assert!(snapshot.token.is_empty());
    }

    #[test]
    fn config_load_failure_disables_automatic_input_and_mobile_gateway() {
        let config = load_config_or_fail_closed(|| anyhow::bail!("persisted config is unreadable"));

        assert!(!config.features.automatic_input_forwarding);
        assert!(!config.features.mobile_gateway_enabled);
    }

    #[test]
    fn successful_default_config_load_keeps_new_install_input_forwarding_enabled() {
        let config = load_config_or_fail_closed(|| Ok(Config::default()));

        assert!(config.features.automatic_input_forwarding);
        assert!(!config.features.mobile_gateway_enabled);
    }

    #[test]
    fn successful_config_load_preserves_explicitly_disabled_input_forwarding() {
        let mut persisted = Config::default();
        persisted.features.automatic_input_forwarding = false;

        let config = load_config_or_fail_closed(|| Ok(persisted));

        assert!(!config.features.automatic_input_forwarding);
        assert!(!config.features.mobile_gateway_enabled);
    }

    #[tokio::test]
    async fn daemon_shutdown_waits_for_mobile_cleanup_before_returning() {
        let (shutdown_tx, _) = broadcast::channel::<()>(4);
        let mut shutdown_rx = shutdown_tx.subscribe();
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut mobile_task = tokio::spawn(async move {
            events_tx.send("Pressed").unwrap();
            let _ = shutdown_rx.recv().await;
            tokio::task::yield_now().await;
            events_tx.send("Released").unwrap();
            Ok(())
        });

        assert_eq!(events_rx.recv().await, Some("Pressed"));
        complete_mobile_gateway_shutdown(&shutdown_tx, Some(&mut mobile_task))
            .await
            .unwrap();

        assert_eq!(events_rx.try_recv(), Ok("Released"));
    }

    #[tokio::test]
    async fn saturated_router_command_queue_eventually_accepts_latest_snapshot() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        tx.try_send(RouterCommand::BackendDegraded).unwrap();
        let layout = LayoutGraph::new(DeviceId::new_v4());
        let pending = tokio::spawn({
            let tx = tx.clone();
            let layout = layout.clone();
            async move { enqueue_router_command(&tx, RouterCommand::LayoutChanged(layout)).await }
        });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());

        assert!(matches!(
            rx.recv().await,
            Some(RouterCommand::BackendDegraded)
        ));
        pending
            .await
            .unwrap()
            .expect("latest layout snapshot must eventually be queued");
        assert!(matches!(
            rx.recv().await,
            Some(RouterCommand::LayoutChanged(applied)) if applied == layout
        ));
    }

    #[tokio::test]
    async fn closed_router_command_queue_returns_explicit_error() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);

        let error = enqueue_router_command(&tx, RouterCommand::Shutdown)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("channel is closed"));
    }

    #[test]
    fn backend_diagnostics_use_the_constructed_inject_backend_identity_and_capability() {
        let mut state = test_daemon_state();

        state.update_backend_state(
            Some(ResolvedInputMode::Portable),
            vec![BackendKind::Portable],
            BackendHealth::Healthy,
            BackendKind::WindowsNative,
            BackendHealth::Healthy,
            true,
            None,
        );

        assert_eq!(
            state.local_controls.inject_backend.mode,
            Some(ResolvedInputMode::Portable)
        );
        assert_eq!(
            state.local_controls.inject_backend.kind,
            Some(BackendKind::WindowsNative)
        );
        assert!(state.local_controls.inject_backend.text_commit_supported);
    }

    fn connected_connection_info(device_id: DeviceId, rtt_ms: Option<u64>) -> ConnectionInfo {
        let mut info = ConnectionInfo::new(device_id, "127.0.0.1:27431".to_string());
        info.state = rshare_net::connection::ConnectionState::Connected;
        info.datagram_available = true;
        info.rtt_ms = rtt_ms;
        info
    }

    fn fake_virtual_display_create_result(
        request: &VirtualDisplayCreateRequest,
        operation_status: VirtualDisplayOperationStatus,
        display_status: VirtualDisplayStatus,
    ) -> Result<VirtualDisplayOperationResult> {
        Ok(VirtualDisplayOperationResult {
            status: operation_status,
            display: Some(VirtualDisplaySnapshot {
                id: request.id.clone().expect("manager should normalize the id"),
                width: request.width,
                height: request.height,
                refresh_rate_millihz: request.refresh_rate_millihz.or(Some(60_000)),
                name: request.name.clone(),
                status: display_status,
                display_id: (display_status == VirtualDisplayStatus::Active)
                    .then(|| "windows-display-rshare".to_string()),
                message: None,
            }),
            message: None,
        })
    }

    fn fake_virtual_display_created(
        request: &VirtualDisplayCreateRequest,
    ) -> Result<VirtualDisplayOperationResult> {
        fake_virtual_display_create_result(
            request,
            VirtualDisplayOperationStatus::Created,
            VirtualDisplayStatus::Active,
        )
    }

    fn fake_virtual_display_driver_unavailable(
        request: &VirtualDisplayCreateRequest,
    ) -> Result<VirtualDisplayOperationResult> {
        fake_virtual_display_create_result(
            request,
            VirtualDisplayOperationStatus::DriverUnavailable,
            VirtualDisplayStatus::DriverUnavailable,
        )
    }

    #[test]
    fn virtual_display_manager_skips_platform_create_for_matching_active_display() {
        let mut manager = VirtualDisplayManager::default();
        let platform_create_calls = Cell::new(0);
        let request = VirtualDisplayCreateRequest {
            id: Some("vd-1".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        };

        let first = manager.create_with(request.clone(), |request| {
            platform_create_calls.set(platform_create_calls.get() + 1);
            fake_virtual_display_created(request)
        });
        let duplicate = manager.create_with(request, |_| {
            platform_create_calls.set(platform_create_calls.get() + 1);
            panic!("matching active display must not call the platform")
        });

        assert_eq!(first.status, VirtualDisplayOperationStatus::Created);
        assert_eq!(
            duplicate.status,
            VirtualDisplayOperationStatus::AlreadyExists
        );
        assert_eq!(platform_create_calls.get(), 1);
    }

    #[test]
    fn virtual_display_manager_removes_snapshot_through_platform_callback() {
        let mut manager = VirtualDisplayManager::default();
        let _ = manager.sync_platform_displays(vec![VirtualDisplaySnapshot {
            id: "vd-1".to_string(),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: None,
            status: VirtualDisplayStatus::Active,
            display_id: Some("windows-display-rshare".to_string()),
            message: None,
        }]);
        let platform_remove_calls = Cell::new(0);

        let result = manager.remove_with(
            VirtualDisplayRemoveRequest {
                id: "vd-1".to_string(),
            },
            |request| {
                platform_remove_calls.set(platform_remove_calls.get() + 1);
                Ok(VirtualDisplayOperationResult {
                    status: VirtualDisplayOperationStatus::Removed,
                    display: Some(VirtualDisplaySnapshot {
                        id: request.id.clone(),
                        width: 1920,
                        height: 1080,
                        refresh_rate_millihz: Some(60_000),
                        name: None,
                        status: VirtualDisplayStatus::Removed,
                        display_id: None,
                        message: None,
                    }),
                    message: None,
                })
            },
        );

        assert_eq!(result.status, VirtualDisplayOperationStatus::Removed);
        assert_eq!(platform_remove_calls.get(), 1);
        assert!(manager.list().is_empty());
    }

    #[test]
    fn virtual_display_manager_rejects_empty_remove_id_without_platform_call() {
        let mut manager = VirtualDisplayManager::default();

        let result = manager.remove_with(
            VirtualDisplayRemoveRequest {
                id: "  ".to_string(),
            },
            |_| panic!("empty virtual display id must not call the platform"),
        );

        assert_eq!(result.status, VirtualDisplayOperationStatus::Failed);
        assert!(manager.list().is_empty());
    }

    #[test]
    fn virtual_display_manager_records_platform_create_result() {
        let mut manager = VirtualDisplayManager::default();

        let result = manager.create_with(
            rshare_core::VirtualDisplayCreateRequest {
                id: Some("vd-1".to_string()),
                width: 1920,
                height: 1080,
                refresh_rate_millihz: Some(60_000),
                name: Some("R-ShareMouse Virtual Display".to_string()),
            },
            fake_virtual_display_created,
        );

        assert_eq!(result.status, VirtualDisplayOperationStatus::Created);
        assert_eq!(manager.list().len(), 1);
        assert_eq!(manager.list()[0].id, "vd-1");
    }

    #[test]
    fn virtual_display_manager_rejects_duplicate_ids() {
        let mut manager = VirtualDisplayManager::default();
        let request = rshare_core::VirtualDisplayCreateRequest {
            id: Some("vd-1".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: None,
        };

        let _ = manager.sync_platform_displays(vec![VirtualDisplaySnapshot {
            id: "vd-1".to_string(),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: None,
            status: rshare_core::VirtualDisplayStatus::Active,
            display_id: Some("windows-display-rshare".to_string()),
            message: None,
        }]);
        let result = manager.create_with(request, |_| {
            panic!("matching active display must not call the platform")
        });

        assert_eq!(
            result.status,
            rshare_core::VirtualDisplayOperationStatus::AlreadyExists
        );
        assert_eq!(manager.list().len(), 1);
    }

    #[test]
    fn virtual_display_manager_allows_active_display_mode_update() {
        let mut manager = VirtualDisplayManager::default();
        let _ = manager.sync_platform_displays(vec![VirtualDisplaySnapshot {
            id: "vd-1".to_string(),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
            status: rshare_core::VirtualDisplayStatus::Active,
            display_id: Some("windows-display-rshare".to_string()),
            message: None,
        }]);

        let result = manager.create_with(
            rshare_core::VirtualDisplayCreateRequest {
                id: Some("vd-1".to_string()),
                width: 2560,
                height: 1440,
                refresh_rate_millihz: Some(144_000),
                name: Some("R-ShareMouse Virtual Display".to_string()),
            },
            fake_virtual_display_created,
        );

        assert_eq!(result.status, VirtualDisplayOperationStatus::Created);
        assert_eq!(manager.list()[0].width, 2560);
        assert_eq!(manager.list()[0].height, 1440);
    }

    #[test]
    fn virtual_display_manager_retries_after_driver_unavailable_result() {
        let mut manager = VirtualDisplayManager::default();
        let platform_create_calls = Cell::new(0);
        let request = rshare_core::VirtualDisplayCreateRequest {
            id: Some("vd-1".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        };

        let first = manager.create_with(request.clone(), |request| {
            platform_create_calls.set(platform_create_calls.get() + 1);
            fake_virtual_display_driver_unavailable(request)
        });
        let retry = manager.create_with(request, |request| {
            platform_create_calls.set(platform_create_calls.get() + 1);
            fake_virtual_display_driver_unavailable(request)
        });

        assert_eq!(
            first.status,
            rshare_core::VirtualDisplayOperationStatus::DriverUnavailable
        );
        assert_eq!(retry.status, first.status);
        assert_eq!(platform_create_calls.get(), 2);
        assert_eq!(manager.list().len(), 1);
        assert_eq!(manager.list()[0].id, "vd-1");
    }

    #[test]
    fn virtual_display_manager_reuses_default_id_after_existing_default_record() {
        let mut manager = VirtualDisplayManager::default();

        let first = manager.create_with(
            rshare_core::VirtualDisplayCreateRequest {
                id: None,
                width: 1920,
                height: 1080,
                refresh_rate_millihz: Some(60_000),
                name: Some("R-ShareMouse Virtual Display".to_string()),
            },
            fake_virtual_display_created,
        );
        let retry = manager.create_with(
            rshare_core::VirtualDisplayCreateRequest {
                id: None,
                width: 2560,
                height: 1440,
                refresh_rate_millihz: Some(144_000),
                name: Some("R-ShareMouse Virtual Display".to_string()),
            },
            fake_virtual_display_created,
        );

        assert_eq!(
            first.display.as_ref().map(|display| display.id.as_str()),
            Some("rshare-vdisplay-1")
        );
        assert_eq!(
            retry.display.as_ref().map(|display| display.id.as_str()),
            Some("rshare-vdisplay-1")
        );
        assert_eq!(manager.list().len(), 1);
    }

    #[test]
    fn virtual_display_manager_rejects_invalid_modes() {
        let mut manager = VirtualDisplayManager::default();

        let result = manager.create_with(
            rshare_core::VirtualDisplayCreateRequest {
                id: Some("vd-invalid".to_string()),
                width: 0,
                height: 1080,
                refresh_rate_millihz: Some(60_000),
                name: None,
            },
            |_| panic!("invalid mode must not call the platform"),
        );

        assert_eq!(
            result.status,
            rshare_core::VirtualDisplayOperationStatus::InvalidMode
        );
        assert!(manager.list().is_empty());
    }

    #[test]
    fn virtual_display_manager_syncs_platform_visible_displays_into_list() {
        let mut manager = VirtualDisplayManager::default();
        let changed = manager.sync_platform_displays(vec![VirtualDisplaySnapshot {
            id: "rshare-vdisplay-1".to_string(),
            width: 2560,
            height: 1440,
            refresh_rate_millihz: Some(144_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
            status: rshare_core::VirtualDisplayStatus::Active,
            display_id: Some("windows-idd-connector-0".to_string()),
            message: None,
        }]);

        assert!(changed);
        let displays = manager.list();
        assert_eq!(displays.len(), 1);
        assert_eq!(
            displays[0].status,
            rshare_core::VirtualDisplayStatus::Active
        );
        assert_eq!(
            displays[0].display_id.as_deref(),
            Some("windows-idd-connector-0")
        );
    }

    #[test]
    fn virtual_display_manager_sync_reports_manual_mode_changes() {
        let mut manager = VirtualDisplayManager::default();
        let _ = manager.sync_platform_displays(vec![VirtualDisplaySnapshot {
            id: "rshare-vdisplay-1".to_string(),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
            status: rshare_core::VirtualDisplayStatus::Active,
            display_id: Some("windows-display-rshare".to_string()),
            message: None,
        }]);

        let changed = manager.sync_platform_displays(vec![VirtualDisplaySnapshot {
            id: "rshare-vdisplay-1".to_string(),
            width: 2560,
            height: 1440,
            refresh_rate_millihz: Some(144_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
            status: rshare_core::VirtualDisplayStatus::Active,
            display_id: Some("windows-display-rshare".to_string()),
            message: None,
        }]);

        assert!(changed);
        let displays = manager.list();
        assert_eq!(displays[0].width, 2560);
        assert_eq!(displays[0].height, 1440);
        assert_eq!(displays[0].refresh_rate_millihz, Some(144_000));
    }

    #[test]
    fn virtual_display_manager_sync_marks_missing_active_platform_display_removed() {
        let mut manager = VirtualDisplayManager::default();
        let _ = manager.sync_platform_displays(vec![VirtualDisplaySnapshot {
            id: "rshare-vdisplay-1".to_string(),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
            status: rshare_core::VirtualDisplayStatus::Active,
            display_id: Some("windows-display-rshare".to_string()),
            message: None,
        }]);

        let changed = manager.sync_platform_displays(Vec::new());

        assert!(changed);
        let displays = manager.list();
        assert_eq!(
            displays[0].status,
            rshare_core::VirtualDisplayStatus::Removed
        );
        assert_eq!(displays[0].display_id, None);
    }

    #[test]
    fn virtual_display_manager_sync_marks_missing_pending_platform_display_removed() {
        let mut manager = VirtualDisplayManager::default();
        let _ = manager.sync_platform_displays(vec![VirtualDisplaySnapshot {
            id: "rshare-vdisplay-1".to_string(),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
            status: rshare_core::VirtualDisplayStatus::Pending,
            display_id: None,
            message: Some("waiting for IddCx arrival".to_string()),
        }]);

        let changed = manager.sync_platform_displays(Vec::new());

        assert!(changed);
        let displays = manager.list();
        assert_eq!(
            displays[0].status,
            rshare_core::VirtualDisplayStatus::Removed
        );
        assert_eq!(displays[0].display_id, None);
    }

    #[test]
    fn capability_registry_snapshot_contains_local_reserved_capabilities() {
        let mut state = test_daemon_state();
        state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);
        state.backend_state.update_aggregate_health();
        state.local_controls.display.display_count = 1;

        let snapshot =
            state.capability_registry_snapshot(&NetworkTransportSnapshot::default(), None);

        let local = snapshot
            .devices
            .iter()
            .find(|device| device.device_id == snapshot.local_device_id)
            .expect("local device capability snapshot");
        for kind in [
            rshare_core::EndpointCapabilityKind::Input,
            rshare_core::EndpointCapabilityKind::Clipboard,
            rshare_core::EndpointCapabilityKind::Gamepad,
            rshare_core::EndpointCapabilityKind::Audio,
            rshare_core::EndpointCapabilityKind::DisplayTopology,
            rshare_core::EndpointCapabilityKind::UsbHost,
            rshare_core::EndpointCapabilityKind::UsbReceiver,
            rshare_core::EndpointCapabilityKind::PrivilegedHelper,
            rshare_core::EndpointCapabilityKind::Diagnostics,
        ] {
            assert!(
                local
                    .capabilities
                    .iter()
                    .any(|capability| capability.kind == kind),
                "missing reserved capability {kind:?}"
            );
        }
    }

    #[test]
    fn capability_registry_snapshot_includes_discovered_peer_and_filters_by_device() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        let mut advertised = DeviceCapabilities::default();
        advertised.supports_audio_forwarding = true;
        advertised.supports_usb_forwarding_experimental = true;
        state.upsert_discovered(DiscoveredDevice {
            id: remote_id,
            name: "remote".to_string(),
            hostname: "remote-host".to_string(),
            addresses: vec!["127.0.0.1:27431".parse().unwrap()],
            screen_info: None,
            capabilities: advertised,
            transport_capabilities: rshare_core::PeerTransportCapabilities::required_v3(),
            protocol_compatibility: rshare_net::PeerProtocolCompatibility::Compatible,
            last_seen: Instant::now(),
        });
        state.mark_connected(&remote_id, true);

        let snapshot = state
            .capability_registry_snapshot(&NetworkTransportSnapshot::default(), Some(remote_id));

        assert_eq!(snapshot.devices.len(), 1);
        assert_eq!(snapshot.devices[0].device_id, remote_id);
        assert!(snapshot.devices[0].capabilities.iter().any(|capability| {
            capability.kind == rshare_core::EndpointCapabilityKind::UsbHost
                && capability.state == rshare_core::CapabilityState::Experimental
        }));
    }

    #[test]
    fn capability_registry_snapshot_reports_multi_display_layout_details() {
        let mut state = test_daemon_state();
        let local_id = state.status.device_id;
        let remote_id = DeviceId::new_v4();
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                discovered: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        state.layout.add_node(LayoutNode {
            device_id: remote_id,
            displays: vec![
                rshare_core::DisplayNode::primary(1920, 0, 2560, 1440),
                rshare_core::DisplayNode::secondary("remote-left".to_string(), 0, 0, 1920, 1080),
            ],
        });

        let snapshot = state
            .capability_registry_snapshot(&NetworkTransportSnapshot::default(), Some(remote_id));
        let remote = snapshot
            .devices
            .first()
            .expect("remote capability snapshot");
        let display = remote
            .capabilities
            .iter()
            .find(|capability| {
                capability.kind == rshare_core::EndpointCapabilityKind::DisplayTopology
            })
            .expect("display topology capability");

        assert_eq!(snapshot.local_device_id, local_id);
        assert_eq!(
            display.details.get("display_count").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            display
                .details
                .get("primary_display_id")
                .map(String::as_str),
            Some("primary")
        );
        assert_eq!(
            display
                .details
                .get("display_geometries")
                .map(String::as_str),
            Some("primary:2560x1440@1920,0:primary;remote-left:1920x1080@0,0")
        );
    }

    #[test]
    fn display_scale_update_result_is_nonfatal_and_refreshes_layout() {
        let mut state = test_daemon_state();
        let request = rshare_core::DisplaySettingsUpdateRequest {
            display_id: "primary".to_string(),
            width: None,
            height: None,
            refresh_rate_millihz: None,
            orientation: None,
            primary: None,
            x: None,
            y: None,
            scale_percent: Some(125),
        };

        let refreshed_display = LocalDisplayState {
            display_count: 2,
            virtual_x: 0,
            virtual_y: 0,
            primary_width: 1920,
            primary_height: 1080,
            layout_width: 3200,
            layout_height: 1080,
            displays: vec![
                LocalDisplayInfo {
                    display_id: "primary".to_string(),
                    width: 1920,
                    height: 1080,
                    primary: true,
                    active: true,
                    ..LocalDisplayInfo::default()
                },
                LocalDisplayInfo {
                    display_id: "right".to_string(),
                    x: 1920,
                    width: 1280,
                    height: 720,
                    active: true,
                    ..LocalDisplayInfo::default()
                },
            ],
        };

        let (response, event) = display_settings_update_response_from_result(
            &mut state,
            &request,
            Ok(DisplaySettingsUpdateResult {
                status: DisplayOperationStatus::RequiresSystemSettings,
                message: Some("scale requires system settings".to_string()),
            }),
            Ok(refreshed_display),
        );

        match response {
            DaemonResponse::DisplaySettingsUpdated(result) => {
                assert_eq!(
                    result.status,
                    DisplayOperationStatus::RequiresSystemSettings
                );
            }
            other => panic!("expected display settings result, got {other:?}"),
        }
        let local_node = state
            .layout
            .get_node(state.status.device_id)
            .expect("local layout node");
        assert_eq!(local_node.displays.len(), 2);
        assert_eq!(local_node.displays[1].display_id, "right");
        assert_eq!(event.device_kind, LocalInputDeviceKind::Display);
        assert_eq!(event.event_kind, "settings_update");
        assert_eq!(
            event.payload.get("status").map(String::as_str),
            Some("RequiresSystemSettings")
        );
    }

    #[test]
    fn display_capture_platform_error_returns_structured_result() {
        let request = rshare_core::DisplayCaptureRequest {
            display_id: "display-1".to_string(),
            max_width: Some(640),
            format: rshare_core::DisplayCaptureFormat::Png,
        };

        let response = display_capture_response_from_result(
            &request,
            Err(anyhow::anyhow!("capture backend failed")),
        );

        match response {
            DaemonResponse::DisplayCapture(result) => {
                assert_eq!(result.status, DisplayOperationStatus::ApplyFailed);
                assert!(result.payload.is_none());
                assert!(result.blob.is_none());
                assert_eq!(result.message.as_deref(), Some("capture backend failed"));
            }
            other => panic!("expected display capture result, got {other:?}"),
        }
    }

    #[test]
    fn failed_display_capture_never_emits_a_binary_frame() {
        let mut invalid = rshare_platform::display_capture::success(
            "display-1",
            "image/png",
            1,
            1,
            vec![0x89, b'P', b'N', b'G'].into(),
            "captured",
        );
        invalid.status = DisplayOperationStatus::ApplyFailed;
        invalid.message = Some("capture failed".to_string());

        let (result, binary) = prepare_display_capture_response(invalid);

        assert_eq!(result.status, DisplayOperationStatus::ApplyFailed);
        assert!(result.payload.is_none());
        assert!(result.blob.is_none());
        assert!(binary.is_none());
        assert_eq!(
            result.message.as_deref(),
            Some("invalid failed display capture payload")
        );
    }

    #[test]
    fn identify_displays_platform_error_returns_structured_result() {
        let response =
            display_identify_response_from_result(Err(anyhow::anyhow!("overlay backend failed")));

        match response {
            DaemonResponse::DisplayIdentify(result) => {
                assert_eq!(result.status, DisplayOperationStatus::ApplyFailed);
                assert_eq!(result.message.as_deref(), Some("overlay backend failed"));
            }
            other => panic!("expected display identify result, got {other:?}"),
        }
    }

    #[test]
    fn latency_payload_subtracts_remote_processing_time() {
        let mut payload = BTreeMap::new();
        let (network_round_trip_ms, raw_round_trip_ms) =
            add_latency_measurement_payload(&mut payload, 100, 100, 112, 118, 140);

        assert_eq!(raw_round_trip_ms, 40);
        assert_eq!(network_round_trip_ms, 34);
        assert_eq!(
            payload.get("estimated_one_way_ms").map(String::as_str),
            Some("17")
        );
        assert_eq!(
            payload.get("remote_processing_ms").map(String::as_str),
            Some("6")
        );
    }

    #[test]
    fn transport_feedback_reports_unavailable_without_connections() {
        let network = network_snapshot_from_connections(&[]);
        let feedback = transport_feedback_from_connections(&network, &[]);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Unavailable);
        assert!(feedback.realtime_degraded);
    }

    #[test]
    fn transport_feedback_reports_healthy_realtime_connection() {
        let connection = connected_connection_info(DeviceId::new_v4(), Some(12));
        let connections = [connection];
        let network = network_snapshot_from_connections(&connections);

        let feedback = transport_feedback_from_connections(&network, &connections);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
        assert_eq!(feedback.rtt_ms, Some(12));
    }

    #[test]
    fn transport_feedback_degrades_when_realtime_is_degraded() {
        let mut connection = connected_connection_info(DeviceId::new_v4(), Some(22));
        connection.datagram_available = false;
        let connections = [connection];
        let network = network_snapshot_from_connections(&connections);

        let feedback = transport_feedback_from_connections(&network, &connections);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Degraded);
    }

    #[test]
    fn transport_feedback_degrades_without_rtt_measurement() {
        let connection = connected_connection_info(DeviceId::new_v4(), None);
        let connections = [connection];
        let network = network_snapshot_from_connections(&connections);

        let feedback = transport_feedback_from_connections(&network, &connections);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Degraded);
    }

    #[test]
    fn remote_latency_feedback_reports_pending_probe() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);
        state.pending_latency_probes.insert(
            7,
            PendingLatencyProbe {
                target: remote_id,
                sent_at_ms: 1000,
                role: PendingLatencyProbeRole::LocalRequested,
            },
        );

        let feedback = state.remote_latency_feedback(1250);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices.len(), 1);
        let device = &feedback.devices[0];
        assert_eq!(device.device_id, remote_id);
        assert_eq!(device.status, LatencyFeedbackStatus::Pending);
        assert_eq!(device.latest_sequence, Some(7));
        assert_eq!(device.last_probe_sent_ms, Some(1000));
        assert_eq!(device.pending_duration_ms, Some(250));
    }

    #[test]
    fn remote_latency_feedback_prefers_same_millisecond_newer_pending_probe() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);

        let mut payload = BTreeMap::new();
        payload.insert("target_device_id".to_string(), remote_id.to_string());
        payload.insert("network_round_trip_ms".to_string(), "24".to_string());
        record_latency_diagnostic_event(
            &mut state,
            remote_id,
            "latency_probe_ack",
            "Latency to remote: 24 ms RTT / ~12 ms one-way",
            payload,
        );
        let ack = state
            .local_controls
            .recent_events
            .last_mut()
            .expect("latency ACK event");
        ack.timestamp_ms = 1000;
        ack.sequence = 7;
        state.local_controls.sequence = 7;
        state.pending_latency_probes.insert(
            8,
            PendingLatencyProbe {
                target: remote_id,
                sent_at_ms: 1000,
                role: PendingLatencyProbeRole::LocalRequested,
            },
        );

        let feedback = state.remote_latency_feedback(1100);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices.len(), 1);
        assert_eq!(feedback.devices[0].status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices[0].pending_duration_ms, Some(100));
    }

    #[test]
    fn remote_latency_feedback_prefers_newer_pending_over_delayed_old_ack() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);

        let mut payload = BTreeMap::new();
        payload.insert("target_device_id".to_string(), remote_id.to_string());
        payload.insert("probe_sequence".to_string(), "7".to_string());
        payload.insert("network_round_trip_ms".to_string(), "24".to_string());
        record_latency_diagnostic_event(
            &mut state,
            remote_id,
            "latency_probe_ack",
            "Latency to remote: 24 ms RTT / ~12 ms one-way",
            payload,
        );
        let ack = state
            .local_controls
            .recent_events
            .last_mut()
            .expect("latency ACK event");
        ack.timestamp_ms = 1100;
        ack.sequence = 9;
        state.local_controls.sequence = 9;
        state.pending_latency_probes.insert(
            8,
            PendingLatencyProbe {
                target: remote_id,
                sent_at_ms: 1000,
                role: PendingLatencyProbeRole::LocalRequested,
            },
        );

        let feedback = state.remote_latency_feedback(1150);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices.len(), 1);
        assert_eq!(feedback.devices[0].status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices[0].pending_duration_ms, Some(150));
    }

    #[test]
    fn remote_latency_feedback_uses_origin_probe_sequence_for_endpoint_switch_ack_ordering() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);

        let mut payload = BTreeMap::new();
        payload.insert("target_device_id".to_string(), remote_id.to_string());
        payload.insert("origin_device_id".to_string(), remote_id.to_string());
        payload.insert("probe_sequence".to_string(), "99".to_string());
        payload.insert("origin_probe_sequence".to_string(), "7".to_string());
        payload.insert("network_round_trip_ms".to_string(), "24".to_string());
        record_latency_diagnostic_event(
            &mut state,
            remote_id,
            "latency_endpoint_switch_ack",
            "Endpoint-side latency to remote: 24 ms RTT / ~12 ms one-way",
            payload,
        );
        let ack = state
            .local_controls
            .recent_events
            .last_mut()
            .expect("latency endpoint switch ACK event");
        ack.timestamp_ms = 1100;
        ack.sequence = 9;
        state.local_controls.sequence = 9;
        state.pending_latency_probes.insert(
            8,
            PendingLatencyProbe {
                target: remote_id,
                sent_at_ms: 1000,
                role: PendingLatencyProbeRole::LocalRequested,
            },
        );

        let feedback = state.remote_latency_feedback(1150);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices.len(), 1);
        assert_eq!(feedback.devices[0].status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices[0].pending_duration_ms, Some(150));
    }

    #[test]
    fn remote_latency_feedback_uses_endpoint_probe_sequence_for_endpoint_switch_pending() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);

        let mut payload = BTreeMap::new();
        payload.insert("target_device_id".to_string(), remote_id.to_string());
        payload.insert("origin_device_id".to_string(), remote_id.to_string());
        payload.insert("probe_sequence".to_string(), "20".to_string());
        payload.insert("origin_probe_sequence".to_string(), "1000".to_string());
        payload.insert("network_round_trip_ms".to_string(), "24".to_string());
        record_latency_diagnostic_event(
            &mut state,
            remote_id,
            "latency_endpoint_switch_ack",
            "Old endpoint-side latency sample",
            payload,
        );
        let ack = state
            .local_controls
            .recent_events
            .last_mut()
            .expect("latency endpoint switch ACK event");
        ack.timestamp_ms = 1100;
        ack.sequence = 20;
        state.local_controls.sequence = 20;
        state.pending_latency_probes.insert(
            21,
            PendingLatencyProbe {
                target: remote_id,
                sent_at_ms: 1125,
                role: PendingLatencyProbeRole::EndpointSwitchReport {
                    origin_device_id: remote_id,
                    origin_sequence: 1001,
                },
            },
        );

        let feedback = state.remote_latency_feedback(1175);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices.len(), 1);
        assert_eq!(feedback.devices[0].status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices[0].latest_sequence, Some(21));
        assert_eq!(feedback.devices[0].pending_duration_ms, Some(50));
    }

    #[test]
    fn remote_latency_feedback_does_not_use_origin_sequence_fallback_for_endpoint_pending() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);

        let mut payload = BTreeMap::new();
        payload.insert("target_device_id".to_string(), remote_id.to_string());
        payload.insert("origin_device_id".to_string(), remote_id.to_string());
        payload.insert("origin_probe_sequence".to_string(), "1000".to_string());
        payload.insert("network_round_trip_ms".to_string(), "24".to_string());
        record_latency_diagnostic_event(
            &mut state,
            remote_id,
            "latency_endpoint_switch_ack",
            "Malformed endpoint-side latency sample",
            payload,
        );
        let ack = state
            .local_controls
            .recent_events
            .last_mut()
            .expect("latency endpoint switch ACK event");
        ack.timestamp_ms = 1100;
        ack.sequence = 20;
        state.local_controls.sequence = 20;
        state.pending_latency_probes.insert(
            21,
            PendingLatencyProbe {
                target: remote_id,
                sent_at_ms: 1125,
                role: PendingLatencyProbeRole::EndpointSwitchReport {
                    origin_device_id: remote_id,
                    origin_sequence: 1001,
                },
            },
        );

        let feedback = state.remote_latency_feedback(1175);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices.len(), 1);
        assert_eq!(feedback.devices[0].status, LatencyFeedbackStatus::Pending);
        assert_eq!(feedback.devices[0].latest_sequence, Some(21));
        assert_eq!(feedback.devices[0].pending_duration_ms, Some(50));
    }

    #[test]
    fn remote_latency_feedback_uses_local_sequence_when_remote_ack_timestamps_are_skewed() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);

        let old_ack = LocalInputDiagnosticEvent {
            sequence: 40,
            timestamp_ms: 20_000,
            device_kind: LocalInputDeviceKind::Backend,
            event_kind: "latency_endpoint_switch_ack".to_string(),
            summary: "Old endpoint latency sample".to_string(),
            device_id: Some(remote_id.to_string()),
            device_instance_id: None,
            capture_path: Some("rshare-net".to_string()),
            source: LocalInputEventSource::System,
            payload: BTreeMap::from([
                ("origin_device_id".to_string(), remote_id.to_string()),
                ("origin_probe_sequence".to_string(), "40".to_string()),
                ("network_round_trip_ms".to_string(), "90".to_string()),
            ]),
        };
        record_remote_diagnostic_event(&mut state, remote_id, old_ack);

        let new_ack = LocalInputDiagnosticEvent {
            sequence: 41,
            timestamp_ms: 1_000,
            device_kind: LocalInputDeviceKind::Backend,
            event_kind: "latency_endpoint_switch_ack".to_string(),
            summary: "New endpoint latency sample".to_string(),
            device_id: Some(remote_id.to_string()),
            device_instance_id: None,
            capture_path: Some("rshare-net".to_string()),
            source: LocalInputEventSource::System,
            payload: BTreeMap::from([
                ("origin_device_id".to_string(), remote_id.to_string()),
                ("origin_probe_sequence".to_string(), "41".to_string()),
                ("network_round_trip_ms".to_string(), "24".to_string()),
            ]),
        };
        record_remote_diagnostic_event(&mut state, remote_id, new_ack);

        let feedback = state.remote_latency_feedback(21_000);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
        assert_eq!(feedback.devices.len(), 1);
        assert_eq!(feedback.devices[0].status, LatencyFeedbackStatus::Healthy);
        assert_eq!(feedback.devices[0].network_round_trip_ms, Some(24));
        assert_eq!(
            feedback.devices[0].summary.as_deref(),
            Some("New endpoint latency sample")
        );
    }

    #[test]
    fn remote_latency_feedback_uses_local_sequence_across_ack_sequence_domains() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);

        let normal_ack = LocalInputDiagnosticEvent {
            sequence: 100,
            timestamp_ms: 1_000,
            device_kind: LocalInputDeviceKind::Backend,
            event_kind: "latency_probe_ack".to_string(),
            summary: "Old normal latency sample".to_string(),
            device_id: Some(remote_id.to_string()),
            device_instance_id: None,
            capture_path: Some("rshare-net".to_string()),
            source: LocalInputEventSource::System,
            payload: BTreeMap::from([
                ("target_device_id".to_string(), remote_id.to_string()),
                ("probe_sequence".to_string(), "100".to_string()),
                ("network_round_trip_ms".to_string(), "24".to_string()),
            ]),
        };
        push_recent_local_event(&mut state.local_controls, normal_ack);

        let endpoint_ack = LocalInputDiagnosticEvent {
            sequence: 102,
            timestamp_ms: 1_100,
            device_kind: LocalInputDeviceKind::Backend,
            event_kind: "latency_endpoint_switch_ack".to_string(),
            summary: "New endpoint latency sample".to_string(),
            device_id: Some(remote_id.to_string()),
            device_instance_id: None,
            capture_path: Some("rshare-net".to_string()),
            source: LocalInputEventSource::System,
            payload: BTreeMap::from([
                ("origin_device_id".to_string(), remote_id.to_string()),
                ("probe_sequence".to_string(), "101".to_string()),
                ("origin_probe_sequence".to_string(), "7".to_string()),
                ("network_round_trip_ms".to_string(), "30".to_string()),
            ]),
        };
        push_recent_local_event(&mut state.local_controls, endpoint_ack);
        state.local_controls.sequence = 102;

        let feedback = state.remote_latency_feedback(1_200);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
        assert_eq!(feedback.devices.len(), 1);
        assert_eq!(feedback.devices[0].latest_sequence, Some(102));
        assert_eq!(feedback.devices[0].network_round_trip_ms, Some(30));
        assert_eq!(
            feedback.devices[0].summary.as_deref(),
            Some("New endpoint latency sample")
        );
    }

    #[test]
    fn remote_latency_feedback_reports_timeout_for_stale_pending_probe() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);
        state.pending_latency_probes.insert(
            7,
            PendingLatencyProbe {
                target: remote_id,
                sent_at_ms: 1000,
                role: PendingLatencyProbeRole::LocalRequested,
            },
        );

        let feedback = state.remote_latency_feedback(3000);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Timeout);
        assert_eq!(feedback.devices.len(), 1);
        assert_eq!(feedback.devices[0].status, LatencyFeedbackStatus::Timeout);
        assert_eq!(feedback.devices[0].pending_duration_ms, Some(2000));
    }

    #[test]
    fn remote_latency_pending_probes_are_cleared_when_device_is_removed() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);
        state.pending_latency_probes.insert(
            7,
            PendingLatencyProbe {
                target: remote_id,
                sent_at_ms: 1000,
                role: PendingLatencyProbeRole::LocalRequested,
            },
        );

        state.remove_device(&remote_id);

        assert!(!state
            .pending_latency_probes
            .values()
            .any(|probe| probe.target == remote_id));

        state.mark_connected(&remote_id, true);
        let feedback = state.remote_latency_feedback(3000);

        assert_eq!(feedback.status, LatencyFeedbackStatus::Idle);
        assert_eq!(feedback.devices.len(), 1);
        assert_eq!(feedback.devices[0].status, LatencyFeedbackStatus::Idle);
    }

    #[test]
    fn remote_latency_feedback_reports_ack_metrics() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.mark_connected(&remote_id, true);
        let mut payload = BTreeMap::new();
        payload.insert("target_device_id".to_string(), remote_id.to_string());
        payload.insert("network_round_trip_ms".to_string(), "24".to_string());
        payload.insert("raw_round_trip_ms".to_string(), "30".to_string());
        payload.insert("estimated_one_way_ms".to_string(), "12".to_string());
        payload.insert("remote_processing_ms".to_string(), "6".to_string());
        payload.insert("direction".to_string(), "origin_to_endpoint".to_string());
        let event = record_latency_diagnostic_event(
            &mut state,
            remote_id,
            "latency_probe_ack",
            "Latency to remote: 24 ms RTT / ~12 ms one-way",
            payload,
        );

        let feedback = state.remote_latency_feedback(timestamp_ms_now());

        assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
        assert_eq!(feedback.devices.len(), 1);
        let device = &feedback.devices[0];
        assert_eq!(device.status, LatencyFeedbackStatus::Healthy);
        assert_eq!(device.latest_sequence, Some(event.sequence));
        assert_eq!(device.last_ack_ms, Some(event.timestamp_ms));
        assert_eq!(device.network_round_trip_ms, Some(24));
        assert_eq!(device.raw_round_trip_ms, Some(30));
        assert_eq!(device.estimated_one_way_ms, Some(12));
        assert_eq!(device.remote_processing_ms, Some(6));
        assert_eq!(device.direction.as_deref(), Some("origin_to_endpoint"));
        assert_eq!(
            device.summary.as_deref(),
            Some("Latency to remote: 24 ms RTT / ~12 ms one-way")
        );
    }

    #[test]
    fn remote_latency_feedback_degrades_ack_missing_or_high_rtt() {
        let mut state = test_daemon_state();
        let missing_rtt_id = DeviceId::new_v4();
        let high_rtt_id = DeviceId::new_v4();
        state.mark_connected(&missing_rtt_id, true);
        state.mark_connected(&high_rtt_id, true);

        record_latency_diagnostic_event(
            &mut state,
            missing_rtt_id,
            "latency_probe_ack",
            "Latency probe missing RTT",
            BTreeMap::new(),
        );

        let mut high_rtt_payload = BTreeMap::new();
        high_rtt_payload.insert("target_device_id".to_string(), high_rtt_id.to_string());
        high_rtt_payload.insert("network_round_trip_ms".to_string(), "51".to_string());
        record_latency_diagnostic_event(
            &mut state,
            high_rtt_id,
            "latency_probe_ack",
            "Latency probe high RTT",
            high_rtt_payload,
        );

        let feedback = state.remote_latency_feedback(timestamp_ms_now());

        assert_eq!(feedback.status, LatencyFeedbackStatus::Degraded);
        let missing = feedback
            .devices
            .iter()
            .find(|device| device.device_id == missing_rtt_id)
            .expect("missing RTT device feedback");
        assert_eq!(missing.status, LatencyFeedbackStatus::Degraded);
        assert_eq!(missing.network_round_trip_ms, None);
        let high = feedback
            .devices
            .iter()
            .find(|device| device.device_id == high_rtt_id)
            .expect("high RTT device feedback");
        assert_eq!(high.status, LatencyFeedbackStatus::Degraded);
        assert_eq!(high.network_round_trip_ms, Some(51));
    }

    #[test]
    fn remote_latency_feedback_reports_disconnected_device_unavailable() {
        let mut state = test_daemon_state();
        let remote_id = DeviceId::new_v4();
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: Vec::new(),
                connected: false,
                discovered: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );

        let feedback = state.remote_latency_feedback(timestamp_ms_now());

        assert_eq!(feedback.status, LatencyFeedbackStatus::Unavailable);
        assert_eq!(feedback.devices.len(), 1);
        assert_eq!(feedback.devices[0].device_id, remote_id);
        assert_eq!(feedback.devices[0].device_name.as_deref(), Some("remote"));
        assert_eq!(
            feedback.devices[0].status,
            LatencyFeedbackStatus::Unavailable
        );
        assert_eq!(feedback.devices[0].network_round_trip_ms, None);
        assert_eq!(feedback.devices[0].pending_duration_ms, None);
    }

    #[test]
    fn usb_descriptor_probe_parses_device_descriptor() {
        let bytes = [
            18, 1, 0x10, 0x02, 0xff, 0x01, 0x02, 64, 0x5e, 0x04, 0x8e, 0x02, 0x00, 0x01, 1, 2, 3, 1,
        ];
        let descriptor = usb_device_descriptor_from_bytes("usb:1-2", &bytes).unwrap();

        assert_eq!(descriptor.bus_id, "usb:1-2");
        assert_eq!(descriptor.vendor_id, 0x045e);
        assert_eq!(descriptor.product_id, 0x028e);
        assert_eq!(descriptor.class_code, 0xff);
        assert_eq!(descriptor.subclass_code, 0x01);
        assert_eq!(descriptor.protocol_code, 0x02);
        assert_eq!(descriptor.usb_version_bcd, 0x0210);
        assert_eq!(descriptor.device_version_bcd, 0x0100);
    }

    #[test]
    fn local_input_event_updates_diagnostic_snapshot() {
        let mut state = test_daemon_state();

        let event = rshare_input::InputEvent::key(
            rshare_input::KeyCode::ShiftLeft,
            rshare_input::ButtonState::Pressed,
        );
        let diagnostic = state.record_local_input_event(&event);

        assert_eq!(diagnostic.sequence, 1);
        assert_eq!(diagnostic.device_kind, LocalInputDeviceKind::Keyboard);
        assert_eq!(state.local_controls.sequence, 1);
        assert!(state.local_controls.keyboard.detected);
        assert_eq!(
            state.local_controls.keyboard.last_key.as_deref(),
            Some("ShiftLeft")
        );
        assert_eq!(
            state.local_controls.keyboard.pressed_keys,
            vec!["ShiftLeft".to_string()]
        );
        assert_eq!(state.local_controls.recent_events.len(), 1);

        state.record_local_input_event(&rshare_input::InputEvent::key(
            rshare_input::KeyCode::ShiftLeft,
            rshare_input::ButtonState::Released,
        ));
        assert!(state.local_controls.keyboard.pressed_keys.is_empty());
        assert_eq!(state.local_controls.keyboard.event_count, 2);
    }

    #[test]
    fn local_input_event_attributes_single_physical_keyboard() {
        let mut state = test_daemon_state();
        state.local_controls.keyboard_devices.clear();
        state
            .local_controls
            .keyboard_devices
            .push(rshare_core::LocalHardwareDevice {
                id: "linux-evdev-keyboard-event3".to_string(),
                name: "Built-in Keyboard".to_string(),
                source: "Linux evdev".to_string(),
                connected: true,
                device_instance_id: Some("event3".to_string()),
                capture_path: Some("/dev/input/event3".to_string()),
                ..Default::default()
            });

        let diagnostic = state.record_local_input_event(&rshare_input::InputEvent::key(
            rshare_input::KeyCode::ShiftLeft,
            rshare_input::ButtonState::Pressed,
        ));

        assert_eq!(
            diagnostic.device_id.as_deref(),
            Some("linux-evdev-keyboard-event3")
        );
        assert_eq!(diagnostic.device_instance_id.as_deref(), Some("event3"));
        assert_eq!(
            diagnostic.capture_path.as_deref(),
            Some("/dev/input/event3")
        );
        assert_eq!(
            diagnostic.payload.get("device_id").map(String::as_str),
            Some("linux-evdev-keyboard-event3")
        );
        assert_eq!(state.local_controls.keyboard_devices[0].event_count, 1);
        assert_eq!(
            state.local_controls.keyboard_devices[0].last_event_ms,
            diagnostic.timestamp_ms
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn evdev_capture_metadata_attributes_matching_physical_mouse() {
        let captured = captured_input_from_evdev_driver_event(
            rshare_platform::EvdevDriverEvent::MouseButton {
                button: 1,
                pressed: true,
                device_path: "/dev/input/event9".to_string(),
            },
        )
        .expect("mouse button should convert");
        assert_eq!(
            captured
                .metadata
                .as_ref()
                .map(|metadata| metadata.device_id.as_str()),
            Some("linux-evdev-mouse-event9")
        );

        let mut state = test_daemon_state();
        state.local_controls.mouse_devices.clear();
        state
            .local_controls
            .mouse_devices
            .push(rshare_core::LocalHardwareDevice {
                id: "linux-evdev-mouse-event8".to_string(),
                name: "External Mouse".to_string(),
                source: "Linux evdev".to_string(),
                connected: true,
                device_instance_id: Some("event8".to_string()),
                capture_path: Some("/dev/input/event8".to_string()),
                ..Default::default()
            });
        state
            .local_controls
            .mouse_devices
            .push(rshare_core::LocalHardwareDevice {
                id: "linux-evdev-mouse-event9".to_string(),
                name: "Touchpad".to_string(),
                source: "Linux evdev".to_string(),
                connected: true,
                device_instance_id: Some("event9".to_string()),
                capture_path: Some("/dev/input/event9".to_string()),
                ..Default::default()
            });

        let diagnostic = state
            .record_local_input_event_with_metadata(&captured.event, captured.metadata.as_ref());

        assert_eq!(
            diagnostic.device_id.as_deref(),
            Some("linux-evdev-mouse-event9")
        );
        assert_eq!(state.local_controls.mouse_devices[0].event_count, 0);
        assert_eq!(state.local_controls.mouse_devices[1].event_count, 1);
        assert_eq!(
            state.local_controls.mouse_devices[1].last_event_ms,
            diagnostic.timestamp_ms
        );
    }

    #[test]
    fn local_input_feedback_is_idle_when_backend_is_healthy_without_events() {
        let mut state = test_daemon_state();
        state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);

        let feedback = state.local_input_feedback();

        assert_eq!(feedback.status, LatencyFeedbackStatus::Idle);
        assert_eq!(feedback.event_count, 0);
    }

    #[test]
    fn local_input_feedback_uses_latest_keyboard_and_mouse_events() {
        let mut state = test_daemon_state();
        state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);
        state.record_local_input_event(&rshare_input::InputEvent::key(
            rshare_input::KeyCode::ShiftLeft,
            rshare_input::ButtonState::Pressed,
        ));
        state.record_local_input_event(&rshare_input::InputEvent::mouse_move(10, 20));

        let feedback = state.local_input_feedback();

        assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
        assert_eq!(feedback.event_count, 2);
        assert_eq!(feedback.latest_sequence, Some(2));
        assert!(feedback.latest_keyboard_event_ms.is_some());
        assert!(feedback.latest_mouse_event_ms.is_some());
    }

    #[test]
    fn local_input_feedback_uses_latest_gamepad_event() {
        let mut state = test_daemon_state();
        state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);
        let mut gamepad = rshare_core::GamepadState::neutral(0, 1, timestamp_ms_now());
        gamepad.buttons.push(rshare_core::GamepadButtonState {
            button: rshare_core::GamepadButton::South,
            pressed: true,
        });

        let event =
            state.record_local_input_event(&rshare_input::InputEvent::gamepad_state(gamepad));
        let feedback = state.local_input_feedback();

        assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
        assert_eq!(feedback.event_count, 1);
        assert_eq!(feedback.latest_sequence, Some(event.sequence));
        assert_eq!(feedback.latest_event_ms, Some(event.timestamp_ms));
        assert_eq!(feedback.latest_gamepad_event_ms, Some(event.timestamp_ms));
        assert_eq!(feedback.latest_gamepad_id, Some(0));
        assert_eq!(feedback.latest_gamepad_event_kind.as_deref(), Some("state"));
        assert!(feedback
            .latest_gamepad_button
            .as_deref()
            .is_some_and(|value| { value.contains("South") }));
    }

    #[test]
    fn local_input_feedback_event_count_includes_gamepads() {
        let mut state = test_daemon_state();
        state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);
        state.local_controls.keyboard.event_count = 2;
        state.local_controls.mouse.event_count = 3;
        let mut gamepad = rshare_core::LocalGamepadState {
            gamepad_id: 0,
            name: "Pad 0".to_string(),
            connected: true,
            buttons: Vec::new(),
            pressed_buttons: Vec::new(),
            last_button: None,
            left_stick_x: 0,
            left_stick_y: 0,
            right_stick_x: 0,
            right_stick_y: 0,
            left_trigger: 0,
            right_trigger: 0,
            event_count: 4,
            button_event_count: 0,
            button_press_count: 0,
            button_release_count: 0,
            axis_event_count: 0,
            trigger_event_count: 0,
            last_axis: None,
            last_seen_ms: 0,
        };
        state.local_controls.gamepads.push(gamepad.clone());
        gamepad.gamepad_id = 1;
        gamepad.name = "Pad 1".to_string();
        gamepad.event_count = 5;
        state.local_controls.gamepads.push(gamepad);

        let feedback = state.local_input_feedback();

        assert_eq!(feedback.event_count, 14);
    }

    #[test]
    fn local_input_feedback_ignores_later_backend_diagnostic_for_latest_input() {
        let mut state = test_daemon_state();
        state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);
        state.record_local_input_event(&rshare_input::InputEvent::key(
            rshare_input::KeyCode::ShiftLeft,
            rshare_input::ButtonState::Pressed,
        ));
        let keyboard_event = state.local_controls.recent_events.last_mut().unwrap();
        keyboard_event.capture_path = Some("portable-capture".to_string());
        let keyboard_sequence = keyboard_event.sequence;
        let keyboard_timestamp_ms = keyboard_event.timestamp_ms;

        let backend_event = LocalInputDiagnosticEvent {
            sequence: keyboard_sequence.saturating_add(1),
            timestamp_ms: keyboard_timestamp_ms.saturating_add(1),
            device_kind: LocalInputDeviceKind::Backend,
            event_kind: "latency".to_string(),
            summary: "Network latency sample".to_string(),
            device_id: None,
            device_instance_id: None,
            capture_path: Some("rshare-net".to_string()),
            source: LocalInputEventSource::System,
            payload: BTreeMap::new(),
        };
        state.local_controls.sequence = backend_event.sequence;
        push_recent_local_event(&mut state.local_controls, backend_event);

        let feedback = state.local_input_feedback();

        assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
        assert_eq!(feedback.event_count, 1);
        assert_eq!(feedback.latest_sequence, Some(keyboard_sequence));
        assert_eq!(feedback.latest_event_ms, Some(keyboard_timestamp_ms));
        assert_eq!(feedback.capture_path.as_deref(), Some("portable-capture"));
    }

    #[test]
    fn local_input_feedback_ignores_remote_keyboard_diagnostic_for_latest_input() {
        let mut state = test_daemon_state();
        state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);
        state.record_local_input_event(&rshare_input::InputEvent::key(
            rshare_input::KeyCode::ShiftLeft,
            rshare_input::ButtonState::Pressed,
        ));
        let keyboard_event = state.local_controls.recent_events.last_mut().unwrap();
        keyboard_event.capture_path = Some("portable-capture".to_string());
        let keyboard_sequence = keyboard_event.sequence;
        let keyboard_timestamp_ms = keyboard_event.timestamp_ms;

        let remote_device_id = DeviceId::new_v4();
        let mut payload = BTreeMap::new();
        payload.insert("remote_device_id".to_string(), remote_device_id.to_string());
        let remote_event = LocalInputDiagnosticEvent {
            sequence: keyboard_sequence.saturating_add(1),
            timestamp_ms: keyboard_timestamp_ms.saturating_add(1),
            device_kind: LocalInputDeviceKind::Keyboard,
            event_kind: "key".to_string(),
            summary: "Remote key ShiftLeft Pressed".to_string(),
            device_id: Some(remote_device_id.to_string()),
            device_instance_id: None,
            capture_path: Some("remote-daemon".to_string()),
            source: LocalInputEventSource::System,
            payload,
        };
        state.local_controls.sequence = remote_event.sequence;
        push_recent_local_event(&mut state.local_controls, remote_event);

        let feedback = state.local_input_feedback();

        assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
        assert_eq!(feedback.event_count, 1);
        assert_eq!(feedback.latest_sequence, Some(keyboard_sequence));
        assert_eq!(feedback.latest_event_ms, Some(keyboard_timestamp_ms));
        assert_eq!(
            feedback.latest_keyboard_event_ms,
            Some(keyboard_timestamp_ms)
        );
        assert_eq!(feedback.capture_path.as_deref(), Some("portable-capture"));
    }

    #[test]
    fn local_input_feedback_is_unavailable_without_selected_backend() {
        let mut state = test_daemon_state();
        state.record_local_input_event(&rshare_input::InputEvent::key(
            rshare_input::KeyCode::ShiftLeft,
            rshare_input::ButtonState::Pressed,
        ));

        let feedback = state.local_input_feedback();

        assert_eq!(feedback.status, LatencyFeedbackStatus::Unavailable);
        assert_eq!(feedback.event_count, 1);
    }

    #[test]
    fn local_input_feedback_is_degraded_when_selected_backend_is_degraded() {
        let mut state = test_daemon_state();
        state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);
        state.backend_state.aggregate_health = BackendHealth::Degraded {
            reason: BackendFailureReason::RuntimeError,
        };
        let event = state.record_local_input_event(&rshare_input::InputEvent::key(
            rshare_input::KeyCode::ShiftLeft,
            rshare_input::ButtonState::Pressed,
        ));

        let feedback = state.local_input_feedback();

        assert_eq!(feedback.status, LatencyFeedbackStatus::Degraded);
        assert_eq!(feedback.event_count, 1);
        assert_eq!(feedback.latest_sequence, Some(event.sequence));
        assert_eq!(feedback.latest_event_ms, Some(event.timestamp_ms));
        assert_eq!(feedback.latest_keyboard_event_ms, Some(event.timestamp_ms));
    }

    #[test]
    fn local_input_feedback_saturates_event_count() {
        let mut state = test_daemon_state();
        state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);
        state.local_controls.keyboard.event_count = 1;
        state.local_controls.mouse.event_count = 1;
        state
            .local_controls
            .gamepads
            .push(rshare_core::LocalGamepadState {
                gamepad_id: 0,
                name: "Pad".to_string(),
                connected: true,
                buttons: Vec::new(),
                pressed_buttons: Vec::new(),
                last_button: None,
                left_stick_x: 0,
                left_stick_y: 0,
                right_stick_x: 0,
                right_stick_y: 0,
                left_trigger: 0,
                right_trigger: 0,
                event_count: u64::MAX,
                button_event_count: 0,
                button_press_count: 0,
                button_release_count: 0,
                axis_event_count: 0,
                trigger_event_count: 0,
                last_axis: None,
                last_seen_ms: 0,
            });

        let feedback = state.local_input_feedback();

        assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
        assert_eq!(feedback.event_count, u64::MAX);
    }

    #[test]
    fn endpoint_events_project_from_local_diagnostics() {
        let mut state = test_daemon_state();

        state.record_local_input_event(&rshare_input::InputEvent::key(
            rshare_input::KeyCode::ShiftLeft,
            rshare_input::ButtonState::Pressed,
        ));

        let events = state.endpoint_events(
            &EndpointEventFilter {
                endpoint_id: Some(state.status.device_id),
                kinds: vec![rshare_core::EndpointEventKind::Keyboard],
                include_loopback: true,
                ..EndpointEventFilter::default()
            },
            None,
            Some(8),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].endpoint_id, state.status.device_id);
        assert_eq!(events[0].origin_endpoint_id, state.status.device_id);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(
            events[0].device.attribution,
            rshare_core::DeviceAttribution::Aggregate
        );
    }

    #[test]
    fn remote_endpoint_delta_is_mirrored_under_remote_endpoint() {
        let mut state = test_daemon_state();
        let remote = DeviceId::new_v4();
        let remote_event = rshare_core::EndpointEvent {
            event_id: 7,
            sequence: 7,
            timestamp_ms: 42,
            endpoint_id: remote,
            origin_endpoint_id: remote,
            device: rshare_core::EndpointDeviceRef {
                device_id: "keyboard-default".to_string(),
                instance_id: None,
                display_name: "Aggregate Keyboard".to_string(),
                kind: rshare_core::EndpointEventKind::Keyboard,
                attribution: rshare_core::DeviceAttribution::Aggregate,
            },
            direction: rshare_core::EndpointEventDirection::Observed,
            source: rshare_core::EndpointEventSource::Hardware,
            kind: rshare_core::EndpointEventKind::Keyboard,
            payload: rshare_core::EndpointEventPayload::Keyboard {
                key: "A".to_string(),
                state: "Pressed".to_string(),
            },
            correlation_id: None,
        };

        let mirrored = state.mirror_remote_endpoint_event(remote, remote_event);

        assert_eq!(mirrored.endpoint_id, remote);
        assert_eq!(mirrored.origin_endpoint_id, remote);
        assert_eq!(
            mirrored.source,
            rshare_core::EndpointEventSource::RemoteMirror
        );
        let events = state.endpoint_events(
            &EndpointEventFilter {
                endpoint_id: Some(remote),
                kinds: vec![rshare_core::EndpointEventKind::Keyboard],
                ..EndpointEventFilter::default()
            },
            None,
            Some(8),
        );
        assert_eq!(events, vec![mirrored]);
    }

    #[test]
    fn local_mouse_event_updates_diagnostic_snapshot() {
        let mut state = test_daemon_state();
        state.local_controls.display = LocalDisplayState {
            display_count: 2,
            virtual_x: -1280,
            virtual_y: 0,
            primary_width: 1920,
            primary_height: 1080,
            layout_width: 3200,
            layout_height: 1080,
            displays: vec![
                LocalDisplayInfo {
                    display_id: "left".to_string(),
                    x: -1280,
                    y: 0,
                    width: 1280,
                    height: 720,
                    primary: false,
                    ..LocalDisplayInfo::default()
                },
                LocalDisplayInfo {
                    display_id: "primary".to_string(),
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    primary: true,
                    ..LocalDisplayInfo::default()
                },
            ],
        };

        let diagnostic =
            state.record_local_input_event(&rshare_input::InputEvent::mouse_move(-1200, 34));

        assert_eq!(diagnostic.device_kind, LocalInputDeviceKind::Mouse);
        assert_eq!(state.local_controls.mouse.x, -1200);
        assert_eq!(state.local_controls.mouse.y, 34);
        assert_eq!(state.local_controls.mouse.event_count, 1);
        assert_eq!(state.local_controls.mouse.move_count, 1);
        assert_eq!(
            state.local_controls.mouse.current_display_id.as_deref(),
            Some("left")
        );
        assert_eq!(state.local_controls.mouse.display_relative_x, 80);
        assert_eq!(diagnostic.payload["display_id"], "left");
        state.record_local_input_event(&rshare_input::InputEvent::mouse_button(
            rshare_input::MouseButton::Forward,
            rshare_input::ButtonState::Pressed,
        ));
        state.record_local_input_event(&rshare_input::InputEvent::mouse_wheel(1, -2));
        assert_eq!(
            state.local_controls.mouse.pressed_buttons,
            vec!["Forward".to_string()]
        );
        assert_eq!(state.local_controls.mouse.button_press_count, 1);
        assert_eq!(state.local_controls.mouse.wheel_event_count, 1);
        assert_eq!(state.local_controls.mouse.wheel_total_x, 1);
        assert_eq!(state.local_controls.mouse.wheel_total_y, -2);
        assert_eq!(
            state.local_controls.recent_events[0].summary,
            "Mouse move -1200, 34"
        );
    }

    #[test]
    fn recent_local_events_preserve_mouse_button_under_move_flood() {
        let mut state = test_daemon_state();

        state.record_local_input_event(&rshare_input::InputEvent::mouse_button(
            rshare_input::MouseButton::Left,
            rshare_input::ButtonState::Pressed,
        ));
        state.record_local_input_event(&rshare_input::InputEvent::mouse_button(
            rshare_input::MouseButton::Left,
            rshare_input::ButtonState::Released,
        ));

        for index in 0..100 {
            state.record_local_input_event(&rshare_input::InputEvent::mouse_move(index, index));
        }

        assert!(
            state
                .local_controls
                .recent_events
                .iter()
                .any(|event| event.device_kind == LocalInputDeviceKind::Mouse
                    && event.event_kind == "button"
                    && event.payload.get("button").map(String::as_str) == Some("Left")),
            "mouse button events should remain visible in recent_events after move flood"
        );
    }

    #[test]
    fn local_layout_geometry_binds_multiple_displays_into_one_device_node() {
        let mut state = test_daemon_state();
        let local_id = state.status.device_id;
        state.local_controls.display = LocalDisplayState {
            display_count: 2,
            virtual_x: 0,
            virtual_y: 0,
            primary_width: 2560,
            primary_height: 1440,
            layout_width: 5120,
            layout_height: 1440,
            displays: vec![
                LocalDisplayInfo {
                    display_id: "primary".to_string(),
                    x: 0,
                    y: 0,
                    width: 2560,
                    height: 1440,
                    primary: true,
                    ..LocalDisplayInfo::default()
                },
                LocalDisplayInfo {
                    display_id: "display-2".to_string(),
                    x: 2560,
                    y: 0,
                    width: 2560,
                    height: 1440,
                    primary: false,
                    ..LocalDisplayInfo::default()
                },
            ],
        };

        assert!(state.reconcile_local_layout_geometry());

        let local_node = state.layout.get_node(local_id).expect("local layout node");
        assert_eq!(local_node.displays.len(), 2);
        assert!(local_node.displays[0].primary);
        assert_eq!(local_node.displays[0].x, 0);
        assert_eq!(local_node.displays[0].width, 2560);
        assert_eq!(local_node.displays[1].display_id, "display-2");
        assert_eq!(local_node.displays[1].x, 2560);
        assert_eq!(local_node.displays[1].height, 1440);
    }

    #[test]
    fn fallback_display_state_marks_primary_display_active() {
        let display = fallback_display_state(1920, 1080);

        assert_eq!(display.displays.len(), 1);
        assert!(display.displays[0].active);
    }

    #[test]
    fn local_gamepad_event_updates_diagnostic_snapshot() {
        let mut state = test_daemon_state();

        let first = rshare_core::GamepadState {
            gamepad_id: 0,
            sequence: 1,
            buttons: vec![rshare_core::GamepadButtonState {
                button: rshare_core::GamepadButton::South,
                pressed: true,
            }],
            left_stick_x: 1200,
            left_stick_y: -2400,
            right_stick_x: 0,
            right_stick_y: 0,
            left_trigger: 128,
            right_trigger: 0,
            timestamp_ms: 100,
        };
        let diagnostic =
            state.record_local_input_event(&rshare_input::InputEvent::gamepad_state(first));

        assert_eq!(diagnostic.device_kind, LocalInputDeviceKind::Gamepad);
        assert_eq!(diagnostic.payload["pressed_buttons"], "South");
        let gamepad = &state.local_controls.gamepads[0];
        assert!(gamepad.connected);
        assert_eq!(gamepad.pressed_buttons, vec!["South".to_string()]);
        assert_eq!(gamepad.button_press_count, 1);
        assert_eq!(gamepad.button_release_count, 0);
        assert_eq!(gamepad.axis_event_count, 1);
        assert_eq!(gamepad.trigger_event_count, 1);

        let second = rshare_core::GamepadState {
            gamepad_id: 0,
            sequence: 2,
            buttons: vec![
                rshare_core::GamepadButtonState {
                    button: rshare_core::GamepadButton::South,
                    pressed: false,
                },
                rshare_core::GamepadButtonState {
                    button: rshare_core::GamepadButton::East,
                    pressed: true,
                },
            ],
            left_stick_x: 0,
            left_stick_y: 0,
            right_stick_x: 500,
            right_stick_y: -500,
            left_trigger: 0,
            right_trigger: 255,
            timestamp_ms: 200,
        };
        let diagnostic =
            state.record_local_input_event(&rshare_input::InputEvent::gamepad_state(second));

        let gamepad = &state.local_controls.gamepads[0];
        assert_eq!(gamepad.pressed_buttons, vec!["East".to_string()]);
        assert_eq!(gamepad.button_event_count, 3);
        assert_eq!(gamepad.button_press_count, 2);
        assert_eq!(gamepad.button_release_count, 1);
        assert_eq!(gamepad.axis_event_count, 2);
        assert_eq!(gamepad.trigger_event_count, 2);
        assert_eq!(gamepad.event_count, 2);
        assert_eq!(diagnostic.payload["button_press_count"], "2");
        assert_eq!(diagnostic.payload["pressed_buttons"], "East");
    }

    #[derive(Debug)]
    struct TestInjectBackend {
        active: bool,
        fail: bool,
        injected: Vec<rshare_input::InputEvent>,
    }

    impl InjectBackend for TestInjectBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Portable
        }

        fn health(&self) -> BackendHealth {
            if self.active {
                BackendHealth::Healthy
            } else {
                BackendHealth::Degraded {
                    reason: BackendFailureReason::Unavailable,
                }
            }
        }

        fn inject(&mut self, event: rshare_input::InputEvent) -> Result<()> {
            if self.fail {
                anyhow::bail!("test injection failed");
            }
            self.injected.push(event);
            Ok(())
        }

        fn is_active(&self) -> bool {
            self.active
        }
    }

    #[derive(Debug)]
    struct RecordingKindInjectBackend {
        kind: BackendKind,
        injected: Arc<std::sync::Mutex<Vec<rshare_input::InputEvent>>>,
    }

    impl InjectBackend for RecordingKindInjectBackend {
        fn kind(&self) -> BackendKind {
            self.kind
        }

        fn health(&self) -> BackendHealth {
            BackendHealth::Healthy
        }

        fn inject(&mut self, event: rshare_input::InputEvent) -> Result<()> {
            self.injected.lock().unwrap().push(event);
            Ok(())
        }

        fn is_active(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn run_local_input_test_reports_success_and_broadcasts_feedback() {
        let backend = test_injection_handle(TestInjectBackend {
            active: true,
            fail: false,
            injected: Vec::new(),
        });
        let state = Arc::new(RwLock::new(test_daemon_state()));
        let (events, mut rx) = broadcast::channel(4);

        let result = run_local_input_test(
            &backend,
            &state,
            &events,
            LocalInputTestRequest {
                kind: LocalInputTestKind::KeyboardShift,
            },
        )
        .await;

        assert_eq!(result.status, LocalInputTestStatus::Success);
        let event = rx.recv().await.unwrap();
        assert_eq!(event.source, LocalInputEventSource::InjectedLoopback);
        assert_eq!(event.device_kind, LocalInputDeviceKind::Keyboard);
        assert_eq!(state.read().await.local_controls.recent_events.len(), 1);
    }

    #[tokio::test]
    async fn virtual_hid_mouse_test_uses_absolute_round_trip_coordinates() {
        let injected = Arc::new(std::sync::Mutex::new(Vec::new()));
        let backend = test_injection_handle(RecordingKindInjectBackend {
            kind: BackendKind::VirtualHid,
            injected: injected.clone(),
        });
        let mut daemon = test_daemon_state();
        daemon.local_controls.mouse.x = 500;
        daemon.local_controls.mouse.y = 300;
        let state = Arc::new(RwLock::new(daemon));
        let (events, mut rx) = broadcast::channel(4);

        let result = run_local_input_test(
            &backend,
            &state,
            &events,
            LocalInputTestRequest {
                kind: LocalInputTestKind::MouseMove,
            },
        )
        .await;

        assert_eq!(result.status, LocalInputTestStatus::Success);
        let injected = injected.lock().unwrap();
        assert_eq!(injected.len(), 2);
        assert!(matches!(
            injected[0],
            rshare_input::InputEvent::MouseMove { x: 508, y: 308 }
        ));
        assert!(matches!(
            injected[1],
            rshare_input::InputEvent::MouseMove { x: 500, y: 300 }
        ));
        let event = rx.recv().await.unwrap();
        assert_eq!(event.payload.get("x").map(String::as_str), Some("508"));
        assert_eq!(event.payload.get("y").map(String::as_str), Some("308"));
        assert_eq!(
            event.payload.get("return_x").map(String::as_str),
            Some("500")
        );
        assert_eq!(
            event.payload.get("return_y").map(String::as_str),
            Some("300")
        );
    }

    #[tokio::test]
    async fn injected_test_marks_immediate_capture_feedback_as_loopback() {
        let state = Arc::new(RwLock::new(test_daemon_state()));

        record_injected_test_event(&state, LocalInputTestKind::KeyboardShift, BTreeMap::new())
            .await;
        let mut state = state.write().await;
        let feedback = state.record_local_input_event(&rshare_input::InputEvent::key(
            rshare_input::KeyCode::ShiftLeft,
            rshare_input::ButtonState::Pressed,
        ));

        assert_eq!(feedback.source, LocalInputEventSource::InjectedLoopback);
        assert_eq!(
            feedback.payload.get("source_note").map(String::as_str),
            Some("possible daemon injection loopback")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_driver_source_keeps_pending_loopback_classification() {
        assert_eq!(
            resolve_windows_driver_local_source(
                LocalInputEventSource::InjectedLoopback,
                LocalInputEventSource::Hardware,
            ),
            LocalInputEventSource::InjectedLoopback
        );
        assert_eq!(
            resolve_windows_driver_local_source(
                LocalInputEventSource::Hardware,
                LocalInputEventSource::DriverTest,
            ),
            LocalInputEventSource::DriverTest
        );
    }

    #[tokio::test]
    async fn run_local_input_test_reports_backend_unavailable() {
        let backend = test_injection_handle(TestInjectBackend {
            active: false,
            fail: false,
            injected: Vec::new(),
        });
        let state = Arc::new(RwLock::new(test_daemon_state()));
        let (events, _rx) = broadcast::channel(4);

        let result = run_local_input_test(
            &backend,
            &state,
            &events,
            LocalInputTestRequest {
                kind: LocalInputTestKind::MouseMove,
            },
        )
        .await;

        assert_eq!(result.status, LocalInputTestStatus::BackendUnavailable);
    }

    #[tokio::test]
    async fn inject_endpoint_event_reports_result_and_correlated_loopback() {
        let backend = test_injection_handle(TestInjectBackend {
            active: true,
            fail: false,
            injected: Vec::new(),
        });
        let state = Arc::new(RwLock::new(test_daemon_state()));
        let network_manager = Arc::new(Mutex::new(NetworkManager::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local".to_string(),
        )));
        let (events, _rx) = broadcast::channel(4);
        let correlation_id = "ipc-shift-1".to_string();

        let result = inject_endpoint_event(
            &network_manager,
            &backend,
            &state,
            &events,
            rshare_core::EndpointInjectTarget::Local,
            rshare_core::EndpointInjectRequest {
                correlation_id: correlation_id.clone(),
                device_kind: rshare_core::EndpointEventKind::Keyboard,
                payload: rshare_core::EndpointEventPayload::Keyboard {
                    key: "ShiftLeft".to_string(),
                    state: "Pressed".to_string(),
                },
                mode: rshare_core::EndpointInjectMode::RequireHealthyBackend,
                timeout_ms: 750,
            },
        )
        .await;

        assert!(result.accepted);
        assert_eq!(result.correlation_id, correlation_id);
        assert_eq!(result.backend_kind, Some(BackendKind::Portable));
        assert_eq!(result.error, None);
        assert!(result.loopback_event_id.is_some());

        let mut state = state.write().await;
        let events = state.endpoint_events(
            &EndpointEventFilter {
                kinds: vec![rshare_core::EndpointEventKind::Keyboard],
                include_loopback: true,
                ..EndpointEventFilter::default()
            },
            None,
            Some(8),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].correlation_id.as_deref(), Some("ipc-shift-1"));
        assert_eq!(
            events[0].direction,
            rshare_core::EndpointEventDirection::InjectedLoopback
        );
    }

    #[tokio::test]
    async fn inject_endpoint_event_accepts_unicode_text_commit() {
        let injected = Arc::new(std::sync::Mutex::new(Vec::new()));
        let backend = test_injection_handle(RecordingKindInjectBackend {
            kind: BackendKind::Portable,
            injected: injected.clone(),
        });
        let state = Arc::new(RwLock::new(test_daemon_state()));
        let network_manager = Arc::new(Mutex::new(NetworkManager::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local".to_string(),
        )));
        let (events, mut rx) = broadcast::channel(4);

        let result = inject_endpoint_event(
            &network_manager,
            &backend,
            &state,
            &events,
            rshare_core::EndpointInjectTarget::Local,
            rshare_core::EndpointInjectRequest {
                correlation_id: "mobile-text-1".to_string(),
                device_kind: rshare_core::EndpointEventKind::Keyboard,
                payload: rshare_core::EndpointEventPayload::TextCommit {
                    text: "你好🙂".to_string(),
                },
                mode: rshare_core::EndpointInjectMode::RequireHealthyBackend,
                timeout_ms: 750,
            },
        )
        .await;

        assert!(result.accepted);
        let injected = injected.lock().unwrap();
        assert_eq!(injected.len(), 1);
        assert!(matches!(
            &injected[0],
            rshare_input::InputEvent::TextCommit { text } if text == "你好🙂"
        ));
        drop(injected);

        let diagnostic = rx.recv().await.unwrap();
        let diagnostic_json = serde_json::to_string(&diagnostic).unwrap();
        assert_eq!(
            diagnostic.payload.get("char_count").map(String::as_str),
            Some("3")
        );
        assert!(!diagnostic.payload.contains_key("text"));
        assert!(!diagnostic_json.contains("你好🙂"));
        assert!(!diagnostic_json.contains("\"text\":"));

        let mut state = state.write().await;
        let endpoint_events = state.endpoint_events(
            &EndpointEventFilter {
                kinds: vec![rshare_core::EndpointEventKind::Keyboard],
                include_loopback: true,
                ..EndpointEventFilter::default()
            },
            None,
            Some(8),
        );
        let endpoint_json = serde_json::to_string(&endpoint_events).unwrap();
        assert!(endpoint_json.contains("char_count"));
        assert!(!endpoint_json.contains("你好🙂"));
        assert!(!endpoint_json.contains("\"text\":"));
    }

    #[test]
    fn captured_text_commit_diagnostics_never_retain_raw_content() {
        let mut state = test_daemon_state();

        let diagnostic = state.record_local_input_event(&rshare_input::InputEvent::TextCommit {
            text: "私密文本🙂".to_string(),
        });
        let endpoint = state.endpoint_event_from_local(diagnostic.clone());

        assert_eq!(
            diagnostic.payload.get("char_count").map(String::as_str),
            Some("5")
        );
        assert!(!diagnostic.payload.contains_key("text"));
        for serialized in [
            serde_json::to_string(&diagnostic).unwrap(),
            serde_json::to_string(&endpoint).unwrap(),
        ] {
            assert!(!serialized.contains("私密文本🙂"));
            assert!(!serialized.contains("\"text\":"));
            assert!(serialized.contains("char_count"));
        }
    }

    #[test]
    fn mobile_gateway_authorizes_query_or_bearer_token() {
        let query_request =
            mobile_gateway::MobileHttpRequest::new("GET", "/mobile?t=mobile-secret", Vec::new());
        assert!(mobile_gateway::is_authorized_mobile_request(
            &query_request,
            "mobile-secret"
        ));

        let header_request = mobile_gateway::MobileHttpRequest::new(
            "POST",
            "/api/inject",
            vec![("authorization", "Bearer mobile-secret")],
        );
        assert!(mobile_gateway::is_authorized_mobile_request(
            &header_request,
            "mobile-secret"
        ));

        let encoded_query_request = mobile_gateway::MobileHttpRequest::new(
            "GET",
            "/mobile?t=mobile%2Bsecret%2Ftoken%3D",
            Vec::new(),
        );
        assert!(mobile_gateway::is_authorized_mobile_request(
            &encoded_query_request,
            "mobile+secret/token="
        ));
    }

    #[test]
    fn mobile_gateway_rejects_missing_or_wrong_token() {
        let missing = mobile_gateway::MobileHttpRequest::new("GET", "/mobile", Vec::new());
        assert!(!mobile_gateway::is_authorized_mobile_request(
            &missing,
            "mobile-secret"
        ));

        let wrong = mobile_gateway::MobileHttpRequest::new("GET", "/mobile?t=wrong", Vec::new());
        assert!(!mobile_gateway::is_authorized_mobile_request(
            &wrong,
            "mobile-secret"
        ));
    }

    #[test]
    fn mobile_gateway_routes_only_mobile_page_status_and_inject() {
        assert_eq!(
            mobile_gateway::route_mobile_http_request("GET", "/mobile?t=token"),
            mobile_gateway::MobileGatewayRoute::Page
        );
        assert_eq!(
            mobile_gateway::route_mobile_http_request("GET", "/mobile.webmanifest?t=token"),
            mobile_gateway::MobileGatewayRoute::NotFound
        );
        assert_eq!(
            mobile_gateway::route_mobile_http_request("GET", "/mobile-icon.svg?t=token"),
            mobile_gateway::MobileGatewayRoute::NotFound
        );
        assert_eq!(
            mobile_gateway::route_mobile_http_request("GET", "/api/local-controls?t=token"),
            mobile_gateway::MobileGatewayRoute::LocalControls
        );
        assert_eq!(
            mobile_gateway::route_mobile_http_request("POST", "/api/inject?t=token"),
            mobile_gateway::MobileGatewayRoute::Inject
        );
        assert_eq!(
            mobile_gateway::route_mobile_http_request("POST", "/api/shutdown?t=token"),
            mobile_gateway::MobileGatewayRoute::NotFound
        );
    }

    #[tokio::test]
    async fn remote_endpoint_inject_returns_transport_failure_without_connection() {
        let backend = test_injection_handle(TestInjectBackend {
            active: true,
            fail: false,
            injected: Vec::new(),
        });
        let remote = DeviceId::new_v4();
        let mut daemon_state = test_daemon_state();
        daemon_state.devices.insert(
            remote,
            TrackedDevice {
                id: remote,
                name: "remote".to_string(),
                hostname: "remote".to_string(),
                addresses: vec!["127.0.0.1:1".to_string()],
                connected: true,
                discovered: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        let state = Arc::new(RwLock::new(daemon_state));
        let network_manager = Arc::new(Mutex::new(NetworkManager::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local".to_string(),
        )));
        let (events, _rx) = broadcast::channel(4);

        let result = inject_endpoint_event(
            &network_manager,
            &backend,
            &state,
            &events,
            rshare_core::EndpointInjectTarget::Remote(remote),
            rshare_core::EndpointInjectRequest {
                correlation_id: "remote-shift-1".to_string(),
                device_kind: rshare_core::EndpointEventKind::Keyboard,
                payload: rshare_core::EndpointEventPayload::Keyboard {
                    key: "ShiftLeft".to_string(),
                    state: "Pressed".to_string(),
                },
                mode: rshare_core::EndpointInjectMode::RequireHealthyBackend,
                timeout_ms: 750,
            },
        )
        .await;

        assert!(!result.accepted);
        assert_eq!(
            result.target,
            rshare_core::EndpointInjectTarget::Remote(remote)
        );
        assert_eq!(
            result.error,
            Some(rshare_core::EndpointInjectError::TransportFailed)
        );
        assert!(state.read().await.pending_endpoint_injects.is_empty());
    }

    #[test]
    fn remote_endpoint_inject_result_completes_pending_request() {
        let mut state = test_daemon_state();
        let remote = DeviceId::new_v4();
        let (result_tx, mut result_rx) = oneshot::channel();
        state.pending_endpoint_injects.insert(
            "remote-shift-2".to_string(),
            PendingEndpointInject {
                target: remote,
                started_at_ms: timestamp_ms_now(),
                result_tx,
            },
        );

        assert!(state.complete_pending_endpoint_inject(
            remote,
            rshare_core::EndpointInjectResult {
                correlation_id: "remote-shift-2".to_string(),
                target: rshare_core::EndpointInjectTarget::Local,
                accepted: true,
                backend_kind: Some(BackendKind::Portable),
                health: BackendHealth::Healthy,
                elapsed_ms: 1,
                loopback_event_id: Some(9),
                error: None,
            },
        ));

        let result = result_rx.try_recv().unwrap();
        assert!(result.accepted);
        assert_eq!(
            result.target,
            rshare_core::EndpointInjectTarget::Remote(remote)
        );
        assert_eq!(result.error, None);
        assert!(state.pending_endpoint_injects.is_empty());
    }

    #[test]
    fn backend_with_missing_capture_is_not_reported_as_available() {
        let candidates = vec![candidate_from_component_health(
            BackendKind::Portable,
            BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable,
            },
            BackendHealth::Healthy,
        )];

        let (mode, available, health, error) = resolve_backend_selection(&candidates);

        assert!(mode.is_none());
        assert!(available.is_empty());
        assert!(matches!(
            health,
            BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable
            }
        ));
        assert!(error.unwrap().contains("No input backend"));
    }

    #[test]
    fn discovered_device_updates_in_memory_layout_without_desktop_roundtrip() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "0.0.0.0:27431".to_string(),
            27432,
            42,
        ));

        state.upsert_discovered(DiscoveredDevice {
            id: remote_id,
            name: "remote".to_string(),
            hostname: "remote-host".to_string(),
            addresses: vec!["192.168.1.241:27431".parse().unwrap()],
            screen_info: Some(ScreenInfo::new(0, 0, 2560, 1440)),
            capabilities: DeviceCapabilities::default(),
            transport_capabilities: rshare_core::PeerTransportCapabilities::required_v3(),
            protocol_compatibility: rshare_net::PeerProtocolCompatibility::Compatible,
            last_seen: Instant::now(),
        });

        let remote_node = state.layout.get_node(remote_id);
        assert!(
            remote_node.is_some(),
            "daemon discovery should populate layout immediately"
        );
        assert!(state.layout.links.iter().any(|link| {
            link.from_device == local_id
                && link.to_device == remote_id
                && link.from_edge == Direction::Right
        }));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn fallback_selection_preserves_selected_backend_health() {
        let candidates = vec![
            candidate_from_component_health(
                BackendKind::Portable,
                BackendHealth::Healthy,
                BackendHealth::Healthy,
            ),
            candidate_from_component_health(
                BackendKind::WindowsNative,
                BackendHealth::Degraded {
                    reason: BackendFailureReason::Unavailable,
                },
                BackendHealth::Healthy,
            ),
        ];

        let (mode, available, health, error) = resolve_backend_selection(&candidates);

        assert_eq!(mode, Some(ResolvedInputMode::Portable));
        assert_eq!(available, vec![BackendKind::Portable]);
        assert!(matches!(health, BackendHealth::Healthy));
        assert!(error.unwrap().contains("using Portable"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_filter_capture_only_for_selected_virtual_hid_backend() {
        let driver = rshare_core::LocalDriverDiagnosticState {
            status: "available".to_string(),
            filter_active: true,
            filter_keyboard_connects: 1,
            filter_mouse_connects: 1,
            ..rshare_core::LocalDriverDiagnosticState::default()
        };

        assert!(windows_should_use_filter_capture(
            Some(ResolvedInputMode::VirtualHid),
            &driver
        ));
        assert!(windows_driver_filter_capture_ready(&driver));
        assert!(!windows_should_use_filter_capture(
            Some(ResolvedInputMode::WindowsNative),
            &driver
        ));
        assert!(!windows_should_use_filter_capture(
            Some(ResolvedInputMode::Portable),
            &driver
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_falls_back_to_hook_when_filter_driver_is_not_active() {
        let driver = rshare_core::LocalDriverDiagnosticState {
            status: "available".to_string(),
            filter_active: false,
            ..rshare_core::LocalDriverDiagnosticState::default()
        };

        assert!(!windows_should_use_filter_capture(
            Some(ResolvedInputMode::VirtualHid),
            &driver
        ));
        assert!(!windows_driver_filter_capture_ready(&driver));
    }

    #[cfg(windows)]
    #[test]
    fn windows_falls_back_to_hook_until_filter_attaches_to_keyboard_and_mouse_stacks() {
        let driver = rshare_core::LocalDriverDiagnosticState {
            status: "available".to_string(),
            filter_active: true,
            filter_keyboard_connects: 1,
            filter_mouse_connects: 0,
            ..rshare_core::LocalDriverDiagnosticState::default()
        };

        assert!(!windows_should_use_filter_capture(
            Some(ResolvedInputMode::VirtualHid),
            &driver
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_driver_event_maps_to_local_input_event() {
        let event = rshare_platform::windows::WindowsDriverInputEvent {
            source: rshare_platform::windows::WindowsDriverEventSource::DriverTest,
            device_kind: rshare_platform::windows::WindowsDriverDeviceKind::Keyboard,
            event_kind: rshare_platform::windows::WindowsDriverEventKind::Key,
            device_id: "driver-keyboard".to_string(),
            device_instance_id: "instance".to_string(),
            value0: 0x1E,
            value1: 1,
            value2: 0,
            flags: 0,
            timestamp_us: 1,
        };

        let input = input_event_from_windows_driver_event(&event).unwrap();

        assert!(matches!(
            input,
            rshare_input::InputEvent::Key {
                keycode: rshare_input::KeyCode::Char(b'A'),
                state: rshare_input::ButtonState::Pressed
            }
        ));
        assert_eq!(
            local_source_from_windows_driver_event(event.source),
            LocalInputEventSource::DriverTest
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_driver_capture_publishes_directly_with_filter_origin() {
        let (producer, mut consumer) = SemanticInputIngress::new(4);
        publish_windows_driver_event(
            &producer,
            rshare_platform::windows::WindowsDriverInputEvent {
                source: rshare_platform::windows::WindowsDriverEventSource::Hardware,
                device_kind: rshare_platform::windows::WindowsDriverDeviceKind::Keyboard,
                event_kind: rshare_platform::windows::WindowsDriverEventKind::Key,
                device_id: "17".to_string(),
                device_instance_id: "instance".to_string(),
                value0: 0x1E,
                value1: 1,
                value2: 0,
                flags: 0,
                timestamp_us: 1,
            },
        );

        let captured = consumer.recv().await.expect("direct driver capture");
        assert_eq!(captured.origin.source, CaptureSource::WindowsFilter);
        assert_eq!(captured.origin.device_token, 17);
        assert!(matches!(
            captured.payload,
            CapturedInputPayload::Discrete(InputEvent::Key {
                keycode: rshare_input::KeyCode::Char(b'A'),
                state: rshare_input::ButtonState::Pressed,
            })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_filter_open_failure_starts_real_hook_fallback_seam() {
        let hook_started = Cell::new(false);
        let selection: WindowsCaptureSelection<(), &str> = start_windows_capture_with_fallback(
            true,
            || Err(anyhow::anyhow!("synthetic filter open failure")),
            || {
                hook_started.set(true);
                Ok("hook-listener")
            },
        )
        .unwrap();

        assert!(hook_started.get());
        assert!(matches!(
            selection,
            WindowsCaptureSelection::Hook {
                listener: "hook-listener",
                filter_error: Some(error),
            } if error.contains("synthetic filter open failure")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_driver_start_waits_for_open_failure_before_returning() {
        let (producer, _consumer) = SemanticInputIngress::new(4);
        let result = WindowsDriverCapture::start_with_opener(producer, || {
            Err(anyhow::anyhow!("synthetic startup failure"))
        });

        assert!(result
            .unwrap_err()
            .to_string()
            .contains("synthetic startup failure"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_driver_startup_handshake_times_out_without_blocking_daemon() {
        let (producer, _consumer) = SemanticInputIngress::new(4);
        let started = std::time::Instant::now();
        let result = WindowsDriverCapture::start_with_opener_timeout(
            producer,
            Duration::from_millis(10),
            || {
                std::thread::sleep(Duration::from_millis(100));
                rshare_platform::windows::WindowsDriverEventStream::open_filter()
            },
        );

        assert!(result.unwrap_err().to_string().contains("handshake failed"));
        assert!(started.elapsed() < Duration::from_millis(80));
    }

    #[cfg(windows)]
    #[test]
    fn windows_driver_relative_mouse_move_updates_absolute_position() {
        let display = fallback_display_state(1920, 1080);
        let event = rshare_platform::windows::WindowsDriverInputEvent {
            source: rshare_platform::windows::WindowsDriverEventSource::Hardware,
            device_kind: rshare_platform::windows::WindowsDriverDeviceKind::Mouse,
            event_kind: rshare_platform::windows::WindowsDriverEventKind::MouseMove,
            device_id: "driver-mouse".to_string(),
            device_instance_id: "instance".to_string(),
            value0: 5,
            value1: -10,
            value2: 0,
            flags: 0,
            timestamp_us: 1,
        };

        let input = input_event_from_windows_driver_event_with_pointer(&event, 100, 200, &display)
            .expect("driver mouse delta should map to absolute mouse move");

        assert!(matches!(
            input,
            rshare_input::InputEvent::MouseMove { x: 105, y: 190 }
        ));
    }

    #[test]
    fn mark_connected_tracks_unknown_inbound_device() {
        use rshare_core::Direction;
        use std::collections::HashSet;

        let remote_id = DeviceId::new_v4();
        let local_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        state.mark_connected(&remote_id, true);

        let device = state.devices.get(&remote_id).unwrap();
        assert_eq!(device.id, remote_id);
        assert!(device.connected);
        assert_eq!(device.hostname, "unknown");
        assert!(state.layout.get_node(remote_id).is_some());

        let connected_peers = HashSet::from([remote_id]);
        assert_eq!(
            state
                .layout
                .resolve_target(local_id, Direction::Right, &connected_peers),
            Some(remote_id)
        );
    }

    // Alpha-2 layout-driven routing tests
    // These tests verify that the daemon uses LayoutGraph instead of first_connected_device

    #[test]
    fn daemon_does_not_forward_to_first_connected_without_layout_link() {
        use rshare_core::{Direction, LayoutGraph};
        use std::collections::HashSet;

        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "z-remote-last".to_string(), // Name sorted last, but should not be used
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                discovered: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );

        // Create a layout with local device only (no link to remote)
        let layout = LayoutGraph::new(local_id);
        let connected_peers: HashSet<DeviceId> = [remote_id].into_iter().collect();

        // Edge hit should not find target without layout link
        let target = layout.resolve_target(local_id, Direction::Right, &connected_peers);
        assert_eq!(target, None, "Should not forward without layout link");
    }

    #[test]
    fn daemon_routes_through_layout_graph_not_first_connected() {
        use rshare_core::{Direction, LayoutGraph, LayoutLink, LayoutNode};
        use std::collections::HashSet;

        let local_id = DeviceId::new_v4();
        let remote_a = DeviceId::new_v4();
        let remote_b = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        // Add two connected devices
        state.devices.insert(
            remote_a,
            TrackedDevice {
                id: remote_a,
                name: "a-device".to_string(), // Would be first in name sort
                hostname: "a-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                discovered: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        state.devices.insert(
            remote_b,
            TrackedDevice {
                id: remote_b,
                name: "b-device".to_string(),
                hostname: "b-host".to_string(),
                addresses: vec!["127.0.0.1:27432".to_string()],
                connected: true,
                discovered: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );

        // Create layout that links local->remote_b (not remote_a)
        let mut layout = LayoutGraph::new(local_id);
        layout.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));
        layout.add_node(LayoutNode::new(remote_a, -1920, 0, 1920, 1080));
        layout.add_node(LayoutNode::new(remote_b, 1920, 0, 1920, 1080));
        layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_b,
            to_edge: Direction::Left,
        });

        let connected_peers: HashSet<DeviceId> = [remote_a, remote_b].into_iter().collect();

        // Should route to remote_b based on layout, not remote_a (first by name)
        let target = layout.resolve_target(local_id, Direction::Right, &connected_peers);
        assert_eq!(
            target,
            Some(remote_b),
            "Should route to layout-linked device"
        );
        assert_ne!(
            target,
            Some(remote_a),
            "Should not route to first-connected device"
        );
    }

    #[test]
    fn daemon_disconnect_clears_remote_active_session() {
        use rshare_core::{
            CaptureSessionStateMachine, ControlSessionState, Direction, SuspendReason,
        };

        let remote_id = DeviceId::new_v4();
        let mut machine = CaptureSessionStateMachine::new();

        // Enter remote mode
        machine
            .on_edge_hit(Direction::Right, Some(remote_id))
            .unwrap();
        assert!(matches!(
            machine.state(),
            ControlSessionState::RemoteActive { .. }
        ));

        // Disconnect should clear session
        machine.on_target_disconnect(remote_id);
        assert!(matches!(
            machine.state(),
            ControlSessionState::Suspended {
                reason: SuspendReason::TargetUnavailable
            }
        ));
    }

    #[test]
    fn daemon_backend_degradation_prevents_forwarding() {
        use rshare_core::{
            CaptureSessionStateMachine, ControlSessionState, Direction, SuspendReason,
        };

        let remote_id = DeviceId::new_v4();
        let mut machine = CaptureSessionStateMachine::new();

        // Backend degrades
        machine.on_backend_degraded();

        // Edge hit should not work
        let result = machine.on_edge_hit(Direction::Right, Some(remote_id));
        assert!(result.is_err());

        // State should be suspended
        assert!(matches!(
            machine.state(),
            ControlSessionState::Suspended {
                reason: SuspendReason::BackendDegraded
            }
        ));
    }

    #[test]
    fn daemon_session_state_exposed_in_snapshot() {
        use rshare_core::ControlSessionState;

        let local_id = DeviceId::new_v4();
        let state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        // Session state should be accessible
        let snapshot = state.status_snapshot();
        assert_eq!(
            snapshot.session_state,
            Some(ControlSessionState::LocalReady)
        );
        assert_eq!(snapshot.active_target, None);
    }

    #[test]
    fn status_snapshot_includes_latency_feedback() {
        let mut state = test_daemon_state();
        state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);

        let snapshot = state.status_snapshot_for_connections(&[]);

        assert_eq!(
            snapshot.latency_feedback.local_input.status,
            LatencyFeedbackStatus::Idle
        );
        assert_eq!(
            snapshot.latency_feedback.transport.status,
            LatencyFeedbackStatus::Unavailable
        );
    }

    #[test]
    fn status_snapshot_latency_feedback_uses_connection_snapshot_network() {
        let state = test_daemon_state();
        let connection = connected_connection_info(DeviceId::new_v4(), Some(12));

        let snapshot = state.status_snapshot_for_connections(&[connection]);

        assert_eq!(snapshot.network.rtt_ms, Some(12));
        assert_eq!(
            snapshot.latency_feedback.transport.status,
            LatencyFeedbackStatus::Healthy
        );
        assert_eq!(snapshot.latency_feedback.transport.rtt_ms, Some(12));
    }

    #[tokio::test]
    async fn daemon_ui_projection_uses_live_network_truth_for_status_and_capabilities() {
        let connection = connected_connection_info(DeviceId::new_v4(), Some(12));
        let source = DaemonUiProjectionSource {
            state: Arc::new(RwLock::new(test_daemon_state())),
            network: Arc::new(FixedDaemonNetworkProjection {
                connections: vec![connection],
            }),
            diagnostics: None,
        };

        let snapshot = source.project().await.unwrap();

        assert_eq!(snapshot.status.connected_devices, 1);
        assert_eq!(snapshot.status.network.rtt_ms, Some(12));
        assert_eq!(
            snapshot.status.latency_feedback.transport.status,
            LatencyFeedbackStatus::Healthy
        );
        let local = snapshot
            .capabilities
            .devices
            .iter()
            .find(|device| device.device_id == snapshot.capabilities.local_device_id)
            .unwrap();
        let diagnostics = local
            .capabilities
            .iter()
            .find(|capability| capability.kind == rshare_core::EndpointCapabilityKind::Diagnostics)
            .unwrap();
        assert_eq!(diagnostics.latency_ms, Some(12));
        assert_eq!(diagnostics.transport_state.as_deref(), Some("healthy"));
        assert_eq!(
            diagnostics
                .details
                .get("datagram_available")
                .map(String::as_str),
            Some("true")
        );
    }

    #[tokio::test]
    async fn sampled_control_metrics_wake_live_ui_stream_without_interval_refresh() {
        let (diagnostics_tx, diagnostics_rx) = watch::channel(zero_control_metrics());
        let (_network_tx, network_rx) = watch::channel(0_u64);
        let source = Arc::new(DaemonUiProjectionSource {
            state: Arc::new(RwLock::new(test_daemon_state())),
            network: Arc::new(FixedDaemonNetworkProjection {
                connections: Vec::new(),
            }),
            diagnostics: Some(diagnostics_rx.clone()),
        });
        let initial = source.project().await.unwrap();
        let (_input, feeds) = input_state_channel(4);
        let aggregator = StateAggregator::try_with_projection_and_diagnostics(
            initial,
            8,
            feeds,
            source,
            diagnostics_rx,
            network_rx,
        )
        .unwrap();
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(25), subscriber.recv())
                .await
                .is_err(),
            "live UI must not refresh on an interval without a sampled change"
        );

        diagnostics_tx.send_replace(ControlMetricSnapshot {
            captured: 7,
            routed: 5,
            realtime_replaced: 2,
            realtime_dropped: 1,
            reliable_overflow: 0,
        });
        let UiEnvelope::Delta(delta) = subscriber.recv().await.unwrap() else {
            panic!("sampled diagnostics must wake the live UI stream");
        };
        assert!(matches!(delta.change, UiChange::Status(_)));
        let snapshot = aggregator.latest_snapshot();
        assert_eq!(snapshot.status.latency_feedback.local_input.event_count, 7);
        assert_eq!(
            snapshot.status.latency_feedback.local_input.status,
            LatencyFeedbackStatus::Degraded
        );
        assert_eq!(
            snapshot.dynamic_state.diagnostics,
            snapshot.status.latency_feedback
        );
    }

    #[tokio::test]
    async fn network_projection_notification_updates_live_ui_rtt() {
        let network = MutableDaemonNetworkProjection::new(Vec::new());
        let network_rx = network.changes();
        let (diagnostics_tx, diagnostics_rx) = watch::channel(zero_control_metrics());
        let source = Arc::new(DaemonUiProjectionSource {
            state: Arc::new(RwLock::new(test_daemon_state())),
            network: Arc::new(network.clone()),
            diagnostics: Some(diagnostics_rx.clone()),
        });
        let initial = source.project().await.unwrap();
        let (_input, feeds) = input_state_channel(4);
        let aggregator = StateAggregator::try_with_projection_and_diagnostics(
            initial,
            8,
            feeds,
            source,
            diagnostics_rx,
            network_rx,
        )
        .unwrap();
        let mut subscriber = aggregator.subscribe(None).await.unwrap();
        let _ = subscriber.recv().await.unwrap();

        network.replace(vec![connected_connection_info(
            DeviceId::new_v4(),
            Some(23),
        )]);
        let UiEnvelope::Delta(delta) = subscriber.recv().await.unwrap() else {
            panic!("network actor notification must wake the live UI stream");
        };
        let UiChange::Status(status) = delta.change else {
            panic!("network notification must first emit the changed status");
        };
        assert_eq!(status.connected_devices, 1);
        assert_eq!(status.network.rtt_ms, Some(23));
        assert_eq!(status.latency_feedback.transport.rtt_ms, Some(23));
        drop(diagnostics_tx);
    }

    #[test]
    fn status_snapshot_latency_feedback_uses_connection_infos_for_transport_availability() {
        let state = test_daemon_state();
        let connection = connected_connection_info(DeviceId::new_v4(), Some(12));

        let snapshot = state.status_snapshot_for_connections(&[connection]);

        assert_eq!(snapshot.network.rtt_ms, Some(12));
        assert_eq!(
            snapshot.latency_feedback.transport.status,
            LatencyFeedbackStatus::Healthy
        );
    }

    #[test]
    fn status_snapshot_latency_feedback_degrades_when_any_connection_rtt_is_high() {
        let state = test_daemon_state();
        let fast_connection = connected_connection_info(DeviceId::new_v4(), Some(12));
        let slow_connection = connected_connection_info(DeviceId::new_v4(), Some(200));

        let snapshot = state.status_snapshot_for_connections(&[fast_connection, slow_connection]);

        assert_eq!(snapshot.network.rtt_ms, Some(12));
        assert_eq!(
            snapshot.latency_feedback.transport.status,
            LatencyFeedbackStatus::Degraded
        );
    }

    #[test]
    fn status_snapshot_for_connections_populates_latency_feedback() {
        let state = test_daemon_state();
        let connection = connected_connection_info(DeviceId::new_v4(), Some(12));

        let snapshot = state.status_snapshot_for_connections(&[connection]);

        assert_eq!(snapshot.network.rtt_ms, Some(12));
        assert!(snapshot.latency_feedback.generated_at_ms > 0);
        assert_eq!(snapshot.latency_feedback.transport.rtt_ms, Some(12));
        assert_eq!(
            snapshot.latency_feedback.transport.status,
            LatencyFeedbackStatus::Healthy
        );
    }

    #[test]
    fn daemon_disconnect_clears_active_session_in_snapshot() {
        use rshare_core::{ControlSessionState, Direction, SuspendReason};

        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        // Enter remote mode
        state
            .session
            .on_edge_hit(Direction::Right, Some(remote_id))
            .unwrap();
        let snapshot = state.status_snapshot();
        assert!(matches!(
            snapshot.session_state,
            Some(ControlSessionState::RemoteActive { .. })
        ));

        // Disconnect should update session
        state.session.on_target_disconnect(remote_id);
        let snapshot = state.status_snapshot();
        assert!(matches!(
            snapshot.session_state,
            Some(ControlSessionState::Suspended {
                reason: SuspendReason::TargetUnavailable
            })
        ));
    }

    #[test]
    fn daemon_reconnect_after_session_reset() {
        use rshare_core::ControlSessionState;

        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        // Enter and disconnect
        state
            .session
            .on_edge_hit(Direction::Right, Some(remote_id))
            .unwrap();
        state.session.on_target_disconnect(remote_id);

        // Reset session
        state.session.reset();
        let snapshot = state.status_snapshot();
        assert_eq!(
            snapshot.session_state,
            Some(ControlSessionState::LocalReady)
        );

        // Can enter remote mode again
        state
            .session
            .on_edge_hit(Direction::Right, Some(remote_id))
            .unwrap();
        let snapshot = state.status_snapshot();
        assert!(matches!(
            snapshot.session_state,
            Some(ControlSessionState::RemoteActive { .. })
        ));
    }

    #[test]
    fn stale_layout_from_previous_daemon_run_must_be_canonicalized_to_current_local_device() {
        use rshare_core::{Direction, LayoutGraph, LayoutLink};

        let current_local = DeviceId::new_v4();
        let stale_local = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            current_local,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));

        let mut layout = LayoutGraph::new(stale_local);
        layout.add_node(LayoutNode::new(stale_local, 0, 0, 1920, 1080));
        layout.add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        layout.add_link(LayoutLink::new(
            stale_local,
            Direction::Right,
            remote_id,
            Direction::Left,
        ));

        apply_layout_update(&mut state, layout);

        assert_eq!(state.layout.local_device, current_local);
        assert!(state
            .layout
            .links
            .iter()
            .any(|link| link.from_device == current_local && link.to_device == remote_id));
    }

    fn test_status(local_id: DeviceId) -> ServiceStatusSnapshot {
        ServiceStatusSnapshot::new(
            local_id,
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        )
    }

    fn discovered_device_for_test(id: DeviceId) -> DiscoveredDevice {
        DiscoveredDevice {
            id,
            name: "remote".to_string(),
            hostname: "remote-host".to_string(),
            addresses: vec!["127.0.0.1:27431".parse().unwrap()],
            screen_info: None,
            capabilities: DeviceCapabilities::default(),
            transport_capabilities: rshare_core::PeerTransportCapabilities::required_v3(),
            protocol_compatibility: rshare_net::PeerProtocolCompatibility::Compatible,
            last_seen: Instant::now(),
        }
    }

    #[test]
    fn discovery_lost_removes_unconnected_peer_but_preserves_layout() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(test_status(local_id));
        state.upsert_discovered(discovered_device_for_test(remote_id));

        state.mark_discovery_lost(&remote_id);

        assert!(!state.devices.contains_key(&remote_id));
        assert!(state.layout.get_node(remote_id).is_some());
    }

    #[test]
    fn discovery_lost_keeps_connected_generation_visible() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(test_status(local_id));
        state.upsert_discovered(discovered_device_for_test(remote_id));
        state.mark_connected(&remote_id, true);

        state.mark_discovery_lost(&remote_id);

        let device = state.devices.get(&remote_id).unwrap();
        assert!(device.connected);
        assert!(!device.discovered);
    }

    #[test]
    fn transport_disconnect_keeps_still_discovered_peer_visible() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(test_status(local_id));
        state.upsert_discovered(discovered_device_for_test(remote_id));
        state.mark_connected(&remote_id, true);

        state.mark_disconnected(&remote_id);

        let device = state.devices.get(&remote_id).unwrap();
        assert!(!device.connected);
        assert!(device.discovered);
    }

    #[tokio::test]
    async fn connected_peer_publishes_exact_layout_before_connectivity() {
        let state_dir = temp_state_dir();
        let layout_path = rshare_core::service::layout_graph_path_in(&state_dir);
        let local_id = DeviceId::new_v4();
        let peer = DeviceId::new_v4();
        let layout = remembered_layout(local_id, peer);
        let (tx, mut rx) = tokio::sync::mpsc::channel(2);

        persist_and_publish_connected_peer(&layout, &layout_path, &tx, peer)
            .await
            .unwrap();

        assert_eq!(
            rx.recv().await,
            Some(RouterCommand::LayoutChanged(layout.clone()))
        );
        assert_eq!(
            rx.recv().await,
            Some(RouterCommand::ConnectivityChanged {
                peer,
                connected: true,
            })
        );
        assert_eq!(
            load_layout_from_path(local_id, &layout_path).unwrap(),
            layout
        );
        let _ = std::fs::remove_dir_all(state_dir);
    }

    fn temp_state_dir() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("rshare-daemon-layout-test-{}", DeviceId::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn remembered_layout(local_id: DeviceId, remote_id: DeviceId) -> LayoutGraph {
        use rshare_core::{Direction, LayoutLink};

        let mut layout = LayoutGraph::new(local_id);
        layout.add_node(LayoutNode::new(local_id, 0, 0, 1920, 1080));
        layout.add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        layout.add_link(LayoutLink::new(
            local_id,
            Direction::Right,
            remote_id,
            Direction::Left,
        ));
        layout
    }

    #[test]
    fn daemon_loads_saved_layout_from_state_dir() {
        let state_dir = temp_state_dir();
        let layout_path = rshare_core::service::layout_graph_path_in(&state_dir);
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let expected = remembered_layout(local_id, remote_id);

        save_layout_to_path(&expected, &layout_path).unwrap();

        let loaded = load_layout_from_path(local_id, &layout_path).unwrap();

        assert_eq!(loaded, expected);
        let _ = std::fs::remove_dir_all(state_dir);
    }

    #[test]
    fn daemon_falls_back_to_local_only_layout_when_no_saved_layout_exists() {
        let state_dir = temp_state_dir();
        let layout_path = rshare_core::service::layout_graph_path_in(&state_dir);
        let local_id = DeviceId::new_v4();

        let loaded = load_layout_from_path(local_id, &layout_path).unwrap();

        let state = DaemonState::new(test_status(local_id));
        assert_eq!(loaded, state.layout);
        let _ = std::fs::remove_dir_all(state_dir);
    }

    #[test]
    fn daemon_saved_layout_survives_restart_semantics() {
        let state_dir = temp_state_dir();
        let layout_path = rshare_core::service::layout_graph_path_in(&state_dir);
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let expected = remembered_layout(local_id, remote_id);

        let mut first_start = DaemonState::new(test_status(local_id));
        apply_layout_update(&mut first_start, expected.clone());
        save_layout_to_path(&first_start.layout, &layout_path).unwrap();

        let restarted = load_layout_from_path(local_id, &layout_path).unwrap();

        assert_eq!(restarted, first_start.layout);
        let _ = std::fs::remove_dir_all(state_dir);
    }

    #[test]
    fn daemon_falls_back_to_local_only_layout_when_saved_layout_is_invalid_json() {
        let state_dir = temp_state_dir();
        let layout_path = rshare_core::service::layout_graph_path_in(&state_dir);
        let local_id = DeviceId::new_v4();

        std::fs::write(&layout_path, "{ definitely-not-json").unwrap();

        let loaded = load_layout_from_path(local_id, &layout_path).unwrap();

        assert_eq!(loaded, default_local_only_layout(local_id));
        assert!(
            layout_path.with_extension("json.invalid").exists(),
            "invalid layout should be retained for inspection"
        );
        let _ = std::fs::remove_dir_all(state_dir);
    }

    #[test]
    fn stale_diagnostics_worker_cannot_admit_send_to_replacement_generation() {
        let peer_id = DeviceId::new_v4();
        let old_generation = ControlConnectionId::new();
        let replacement_generation = ControlConnectionId::new();
        let old_subscription = DiagnosticSubscriberId {
            peer_id,
            control_connection_id: old_generation,
        };

        assert!(diagnostic_generation_is_current(
            old_subscription,
            Some(old_generation)
        ));
        assert!(!diagnostic_generation_is_current(
            old_subscription,
            Some(replacement_generation)
        ));
        assert!(!diagnostic_generation_is_current(old_subscription, None));
    }

    #[test]
    fn generationless_connection_error_does_not_clear_live_generation() {
        let peer_id = DeviceId::new_v4();
        let live_generation = ControlConnectionId::new();
        let current_generations =
            std::sync::RwLock::new(HashMap::from([(peer_id, live_generation)]));

        assert_eq!(
            take_current_generation_for_connection_error(&current_generations, peer_id, None),
            None
        );
        assert_eq!(
            current_generations
                .read()
                .expect("input generation registry poisoned")
                .get(&peer_id),
            Some(&live_generation)
        );
    }

    #[test]
    fn telemetry_subscription_enters_the_authenticated_message_dispatch_path() {
        let peer_id = DeviceId::new_v4();
        let control_connection_id = ControlConnectionId::new();
        let auth = Arc::new(rshare_net::handshake::PeerAuthContext {
            peer_id,
            certificate_fingerprint: rshare_net::encryption::PeerCertificateFingerprint::from_der(
                b"telemetry-peer",
            ),
            control_connection_id,
        });
        let message = Message::EndpointEventSubscribe {
            filter: EndpointEventFilter::default(),
        };
        let frame = match ClassifiedMessage::try_from(message) {
            Ok(ClassifiedMessage::Telemetry(frame)) => frame,
            other => panic!("subscription must classify as telemetry, got {other:?}"),
        };

        let (actual_peer, actual_generation, actual_message) =
            into_authenticated_network_message(NetworkEvent::TelemetryReceived { auth, frame })
                .expect("telemetry must use the same authenticated dispatch path as control");

        assert_eq!(actual_peer, peer_id);
        assert_eq!(actual_generation, control_connection_id);
        assert!(matches!(
            actual_message,
            Message::EndpointEventSubscribe { .. }
        ));
    }

    fn peer_stream_test_event(
        sequence: u64,
        device_kind: LocalInputDeviceKind,
        event_kind: &str,
    ) -> LocalInputDiagnosticEvent {
        LocalInputDiagnosticEvent {
            sequence,
            timestamp_ms: sequence,
            device_kind,
            event_kind: event_kind.to_string(),
            summary: format!("peer stream event {sequence}"),
            device_id: None,
            device_instance_id: None,
            capture_path: Some("diagnostics-test".to_string()),
            source: LocalInputEventSource::System,
            payload: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn telemetry_lane_full_keeps_peer_diagnostics_forwarder_alive_for_the_latest_batch() {
        let metrics = Arc::new(ControlMetrics::default());
        let mut runtime = DiagnosticsRuntime::new(metrics, 8);
        let subscriber_id = DiagnosticSubscriberId {
            peer_id: DeviceId::new_v4(),
            control_connection_id: ControlConnectionId::new(),
        };
        runtime.activate_generation(subscriber_id);
        let subscription = runtime
            .subscribe_current(subscriber_id)
            .expect("active generation must admit one peer stream");
        let local_device_id = DeviceId::new_v4();
        let (telemetry_tx, mut telemetry_rx) = tokio::sync::mpsc::channel(1);
        telemetry_tx
            .try_send(rshare_net::TelemetryFrame::latency_probe(1, 1, false, None))
            .unwrap();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let send_attempts = attempts.clone();
        let forwarder = tokio::spawn(run_peer_diagnostics_forwarder(
            subscription,
            local_device_id,
            subscriber_id.peer_id,
            move |frame| {
                send_attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                telemetry_tx.try_send(frame).map_err(|error| match error {
                    tokio::sync::mpsc::error::TrySendError::Full(_) => {
                        rshare_net::qos::TransportSendError::TelemetryLaneFull
                    }
                    tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                        rshare_net::qos::TransportSendError::LaneClosed
                    }
                })
            },
        ));

        runtime.record_discrete(peer_stream_test_event(
            10,
            LocalInputDeviceKind::Keyboard,
            "key",
        ));
        assert!(runtime.sample_at(rshare_daemon::diagnostics_runtime::DIAGNOSTICS_SAMPLE_PERIOD));
        tokio::time::timeout(Duration::from_secs(1), async {
            while attempts.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the full lane must be observed");
        let _filler = telemetry_rx.recv().await.unwrap();

        runtime.record_discrete(peer_stream_test_event(
            11,
            LocalInputDeviceKind::Mouse,
            "move",
        ));
        assert!(
            runtime.sample_at(rshare_daemon::diagnostics_runtime::DIAGNOSTICS_SAMPLE_PERIOD * 2)
        );
        let recovered = tokio::time::timeout(Duration::from_secs(1), telemetry_rx.recv())
            .await
            .expect("a later latest batch must be attempted after transient saturation")
            .expect("typed telemetry lane remains open");
        assert!(matches!(
            recovered.into_message(),
            Message::InputDiagnostic { .. }
        ));
        assert!(
            !forwarder.is_finished(),
            "TelemetryLaneFull must not cancel the peer subscription"
        );
        forwarder.abort();
    }

    #[tokio::test]
    async fn repeated_filters_share_one_broad_peer_stream_and_filter_locally() {
        let metrics = Arc::new(ControlMetrics::default());
        let mut runtime = DiagnosticsRuntime::new(metrics, 8);
        let subscriber_id = DiagnosticSubscriberId {
            peer_id: DeviceId::new_v4(),
            control_connection_id: ControlConnectionId::new(),
        };
        runtime.activate_generation(subscriber_id);
        let subscription = runtime
            .subscribe_current(subscriber_id)
            .expect("first local view starts the broad peer stream");
        assert!(
            runtime.subscribe_current(subscriber_id).is_none(),
            "a second filter must reuse, not replace, the generation stream"
        );
        let local_device_id = DeviceId::new_v4();
        let (telemetry_tx, mut telemetry_rx) = tokio::sync::mpsc::channel(8);
        let forwarder = tokio::spawn(run_peer_diagnostics_forwarder(
            subscription,
            local_device_id,
            subscriber_id.peer_id,
            move |frame| {
                telemetry_tx.try_send(frame).map_err(|error| match error {
                    tokio::sync::mpsc::error::TrySendError::Full(_) => {
                        rshare_net::qos::TransportSendError::TelemetryLaneFull
                    }
                    tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                        rshare_net::qos::TransportSendError::LaneClosed
                    }
                })
            },
        ));

        runtime.record_discrete(peer_stream_test_event(
            21,
            LocalInputDeviceKind::Keyboard,
            "key",
        ));
        runtime.record_discrete(peer_stream_test_event(
            22,
            LocalInputDeviceKind::Mouse,
            "move",
        ));
        assert!(runtime.sample_at(rshare_daemon::diagnostics_runtime::DIAGNOSTICS_SAMPLE_PERIOD));

        let mut peer_events = Vec::new();
        for _ in 0..3 {
            let frame = tokio::time::timeout(Duration::from_secs(1), telemetry_rx.recv())
                .await
                .unwrap()
                .unwrap();
            if let Message::InputDiagnostic { event, .. } = frame.into_message() {
                peer_events.push(EndpointEvent::from_local_diagnostic(local_device_id, event));
            }
        }
        let keyboard = EndpointEventFilter {
            kinds: vec![rshare_core::EndpointEventKind::Keyboard],
            ..EndpointEventFilter::default()
        };
        let mouse = EndpointEventFilter {
            kinds: vec![rshare_core::EndpointEventKind::Mouse],
            ..EndpointEventFilter::default()
        };
        assert_eq!(
            peer_events
                .iter()
                .filter(|event| keyboard.matches(event))
                .count(),
            1
        );
        assert_eq!(
            peer_events
                .iter()
                .filter(|event| mouse.matches(event))
                .count(),
            1
        );
        forwarder.abort();
    }
}
