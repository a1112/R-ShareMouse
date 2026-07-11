//! R-ShareMouse daemon service.
//!
//! Background service that handles input sharing and local IPC for status queries.

mod audio_runtime;
mod endpoint_runtime;
mod mobile_gateway;

use anyhow::Result;
use endpoint_runtime::inject_endpoint_event;
use futures_util::SinkExt;
use rshare_core::{
    default_ipc_addr, default_local_controls_ws_addr, default_mobile_gateway_addr,
    local_capability_snapshots, read_json_line, remote_capability_snapshots, write_json_line,
    AudioFormat, BackendFailureReason, BackendHealth, BackendKind, BackendRuntimeState,
    CapabilityRegistrySnapshot, CapabilityState, CaptureSessionStateMachine, Config,
    ControlSessionState, DaemonDeviceSnapshot, DaemonRequest, DaemonResponse, DeviceCapabilities,
    DeviceCapabilitySnapshot, DeviceId, Direction, DisplayCaptureResult, DisplayIdentifyResult,
    DisplayNode, DisplayOperationStatus, DisplaySettingsUpdateResult, EndpointCapabilityKind,
    EndpointCapabilitySnapshot, EndpointEvent, EndpointEventFilter, EndpointEventStore,
    EndpointInjectError, EndpointInjectRequest, EndpointInjectResult, EndpointInjectTarget,
    FeatureConfig, LatencyFeedbackSnapshot, LatencyFeedbackStatus, LayoutGraph, LayoutNode,
    LocalAudioCaptureSource, LocalAudioCaptureStatus, LocalAudioTestResult, LocalAudioTestStatus,
    LocalControlDeviceSnapshot, LocalDisplayInfo, LocalDisplayState, LocalGamepadState,
    LocalInputDeviceKind, LocalInputDiagnosticEvent, LocalInputEventSource, LocalInputFeedback,
    LocalInputTestKind, LocalInputTestRequest, LocalInputTestResult, LocalInputTestStatus, Message,
    NetworkTransportSnapshot, RemoteDeviceLatencyFeedback, RemoteLatencyFeedback,
    RemoteUsbDeviceSnapshot, ResolvedInputMode, ScreenInfo, ServiceStatusSnapshot,
    TransportFeedback, UsbControlSetupPacket, UsbDescriptorProbeResult, UsbDescriptorProbeStatus,
    UsbDeviceClaimRequest, UsbDeviceDescriptor, UsbDeviceSpeed, UsbTransferDirection,
    UsbTransferKind, UsbTransferPayload, UsbTransferStatus, VirtualDisplayCreateRequest,
    VirtualDisplayOperationResult, VirtualDisplayOperationStatus, VirtualDisplayRemoveRequest,
    VirtualDisplaySnapshot, VirtualDisplayStatus,
};
use rshare_input::{
    BackendCandidate, BackendSelector, CaptureBackend, GamepadListenerConfig, GilrsGamepadListener,
    InjectBackend, InputEvent, PortableCaptureBackend, PortableInjectBackend,
};

#[cfg(any(windows, target_os = "linux"))]
use rshare_input::InputEventChannel;
#[cfg(not(windows))]
use rshare_input::RDevInputListener;
#[cfg(windows)]
use rshare_input::{DefaultInputListener, InputListener};
use rshare_net::{
    connection::{ConnectionInfo, ConnectionState},
    DiscoveredDevice, NetworkEvent, NetworkManager, NetworkManagerConfig,
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
use tokio::sync::{broadcast, oneshot, Mutex, RwLock};
use tokio::time::{Duration, Instant};
use tokio_tungstenite::{accept_async, tungstenite::Message as WsMessage};

#[derive(Clone)]
struct TrackedDevice {
    id: DeviceId,
    name: String,
    hostname: String,
    addresses: Vec<String>,
    connected: bool,
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
        match rshare_platform::virtual_display::create_virtual_display(&platform_request) {
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
        let id = request.id.trim().to_string();
        if id.is_empty() {
            return VirtualDisplayOperationResult {
                status: VirtualDisplayOperationStatus::Failed,
                display: None,
                message: Some("virtual display id is required".to_string()),
            };
        }

        let platform_result = rshare_platform::virtual_display::remove_virtual_display(&request);
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

    fn upsert_discovered(&mut self, device: DiscoveredDevice) {
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
                capabilities: device.capabilities,
                last_seen_at: Instant::now(),
            },
        );
        self.layout
            .merge_discovered_peers_to_right_with_screens([(device.id, screen_info)]);
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
        self.status_snapshot_with_network_and_transport(&network, transport)
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
        inject_health: BackendHealth,
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
        self.local_controls.inject_backend.kind = mode.map(backend_kind_from_resolved_mode);
        self.local_controls.inject_backend.health = Some(inject_health.clone());
        self.local_controls.inject_backend.active = matches!(inject_health, BackendHealth::Healthy);
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
                payload.insert("text".to_string(), text.clone());
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

fn diagnostic_message(local_device_id: DeviceId, event: LocalInputDiagnosticEvent) -> Message {
    Message::InputDiagnostic {
        device_id: local_device_id,
        event,
    }
}

async fn broadcast_diagnostic_event(
    network_manager: &Arc<Mutex<NetworkManager>>,
    local_device_id: DeviceId,
    event: LocalInputDiagnosticEvent,
) {
    let endpoint_event = EndpointEvent::from_local_diagnostic(local_device_id, event.clone());
    let result = {
        let mut manager = network_manager.lock().await;
        manager
            .broadcast(diagnostic_message(local_device_id, event))
            .await
    };

    if let Err(error) = result {
        tracing::debug!("Failed to broadcast input diagnostic event: {}", error);
    }

    let result = {
        let mut manager = network_manager.lock().await;
        manager
            .broadcast(Message::EndpointEventDelta {
                event: endpoint_event,
            })
            .await
    };

    if let Err(error) = result {
        tracing::debug!("Failed to broadcast endpoint event delta: {}", error);
    }
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
    request: &rshare_core::DisplayCaptureRequest,
    result: Result<DisplayCaptureResult>,
) -> DaemonResponse {
    DaemonResponse::DisplayCapture(result.unwrap_or_else(|error| DisplayCaptureResult {
        status: DisplayOperationStatus::ApplyFailed,
        display_id: request.display_id.clone(),
        mime_type: None,
        width: None,
        height: None,
        bytes: Vec::new(),
        message: Some(error.to_string()),
    }))
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

#[cfg(windows)]
fn set_local_shortcut_suppression(enabled: bool) {
    rshare_platform::windows::set_local_input_suppressed(enabled);
}

#[cfg(not(windows))]
fn set_local_shortcut_suppression(_enabled: bool) {}

fn sync_local_shortcut_suppression(state: &DaemonState) {
    let should_suppress = state.features.suppress_local_shortcuts_when_remote
        && matches!(
            state.session.state(),
            ControlSessionState::RemoteActive { .. }
        );
    set_local_shortcut_suppression(should_suppress);
}

#[derive(Debug, Clone)]
struct InputRoutingState {
    remote_target: Option<DeviceId>,
    screen_width: u32,
    screen_height: u32,
    edge_threshold: u32,
    modifiers: ActiveModifiers,
    pending_return_edge: Option<Direction>,
}

impl InputRoutingState {
    fn new(screen_width: u32, screen_height: u32, edge_threshold: u32) -> Self {
        Self {
            remote_target: None,
            screen_width: screen_width.max(1),
            screen_height: screen_height.max(1),
            edge_threshold: edge_threshold.max(1),
            modifiers: ActiveModifiers::default(),
            pending_return_edge: None,
        }
    }

    fn default_with_threshold(edge_threshold: u32) -> Self {
        Self::new(1920, 1080, edge_threshold)
    }

    #[cfg(test)]
    fn for_test(screen_width: u32, screen_height: u32, edge_threshold: u32) -> Self {
        Self::new(screen_width, screen_height, edge_threshold)
    }

    #[cfg(test)]
    fn remote_target(&self) -> Option<DeviceId> {
        self.remote_target
    }

    fn clear_remote_target(&mut self) {
        self.remote_target = None;
    }

    fn set_remote_target(&mut self, target: DeviceId) {
        self.remote_target = Some(target);
    }

    fn schedule_return_to_local(&mut self, return_edge: Direction) {
        self.pending_return_edge = Some(return_edge);
    }

    fn take_pending_return_edge(&mut self) -> Option<Direction> {
        self.pending_return_edge.take()
    }

    fn update_modifier_state(&mut self, event: &InputEvent) -> ActiveModifiers {
        match event {
            InputEvent::Key { keycode, state } => {
                self.modifiers.update_key(*keycode, state.is_pressed());
            }
            InputEvent::KeyExtended {
                keycode,
                state,
                shift,
                ctrl,
                alt,
                meta,
            } => {
                self.modifiers = ActiveModifiers {
                    shift: *shift,
                    ctrl: *ctrl,
                    alt: *alt,
                    meta: *meta,
                    ..ActiveModifiers::default()
                };
                self.modifiers.update_key(*keycode, state.is_pressed());
            }
            _ => {}
        }

        self.modifiers
    }

    fn hit_edges(&self, event: &InputEvent) -> Vec<Direction> {
        let InputEvent::MouseMove { x, y } = event else {
            return Vec::new();
        };

        let mut edges = Vec::with_capacity(4);
        let right_edge_start = self.screen_width.saturating_sub(self.edge_threshold) as i32;
        let bottom_edge_start = self.screen_height.saturating_sub(self.edge_threshold) as i32;

        if *x <= self.edge_threshold as i32 && self.is_vertical_screen_coordinate(*y) {
            edges.push(Direction::Left);
        }
        if *x >= right_edge_start && self.is_vertical_screen_coordinate(*y) {
            edges.push(Direction::Right);
        }
        if *y <= self.edge_threshold as i32 && self.is_horizontal_screen_coordinate(*x) {
            edges.push(Direction::Top);
        }
        if *y >= bottom_edge_start && self.is_horizontal_screen_coordinate(*x) {
            edges.push(Direction::Bottom);
        }

        edges
    }

    fn is_vertical_screen_coordinate(&self, y: i32) -> bool {
        y >= 0 && y < self.screen_height as i32
    }

    fn is_horizontal_screen_coordinate(&self, x: i32) -> bool {
        x >= 0 && x < self.screen_width as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveModifiers {
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
    shift_key: u32,
    ctrl_key: u32,
    alt_key: u32,
    meta_key: u32,
}

impl Default for ActiveModifiers {
    fn default() -> Self {
        Self {
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
            shift_key: 0x10,
            ctrl_key: 0x11,
            alt_key: 0x12,
            meta_key: 0x5B,
        }
    }
}

impl ActiveModifiers {
    fn any(self) -> bool {
        self.shift || self.ctrl || self.alt || self.meta
    }

    fn update_key(&mut self, keycode: rshare_input::KeyCode, pressed: bool) {
        let raw = keycode.to_raw();
        match raw {
            0x10 | 0xA0 | 0xA1 => {
                self.shift = pressed;
                if pressed {
                    self.shift_key = raw;
                }
            }
            0x11 | 0xA2 | 0xA3 => {
                self.ctrl = pressed;
                if pressed {
                    self.ctrl_key = raw;
                }
            }
            0x12 | 0xA4 | 0xA5 => {
                self.alt = pressed;
                if pressed {
                    self.alt_key = raw;
                }
            }
            0x5B | 0x5C => {
                self.meta = pressed;
                if pressed {
                    self.meta_key = raw;
                }
            }
            _ => {}
        }
    }

    fn release_messages(self) -> Vec<Message> {
        let mut messages = Vec::with_capacity(4);
        if self.meta {
            messages.push(key_release_message(self.meta_key));
        }
        if self.shift {
            messages.push(key_release_message(self.shift_key));
        }
        if self.alt {
            messages.push(key_release_message(self.alt_key));
        }
        if self.ctrl {
            messages.push(key_release_message(self.ctrl_key));
        }
        messages
    }
}

fn key_release_message(keycode: u32) -> Message {
    Message::Key {
        keycode,
        state: rshare_core::KeyState::Released,
    }
}

fn is_modifier_key(keycode: rshare_input::KeyCode) -> bool {
    matches!(
        keycode.to_raw(),
        0x10 | 0x11 | 0x12 | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5 | 0x5B | 0x5C
    )
}

fn input_event_to_raw_event(
    event: rshare_input::InputEvent,
) -> Option<rshare_core::engine::RawInputEvent> {
    match event {
        rshare_input::InputEvent::MouseMove { x, y } => {
            Some(rshare_core::engine::RawInputEvent::MouseMove { x, y })
        }
        rshare_input::InputEvent::MouseButton { button, state } => {
            Some(rshare_core::engine::RawInputEvent::MouseButton {
                button: button.to_code(),
                pressed: state.is_pressed(),
            })
        }
        rshare_input::InputEvent::MouseWheel { delta_x, delta_y } => {
            Some(rshare_core::engine::RawInputEvent::MouseWheel { delta_x, delta_y })
        }
        rshare_input::InputEvent::Key { keycode, state } => {
            Some(rshare_core::engine::RawInputEvent::Key {
                keycode: keycode.to_raw(),
                pressed: state.is_pressed(),
            })
        }
        rshare_input::InputEvent::KeyExtended {
            keycode,
            state,
            shift,
            ctrl,
            alt,
            meta,
        } => Some(rshare_core::engine::RawInputEvent::KeyExtended {
            keycode: keycode.to_raw(),
            pressed: state.is_pressed(),
            shift,
            ctrl,
            alt,
            meta,
        }),
        rshare_input::InputEvent::TextCommit { .. } => None,
        rshare_input::InputEvent::GamepadConnected { info } => {
            Some(rshare_core::engine::RawInputEvent::GamepadConnected { info })
        }
        rshare_input::InputEvent::GamepadDisconnected { gamepad_id } => {
            Some(rshare_core::engine::RawInputEvent::GamepadDisconnected { gamepad_id })
        }
        rshare_input::InputEvent::GamepadState { state } => {
            Some(rshare_core::engine::RawInputEvent::GamepadState { state })
        }
    }
}

fn input_event_to_raw_event_with_modifiers(
    event: rshare_input::InputEvent,
    modifiers: ActiveModifiers,
) -> Option<rshare_core::engine::RawInputEvent> {
    match event {
        rshare_input::InputEvent::Key { keycode, state }
            if modifiers.any() && !is_modifier_key(keycode) =>
        {
            Some(rshare_core::engine::RawInputEvent::KeyExtended {
                keycode: keycode.to_raw(),
                pressed: state.is_pressed(),
                shift: modifiers.shift,
                ctrl: modifiers.ctrl,
                alt: modifiers.alt,
                meta: modifiers.meta,
            })
        }
        other => input_event_to_raw_event(other),
    }
}

fn messages_for_input_event(
    state: &mut DaemonState,
    routing: &mut InputRoutingState,
    forwarder: &mut rshare_core::engine::ForwardingEngine,
    event: InputEvent,
    gamepad_forwarding_enabled: bool,
) -> Vec<Message> {
    if is_gamepad_input_event(&event) && !gamepad_forwarding_enabled {
        return Vec::new();
    }

    let active_modifiers = routing.update_modifier_state(&event);

    // Get connected peers set
    let connected_peers: std::collections::HashSet<_> = state
        .devices
        .values()
        .filter(|device| device.connected)
        .map(|device| device.id)
        .collect();
    let local_id = state.status.device_id;
    let edge_hits = routing.hit_edges(&event);
    let mut activation_edge = None;
    let mut activation_target = None;

    match state.session.state() {
        ControlSessionState::RemoteActive {
            target,
            entered_via,
        } => {
            routing.set_remote_target(target);
            if !is_device_connected(state, target) {
                state.session.on_target_disconnect(target);
                routing.clear_remote_target();
                forwarder.clear_target();
                return Vec::new();
            }

            if is_quick_return_hotkey(&event, active_modifiers) {
                routing.schedule_return_to_local(entered_via.opposite());
                return active_modifiers.release_messages();
            }

            let return_edge = entered_via.opposite();
            if edge_hits.contains(&return_edge) {
                let _ = state.session.on_return_edge_hit(return_edge);
                routing.clear_remote_target();
                forwarder.clear_target();
                return Vec::new();
            }
        }
        ControlSessionState::Suspended { .. } => {
            routing.clear_remote_target();
            forwarder.clear_target();
            return Vec::new();
        }
        _ => {
            routing.clear_remote_target();
            if let Some((edge, target)) = edge_hits.iter().find_map(|edge| {
                state
                    .layout
                    .resolve_target(local_id, *edge, &connected_peers)
                    .map(|target| (*edge, target))
            }) {
                if state.session.on_edge_hit(edge, Some(target)).is_ok() {
                    routing.set_remote_target(target);
                    activation_edge = Some(edge);
                    activation_target = Some(target);
                } else {
                    forwarder.clear_target();
                    return Vec::new();
                }
            } else {
                forwarder.clear_target();
                return Vec::new();
            }
        }
    }

    let target = if let Some(remote_target) = state.session.active_target() {
        if !is_device_connected(state, remote_target) {
            state.session.on_target_disconnect(remote_target);
            routing.clear_remote_target();
            forwarder.clear_target();
            return Vec::new();
        }
        routing.set_remote_target(remote_target);
        remote_target
    } else {
        routing.clear_remote_target();
        forwarder.clear_target();
        return Vec::new();
    };

    let activated_on_this_event = Some(target) != forwarder.target();
    forwarder.set_target(target);
    let event = match (activation_edge, activation_target) {
        (Some(edge), Some(target)) => {
            edge_penetration_mouse_event(state, target, edge, event, routing.edge_threshold)
        }
        _ => event,
    };
    let Some(raw_event) = input_event_to_raw_event_with_modifiers(event, active_modifiers) else {
        return Vec::new();
    };

    let mut messages = forwarder.process_event(raw_event);
    if activated_on_this_event && messages.is_empty() {
        messages = forwarder.flush_batch();
    }
    messages
}

#[derive(Debug)]
struct CapturedInputForwardingOutcome {
    target: Option<DeviceId>,
    messages: Vec<Message>,
    suppress_local_shortcuts: bool,
}

fn captured_input_forwarding_outcome(
    state: &mut DaemonState,
    routing: &mut InputRoutingState,
    forwarder: &mut rshare_core::engine::ForwardingEngine,
    event: InputEvent,
    gamepad_forwarding_enabled: bool,
) -> CapturedInputForwardingOutcome {
    captured_input_forwarding_outcome_with_source(
        state,
        routing,
        forwarder,
        event,
        LocalInputEventSource::Hardware,
        gamepad_forwarding_enabled,
    )
}

fn captured_input_forwarding_outcome_with_source(
    state: &mut DaemonState,
    routing: &mut InputRoutingState,
    forwarder: &mut rshare_core::engine::ForwardingEngine,
    event: InputEvent,
    source: LocalInputEventSource,
    gamepad_forwarding_enabled: bool,
) -> CapturedInputForwardingOutcome {
    if !captured_input_source_should_forward(source) {
        return CapturedInputForwardingOutcome {
            target: None,
            messages: Vec::new(),
            suppress_local_shortcuts: false,
        };
    }

    if !state.features.automatic_input_forwarding {
        if state.session.is_remote_active() {
            state.session.reset();
        }
        routing.clear_remote_target();
        forwarder.clear_target();
        return CapturedInputForwardingOutcome {
            target: None,
            messages: Vec::new(),
            suppress_local_shortcuts: false,
        };
    }

    let messages =
        messages_for_input_event(state, routing, forwarder, event, gamepad_forwarding_enabled);
    let target = state.session.active_target();
    let suppress_local_shortcuts = state.features.suppress_local_shortcuts_when_remote
        && matches!(
            state.session.state(),
            ControlSessionState::RemoteActive { .. }
        );

    CapturedInputForwardingOutcome {
        target,
        messages,
        suppress_local_shortcuts,
    }
}

fn captured_input_source_should_forward(source: LocalInputEventSource) -> bool {
    matches!(source, LocalInputEventSource::Hardware)
}

fn is_quick_return_hotkey(event: &InputEvent, modifiers: ActiveModifiers) -> bool {
    let (keycode, state) = match event {
        InputEvent::Key { keycode, state } | InputEvent::KeyExtended { keycode, state, .. } => {
            (*keycode, *state)
        }
        _ => return false,
    };

    if !state.is_pressed() || !modifiers.ctrl || !modifiers.alt {
        return false;
    }

    matches!(keycode.to_raw(), 0x4C | 0x08 | 0x1B)
}

fn edge_penetration_mouse_event(
    state: &DaemonState,
    target: DeviceId,
    edge: Direction,
    event: InputEvent,
    edge_threshold: u32,
) -> InputEvent {
    let (x, y) = match event {
        InputEvent::MouseMove { x, y } => (x, y),
        other => return other,
    };

    let (target_width, target_height) = target_primary_display_size(state, target).unwrap_or((
        state.local_controls.display.primary_width,
        state.local_controls.display.primary_height,
    ));
    let margin = edge_threshold.max(1) as i32;
    let max_x = target_width.saturating_sub(1) as i32;
    let max_y = target_height.saturating_sub(1) as i32;

    let (mapped_x, mapped_y) = match edge {
        Direction::Right => (margin.min(max_x), y.clamp(0, max_y)),
        Direction::Left => ((max_x - margin).max(0), y.clamp(0, max_y)),
        Direction::Top => (x.clamp(0, max_x), (max_y - margin).max(0)),
        Direction::Bottom => (x.clamp(0, max_x), margin.min(max_y)),
    };

    InputEvent::mouse_move(mapped_x, mapped_y)
}

fn target_primary_display_size(state: &DaemonState, target: DeviceId) -> Option<(u32, u32)> {
    state
        .layout
        .get_node(target)?
        .primary_display()
        .map(|display| (display.width.max(1), display.height.max(1)))
}

fn is_gamepad_input_event(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::GamepadConnected { .. }
            | InputEvent::GamepadDisconnected { .. }
            | InputEvent::GamepadState { .. }
    )
}

fn message_to_input_event(message: Message) -> Option<InputEvent> {
    match message {
        Message::MouseMove { x, y } => Some(InputEvent::mouse_move(x, y)),
        Message::MouseButton { button, state } => Some(InputEvent::mouse_button(
            rshare_input::MouseButton::from_code(button.to_code()),
            input_button_state(state),
        )),
        Message::MouseWheel { delta_x, delta_y } => Some(InputEvent::mouse_wheel(delta_x, delta_y)),
        Message::Key { keycode, state } => Some(InputEvent::key(
            input_keycode_from_message(keycode),
            input_key_state(state),
        )),
        Message::KeyExtended {
            keycode,
            state,
            shift,
            ctrl,
            alt,
            meta,
        } => Some(InputEvent::key_extended(
            input_keycode_from_message(keycode),
            input_key_state(state),
            shift,
            ctrl,
            alt,
            meta,
        )),
        Message::GamepadConnected { info } => Some(InputEvent::gamepad_connected(info)),
        Message::GamepadDisconnected { gamepad_id } => {
            Some(InputEvent::gamepad_disconnected(gamepad_id))
        }
        Message::GamepadState { state } => Some(InputEvent::gamepad_state(state)),
        _ => None,
    }
}

fn input_keycode_from_message(keycode: u32) -> rshare_input::KeyCode {
    if keycode == rshare_input::RSHARE_KEYPAD_ENTER_RAW {
        rshare_input::KeyCode::KeypadEnter
    } else {
        rshare_input::KeyCode::Raw(keycode)
    }
}

fn input_button_state(state: rshare_core::ButtonState) -> rshare_input::ButtonState {
    match state {
        rshare_core::ButtonState::Pressed => rshare_input::ButtonState::Pressed,
        rshare_core::ButtonState::Released => rshare_input::ButtonState::Released,
    }
}

fn input_key_state(state: rshare_core::KeyState) -> rshare_input::ButtonState {
    match state {
        rshare_core::KeyState::Pressed => rshare_input::ButtonState::Pressed,
        rshare_core::KeyState::Released => rshare_input::ButtonState::Released,
    }
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

async fn inject_remote_message(
    inject_backend: &Arc<Mutex<Box<dyn InjectBackend>>>,
    state: &Arc<RwLock<DaemonState>>,
    from: DeviceId,
    message: Message,
) {
    let Some(event) = message_to_input_event(message) else {
        return;
    };
    let loopback_device_kind = injected_input_loopback_device_kind(&event);

    let result = {
        let mut backend = inject_backend.lock().await;
        backend.inject(event)
    };

    match result {
        Ok(()) => {
            if let Some(device_kind) = loopback_device_kind {
                state
                    .write()
                    .await
                    .arm_injected_loopback(device_kind, timestamp_ms_now());
            }
        }
        Err(error) => {
            tracing::warn!("Failed to inject input from {}: {}", from, error);
        }
    }
}

fn injected_input_loopback_device_kind(event: &InputEvent) -> Option<LocalInputDeviceKind> {
    match event {
        InputEvent::Key { .. } | InputEvent::KeyExtended { .. } | InputEvent::TextCommit { .. } => {
            Some(LocalInputDeviceKind::Keyboard)
        }
        InputEvent::MouseMove { .. }
        | InputEvent::MouseButton { .. }
        | InputEvent::MouseWheel { .. } => Some(LocalInputDeviceKind::Mouse),
        _ => None,
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
    inject_backend: &Arc<Mutex<Box<dyn InjectBackend>>>,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    test: LocalInputTestRequest,
) -> LocalInputTestResult {
    let mut diagnostic_payload = BTreeMap::new();
    let result = match test.kind {
        LocalInputTestKind::KeyboardShift => {
            let mut backend = inject_backend.lock().await;
            if !backend.is_active() {
                return LocalInputTestResult::failed(
                    LocalInputTestStatus::BackendUnavailable,
                    "Input injection backend is not active.",
                );
            }
            backend
                .inject(InputEvent::key(
                    rshare_input::KeyCode::ShiftLeft,
                    rshare_input::ButtonState::Pressed,
                ))
                .and_then(|_| {
                    backend.inject(InputEvent::key(
                        rshare_input::KeyCode::ShiftLeft,
                        rshare_input::ButtonState::Released,
                    ))
                })
        }
        LocalInputTestKind::MouseMove => {
            let (x, y) = {
                let state = state.read().await;
                (state.local_controls.mouse.x, state.local_controls.mouse.y)
            };
            let mut backend = inject_backend.lock().await;
            if !backend.is_active() {
                return LocalInputTestResult::failed(
                    LocalInputTestStatus::BackendUnavailable,
                    "Input injection backend is not active.",
                );
            }
            let ((first_x, first_y), (second_x, second_y)) =
                ((x.saturating_add(8), y.saturating_add(8)), (x, y));
            diagnostic_payload.insert("x".to_string(), first_x.to_string());
            diagnostic_payload.insert("y".to_string(), first_y.to_string());
            diagnostic_payload.insert("return_x".to_string(), second_x.to_string());
            diagnostic_payload.insert("return_y".to_string(), second_y.to_string());
            backend
                .inject(InputEvent::mouse_move(first_x, first_y))
                .and_then(|_| backend.inject(InputEvent::mouse_move(second_x, second_y)))
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
            payload.insert("text".to_string(), text.clone());
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
    target: DeviceId,
    origin_sequence: u64,
) {
    let now = timestamp_ms_now();
    let Some((local_device_id, sequence, event)) = ({
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
            Some((state.status.device_id, sequence, event))
        }
    }) else {
        return;
    };

    let _ = local_events_tx.send(event.clone());
    broadcast_diagnostic_event(network_manager, local_device_id, event).await;

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
        broadcast_diagnostic_event(network_manager, local_device_id, event).await;
    }
}

async fn handle_network_message(
    state: &Arc<RwLock<DaemonState>>,
    network_manager: &Arc<Mutex<NetworkManager>>,
    inject_backend: &Arc<Mutex<Box<dyn InjectBackend>>>,
    audio_runtime: &audio_runtime::AudioRuntimeHandle,
    usb_runtime: &UsbHostRuntime,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    endpoint_events_tx: &broadcast::Sender<EndpointEvent>,
    from: DeviceId,
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
            let events = {
                let mut state = state.write().await;
                state.endpoint_events(&filter, None, Some(128))
            };
            let result = {
                let mut manager = network_manager.lock().await;
                manager
                    .send_to(&from, Message::EndpointEventSnapshot { events })
                    .await
            };
            if let Err(error) = result {
                tracing::debug!(
                    "Failed to answer endpoint event subscription from {}: {}",
                    from,
                    error
                );
            }
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
            let (event, should_broadcast, local_device_id) = {
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
                (event, should_broadcast, state.status.device_id)
            };
            let _ = local_events_tx.send(event.clone());
            if should_broadcast {
                broadcast_diagnostic_event(network_manager, local_device_id, event).await;
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
        other => {
            inject_remote_message(inject_backend, state, from, other).await;
        }
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

async fn send_forwarded_messages(
    network_manager: &Arc<Mutex<NetworkManager>>,
    target: DeviceId,
    messages: Vec<Message>,
) {
    for message in messages {
        let result = {
            let mut manager = network_manager.lock().await;
            manager.send_to(&target, message).await
        };

        if let Err(error) = result {
            tracing::warn!("Failed to forward input to {}: {}", target, error);
        }
    }
}

async fn run_input_forwarding_loop(
    mut input_rx: tokio::sync::mpsc::UnboundedReceiver<CapturedInputEvent>,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
    edge_threshold: u32,
    gamepad_forwarding_enabled: bool,
) -> Result<()> {
    let mut forwarder = rshare_core::engine::ForwardingEngine::new();
    let mut routing = InputRoutingState::default_with_threshold(edge_threshold);
    let mut flush_interval = tokio::time::interval(Duration::from_millis(2));

    loop {
        tokio::select! {
            captured = input_rx.recv() => {
                let Some(captured) = captured else {
                    break;
                };
                let event = captured.event;
                let metadata = captured.metadata;

                let (target, messages, diagnostic, suppress_local_shortcuts) = {
                    let mut state = state.write().await;
                    let local_event =
                        state.record_local_input_event_with_metadata(&event, metadata.as_ref());
                    let source = local_event.source;
                    let diagnostic = (state.status.device_id, local_event.clone());
                    let _ = local_events_tx.send(local_event);
                    let outcome = captured_input_forwarding_outcome_with_source(
                        &mut state,
                        &mut routing,
                        &mut forwarder,
                        event,
                        source,
                        gamepad_forwarding_enabled,
                    );
                    (
                        outcome.target,
                        outcome.messages,
                        diagnostic,
                        outcome.suppress_local_shortcuts,
                    )
                };
                set_local_shortcut_suppression(suppress_local_shortcuts);

                broadcast_diagnostic_event(&network_manager, diagnostic.0, diagnostic.1).await;

                if let Some(target) = target {
                    send_forwarded_messages(&network_manager, target, messages).await;
                }
                if let Some(return_edge) = routing.take_pending_return_edge() {
                    let mut state = state.write().await;
                    let _ = state.session.on_return_edge_hit(return_edge);
                    routing.clear_remote_target();
                    forwarder.clear_target();
                    set_local_shortcut_suppression(false);
                }
            }
            _ = flush_interval.tick() => {
                if !forwarder.should_flush_batch() {
                    continue;
                }

                let target = {
                    let mut state = state.write().await;
                    if !state.features.automatic_input_forwarding {
                        if state.session.is_remote_active() {
                            state.session.reset();
                        }
                        None
                    } else {
                        state
                            .session
                            .active_target()
                            .filter(|target| is_device_connected(&state, *target))
                    }
                };

                let Some(target) = target else {
                    set_local_shortcut_suppression(false);
                    routing.clear_remote_target();
                    forwarder.clear_target();
                    continue;
                };

                forwarder.set_target(target);
                let messages = forwarder.flush_batch();
                send_forwarded_messages(&network_manager, target, messages).await;
            }
            _ = shutdown_rx.recv() => {
                break;
            }
        }
    }

    set_local_shortcut_suppression(false);
    Ok(())
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
async fn run_windows_driver_capture_loop(
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
    edge_threshold: u32,
) -> Result<()> {
    let (driver_tx, mut driver_rx) =
        tokio::sync::mpsc::unbounded_channel::<rshare_platform::windows::WindowsDriverInputEvent>();

    tokio::task::spawn_blocking(move || {
        let client = match rshare_platform::windows::WindowsDriverClient::open() {
            Ok(client) => client,
            Err(error) => {
                tracing::info!(
                    "RShare Windows driver unavailable, using fallback input path: {error}"
                );
                return;
            }
        };

        loop {
            match client.read_event() {
                Ok(event) => {
                    if driver_tx.send(event).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    if rshare_platform::windows::is_driver_event_queue_empty(&error) {
                        std::thread::sleep(std::time::Duration::from_millis(16));
                        continue;
                    }
                    tracing::warn!("RShare Windows driver event read failed: {error}");
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
        }
    });

    let mut forwarder = rshare_core::engine::ForwardingEngine::new();
    let mut routing = InputRoutingState::default_with_threshold(edge_threshold);

    loop {
        tokio::select! {
            event = driver_rx.recv() => {
                let Some(driver_event) = event else {
                    break;
                };

                let (target, messages, diagnostic, suppress_local_shortcuts) = {
                    let mut state = state.write().await;
                    let Some(input_event) = input_event_from_windows_driver_event_with_pointer(
                        &driver_event,
                        state.local_controls.mouse.x,
                        state.local_controls.mouse.y,
                        &state.local_controls.display,
                    ) else {
                        continue;
                    };
                    let mut local_event =
                        state.record_local_input_event_with_metadata(&input_event, None);
                    local_event.device_id = Some(driver_event.device_id.clone());
                    local_event.device_instance_id = Some(driver_event.device_instance_id.clone());
                    local_event.capture_path = Some("rshare-filter".to_string());
                    local_event.source = resolve_windows_driver_local_source(
                        local_event.source,
                        local_source_from_windows_driver_event(driver_event.source),
                    );
                    let source = local_event.source;
                    local_event.payload.insert("driver_flags".to_string(), driver_event.flags.to_string());
                    update_driver_device_from_event(&mut state.local_controls, &driver_event, local_event.timestamp_ms);
                    replace_recent_local_event(&mut state.local_controls, local_event.clone());
                    let diagnostic = (state.status.device_id, local_event.clone());
                    let _ = local_events_tx.send(local_event);
                    let outcome = captured_input_forwarding_outcome_with_source(
                        &mut state,
                        &mut routing,
                        &mut forwarder,
                        input_event,
                        source,
                        true,
                    );
                    (
                        outcome.target,
                        outcome.messages,
                        diagnostic,
                        outcome.suppress_local_shortcuts,
                    )
                };
                set_local_shortcut_suppression(suppress_local_shortcuts);

                broadcast_diagnostic_event(&network_manager, diagnostic.0, diagnostic.1).await;

                if let Some(target) = target {
                    send_forwarded_messages(&network_manager, target, messages).await;
                }
                if let Some(return_edge) = routing.take_pending_return_edge() {
                    let mut state = state.write().await;
                    let _ = state.session.on_return_edge_hit(return_edge);
                    routing.clear_remote_target();
                    forwarder.clear_target();
                    set_local_shortcut_suppression(false);
                }
            }
            _ = shutdown_rx.recv() => break,
        }
    }

    set_local_shortcut_suppression(false);
    Ok(())
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
fn try_start_evdev_capture(
    tx: tokio::sync::mpsc::UnboundedSender<CapturedInputEvent>,
) -> Result<tokio::task::JoinHandle<()>> {
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

            let Some(input_event) = captured_input_from_evdev_driver_event(evdev_event) else {
                return;
            };

            if tx.send(input_event).is_err() {
                tracing::warn!("Failed to send input event through channel");
            }
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
fn captured_input_from_evdev_driver_event(
    event: rshare_platform::EvdevDriverEvent,
) -> Option<CapturedInputEvent> {
    use rshare_input::{ButtonState, KeyCode, MouseButton};
    use rshare_platform::EvdevDriverEvent;

    let (event, device_kind, device_path) = match event {
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

    let event_name = Path::new(&device_path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string);
    let kind_label = match device_kind {
        LocalInputDeviceKind::Keyboard => "keyboard",
        LocalInputDeviceKind::Mouse => "mouse",
        _ => return None,
    };
    let device_id = event_name
        .as_ref()
        .map(|event_name| format!("linux-evdev-{kind_label}-{event_name}"))
        .unwrap_or_else(|| format!("linux-evdev-{kind_label}-unknown"));

    Some(CapturedInputEvent {
        event,
        metadata: Some(LocalInputDeviceMetadata {
            device_id,
            device_instance_id: event_name,
            capture_path: Some(device_path),
        }),
    })
}

fn forward_input_events_to_captured_channel(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<InputEvent>,
    tx: tokio::sync::mpsc::UnboundedSender<CapturedInputEvent>,
) {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if tx.send(CapturedInputEvent::from(event)).is_err() {
                break;
            }
        }
    });
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
            ..Default::default()
        });

    let mut events = network_manager.events();
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
    let last_backend_error = inject_error.or(backend_error);

    // Initialize backend state
    {
        let mut s = state.write().await;
        s.update_backend_state(
            input_mode,
            available_backends,
            backend_health.clone(), // capture health
            inject_health,
            last_backend_error,
        );
    }
    let inject_backend = Arc::new(Mutex::new(inject_backend));

    let ipc_listener = TcpListener::bind(default_ipc_addr()).await?;
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(8);
    let (local_events_tx, _) = broadcast::channel::<LocalInputDiagnosticEvent>(256);
    let (endpoint_events_tx, _) = broadcast::channel::<EndpointEvent>(256);
    let audio_runtime = audio_runtime::AudioRuntimeHandle::start()?;
    let usb_runtime = Arc::new(Mutex::new(
        rshare_platform::ExperimentalUsbHostRuntime::new(),
    ));

    // Input capture: try Evdev on Linux for kernel-level access, fallback to RDev
    #[cfg(target_os = "linux")]
    let (input_rx, input_channel, gamepad_input_channel) = {
        use tokio::sync::mpsc;

        let (tx, rx) = mpsc::unbounded_channel::<CapturedInputEvent>();
        let (gamepad_input_channel, gamepad_rx) = InputEventChannel::new();
        forward_input_events_to_captured_channel(gamepad_rx, tx.clone());

        // Try to use EvdevCaptureBackend for kernel-level input capture
        match try_start_evdev_capture(tx.clone()) {
            Ok(_evdev_task) => {
                tracing::info!("Using Evdev backend for input capture (kernel-level)");
                // Evdev capture is running in the background task
                (rx, None, gamepad_input_channel)
            }
            Err(e) => {
                tracing::warn!(
                    "Evdev capture unavailable: {:?}, using RDev (Portable) backend",
                    e
                );
                // Fallback to RDev (Portable) backend
                let mut input_listener = RDevInputListener::new();
                let rdev_rx = input_listener.receiver();
                forward_input_events_to_captured_channel(rdev_rx, tx);
                let channel = Some(input_listener);
                (rx, channel, gamepad_input_channel)
            }
        }
    };

    #[cfg(windows)]
    let (input_rx, input_event_channel, _input_listener, use_windows_filter_capture) = {
        let (input_event_channel, raw_input_rx) = InputEventChannel::new();
        let (captured_tx, input_rx) = tokio::sync::mpsc::unbounded_channel::<CapturedInputEvent>();
        forward_input_events_to_captured_channel(raw_input_rx, captured_tx);
        set_local_shortcut_suppression(false);
        let use_windows_filter_capture = {
            let state = state.read().await;
            windows_should_use_filter_capture(input_mode, &state.local_controls.driver)
        };

        if use_windows_filter_capture {
            tracing::info!("Using RShare Windows filter driver input capture");
            (input_rx, input_event_channel, None, true)
        } else {
            let callback_channel = input_event_channel.clone();
            let mut input_listener = DefaultInputListener::new();
            input_listener.start(Box::new(move |event| {
                let _ = callback_channel.send(event);
            }))?;
            tracing::info!("Using native Windows low-level hook input capture");
            (input_rx, input_event_channel, Some(input_listener), false)
        }
    };

    #[cfg(all(not(target_os = "linux"), not(windows)))]
    let (input_rx, mut input_channel) = {
        let mut input_listener = RDevInputListener::new();
        let raw_input_rx = input_listener.receiver();
        let (captured_tx, input_rx) = tokio::sync::mpsc::unbounded_channel::<CapturedInputEvent>();
        forward_input_events_to_captured_channel(raw_input_rx, captured_tx);
        (input_rx, Some(input_listener))
    };

    let mut gamepad_listener_config = GamepadListenerConfig::from(&config.gamepad);
    gamepad_listener_config.enabled = true;
    let mut gamepad_listener = {
        #[cfg(target_os = "linux")]
        {
            GilrsGamepadListener::new(gamepad_input_channel.clone(), gamepad_listener_config)
        }
        #[cfg(windows)]
        {
            GilrsGamepadListener::new(input_event_channel.clone(), gamepad_listener_config)
        }
        #[cfg(all(not(target_os = "linux"), not(windows)))]
        {
            GilrsGamepadListener::new(
                input_channel.as_ref().unwrap().channel(),
                gamepad_listener_config,
            )
        }
    };
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

    let ipc_task = tokio::spawn(run_ipc_server(
        ipc_listener,
        state.clone(),
        network_manager.clone(),
        inject_backend.clone(),
        audio_runtime.clone(),
        usb_runtime.clone(),
        local_events_tx.clone(),
        endpoint_events_tx.clone(),
        layout_path.clone(),
        shutdown_tx.clone(),
    ));
    let local_controls_ws_task = tokio::spawn(run_local_controls_ws_server(
        state.clone(),
        local_events_tx.clone(),
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
    let mobile_gateway_shutdown_rx = shutdown_tx.subscribe();
    let mobile_gateway_task = async move {
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
            std::future::pending::<Result<()>>().await
        }
    };

    let input_forwarding_task = tokio::spawn(run_input_forwarding_loop(
        input_rx,
        state.clone(),
        network_manager.clone(),
        local_events_tx.clone(),
        shutdown_tx.subscribe(),
        config.edge_threshold(),
        config.gamepad.enabled,
    ));

    #[cfg(windows)]
    let _windows_driver_capture_task = if use_windows_filter_capture {
        Some(tokio::spawn(run_windows_driver_capture_loop(
            state.clone(),
            network_manager.clone(),
            local_events_tx.clone(),
            shutdown_tx.subscribe(),
            config.edge_threshold(),
        )))
    } else {
        None
    };

    let event_task = {
        let state = state.clone();
        let inject_backend = inject_backend.clone();
        let network_manager = network_manager.clone();
        let audio_runtime = audio_runtime.clone();
        let usb_runtime = usb_runtime.clone();
        let local_events_tx = local_events_tx.clone();
        let endpoint_events_tx = endpoint_events_tx.clone();
        let layout_path = layout_path.clone();
        tokio::spawn(async move {
            tracing::info!("Event task: starting to wait for events");
            while let Some(event) = events.recv().await {
                match event {
                    NetworkEvent::DeviceFound(device) => {
                        let layout_to_save = {
                            let mut state = state.write().await;
                            state.upsert_discovered(device);
                            state.layout.clone()
                        };
                        if let Err(err) = save_layout_to_path(&layout_to_save, layout_path.as_ref())
                        {
                            tracing::warn!("Failed to persist auto-updated layout: {}", err);
                        }
                    }
                    NetworkEvent::DeviceConnected(id) => {
                        let (should_advertise_usb, layout_to_save) = {
                            let mut state = state.write().await;
                            let layout_changed = state.mark_connected(&id, true);
                            (
                                state.features.usb_advertising_enabled(),
                                layout_changed.then(|| state.layout.clone()),
                            )
                        };
                        if let Some(layout_to_save) = layout_to_save {
                            if let Err(err) =
                                save_layout_to_path(&layout_to_save, layout_path.as_ref())
                            {
                                tracing::warn!(
                                    "Failed to persist connected-device layout: {}",
                                    err
                                );
                            }
                        }
                        if should_advertise_usb {
                            advertise_usb_devices_to(&network_manager, &usb_runtime, id).await;
                        }
                    }
                    NetworkEvent::DeviceDisconnected(id) => {
                        let mut state = state.write().await;
                        // Notify session state machine of target disconnection
                        state.session.on_target_disconnect(id);
                        fail_pending_usb_for_device(
                            &mut state,
                            id,
                            "USB probe target disconnected.",
                        );
                        state.remove_device(&id);
                        sync_local_shortcut_suppression(&state);
                    }
                    NetworkEvent::MessageReceived { from, message } => {
                        handle_network_message(
                            &state,
                            &network_manager,
                            &inject_backend,
                            &audio_runtime,
                            &usb_runtime,
                            &local_events_tx,
                            &endpoint_events_tx,
                            from,
                            message,
                        )
                        .await;
                    }
                    NetworkEvent::ConnectionError { device_id, error } => {
                        tracing::warn!("Connection error to {}: {}", device_id, error);
                        let mut state = state.write().await;
                        state.session.on_target_disconnect(device_id);
                        fail_pending_usb_for_device(
                            &mut state,
                            device_id,
                            "USB probe target connection failed.",
                        );
                        state.mark_connected(&device_id, false);
                        sync_local_shortcut_suppression(&state);
                    }
                }
            }
            tracing::debug!("Event task: events channel closed");
        })
    };

    tracing::info!("Entering tokio::select! loop");
    tokio::select! {
        result = signal::ctrl_c() => {
            match result {
                Ok(()) => tracing::info!("Shutdown signal received"),
                Err(e) => tracing::warn!("Ctrl-C handler error: {}", e),
            }
        }
        _ = shutdown_rx.recv() => {
            tracing::info!("Shutdown requested over IPC");
        }
        result = ipc_task => {
            tracing::info!("IPC task completed");
            result??;
        }
        result = local_controls_ws_task => {
            tracing::info!("Local controls websocket task completed");
            result??;
        }
        result = mobile_gateway_task => {
            tracing::info!("Mobile gateway task completed");
            result?;
        }
        result = event_task => {
            tracing::info!("Event task completed");
            result?;
        }
        result = input_forwarding_task => {
            tracing::info!("Input forwarding task completed");
            result??;
        }
    }

    tracing::info!("tokio::select! exited, cleaning up");
    set_local_shortcut_suppression(false);
    audio_runtime.stop_capture();
    audio_runtime.stop_render();
    audio_runtime.shutdown();
    // Input listener cleanup is handled automatically by task drops
    network_manager.lock().await.stop().await?;

    tracing::info!("R-ShareMouse daemon stopped");
    std::process::exit(0);
}

async fn run_ipc_server(
    listener: TcpListener,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    inject_backend: Arc<Mutex<Box<dyn InjectBackend>>>,
    audio_runtime: audio_runtime::AudioRuntimeHandle,
    usb_runtime: UsbHostRuntime,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    endpoint_events_tx: broadcast::Sender<EndpointEvent>,
    layout_path: Arc<PathBuf>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let network_manager = network_manager.clone();
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
                state,
                network_manager,
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

async fn run_local_controls_ws_server(
    state: Arc<RwLock<DaemonState>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(default_local_controls_ws_addr()).await?;
    tracing::info!(
        "Local controls websocket listening on {}",
        default_local_controls_ws_addr()
    );

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let state = state.clone();
                let local_events_tx = local_events_tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_local_controls_ws_client(stream, state, local_events_tx).await {
                        tracing::debug!("Local controls websocket client error: {}", error);
                    }
                });
            }
            _ = shutdown_rx.recv() => break,
        }
    }

    Ok(())
}

async fn handle_local_controls_ws_client(
    stream: TcpStream,
    state: Arc<RwLock<DaemonState>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
) -> Result<()> {
    let mut websocket = accept_async(stream).await?;
    let snapshot = {
        let mut state = state.write().await;
        state.refresh_local_controls_platform();
        state.local_control_snapshot()
    };
    websocket
        .send(WsMessage::Text(serde_json::to_string(
            &DaemonResponse::LocalControls(snapshot),
        )?))
        .await?;

    let mut events = local_events_tx.subscribe();
    loop {
        let event = events.recv().await?;
        websocket
            .send(WsMessage::Text(serde_json::to_string(
                &DaemonResponse::LocalControlEvent(event),
            )?))
            .await?;
    }
}

async fn handle_ipc_client(
    mut stream: TcpStream,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    inject_backend: Arc<Mutex<Box<dyn InjectBackend>>>,
    audio_runtime: audio_runtime::AudioRuntimeHandle,
    usb_runtime: UsbHostRuntime,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    endpoint_events_tx: broadcast::Sender<EndpointEvent>,
    layout_path: Arc<PathBuf>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    let request: DaemonRequest = read_json_line(&mut stream).await?;

    if matches!(request, DaemonRequest::SubscribeLocalControls) {
        let snapshot = {
            let state = state.read().await;
            state.local_control_snapshot()
        };
        write_json_line(&mut stream, &DaemonResponse::LocalControls(snapshot)).await?;
        let mut events = local_events_tx.subscribe();
        loop {
            match events.recv().await {
                Ok(event) => {
                    write_json_line(&mut stream, &DaemonResponse::LocalControlEvent(event)).await?;
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
        write_json_line(&mut stream, &DaemonResponse::EndpointEvents(events)).await?;
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
                                write_json_line(
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
                                write_json_line(
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
                            let layout_to_save = {
                                let mut state = state.write().await;
                                state
                                    .mark_connected(&device_id, true)
                                    .then(|| state.layout.clone())
                            };
                            if let Some(layout_to_save) = layout_to_save {
                                if let Err(err) =
                                    save_layout_to_path(&layout_to_save, layout_path.as_ref())
                                {
                                    tracing::warn!(
                                        "Failed to persist connected-device layout: {}",
                                        err
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
                    state.mark_connected(&device_id, false);
                    sync_local_shortcut_suppression(&state);
                    DaemonResponse::Ack
                }
                Err(err) => DaemonResponse::Error(err.to_string()),
            }
        }
        DaemonRequest::GetLayout => {
            let state = state.read().await;
            DaemonResponse::Layout(state.layout.clone())
        }
        DaemonRequest::SetLayout { layout } => {
            let mut state = state.write().await;
            let mut canonical_layout = layout;
            canonical_layout.canonicalize_local_device(state.status.device_id);

            match save_layout_to_path(&canonical_layout, layout_path.as_ref()) {
                Ok(()) => {
                    state.layout = canonical_layout;
                    DaemonResponse::Ack
                }
                Err(err) => DaemonResponse::Error(err.to_string()),
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
        DaemonRequest::SubscribeLocalControls => unreachable!("handled before response match"),
        DaemonRequest::SubscribeEndpointEvents { .. } => {
            unreachable!("handled before response match")
        }
        DaemonRequest::Shutdown => {
            let _ = shutdown_tx.send(());
            DaemonResponse::Ack
        }
    };

    write_json_line(&mut stream, &response).await
}

#[cfg(test)]
fn apply_layout_update(state: &mut DaemonState, mut layout: LayoutGraph) {
    layout.canonicalize_local_device(state.status.device_id);
    state.layout = layout;
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
    let mut config = match Config::load() {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(
                "Failed to load persisted config: {}. Falling back to default config.",
                error
            );
            Config::default()
        }
    };

    if let Ok(bind) = std::env::var("RSHARE_BIND") {
        config.network.bind_address = bind;
    }

    if let Ok(port) = std::env::var("RSHARE_PORT") {
        config.network.port = port.parse()?;
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    #[test]
    fn default_daemon_state_disables_mobile_access_without_credentials() {
        let snapshot = test_daemon_state().mobile_access.snapshot();

        assert!(!snapshot.enabled);
        assert_eq!(snapshot.page_url, "不可用");
        assert!(snapshot.token.is_empty());
    }

    fn connected_connection_info(device_id: DeviceId, rtt_ms: Option<u64>) -> ConnectionInfo {
        let mut info = ConnectionInfo::new(device_id, "127.0.0.1:27431".to_string());
        info.state = rshare_net::connection::ConnectionState::Connected;
        info.datagram_available = true;
        info.rtt_ms = rtt_ms;
        info
    }

    fn assert_virtual_display_create_status(status: rshare_core::VirtualDisplayOperationStatus) {
        if cfg!(windows) {
            assert!(
                matches!(
                    status,
                    rshare_core::VirtualDisplayOperationStatus::Created
                        | rshare_core::VirtualDisplayOperationStatus::DriverUnavailable
                ),
                "unexpected virtual display create status: {status:?}"
            );
        } else {
            assert_eq!(
                status,
                rshare_core::VirtualDisplayOperationStatus::Unsupported
            );
        }
    }

    fn assert_virtual_display_remove_status(status: rshare_core::VirtualDisplayOperationStatus) {
        if cfg!(windows) {
            assert!(
                matches!(
                    status,
                    rshare_core::VirtualDisplayOperationStatus::Removed
                        | rshare_core::VirtualDisplayOperationStatus::DriverUnavailable
                ),
                "unexpected virtual display remove status: {status:?}"
            );
        } else {
            assert_eq!(
                status,
                rshare_core::VirtualDisplayOperationStatus::Unsupported
            );
        }
    }

    fn cleanup_created_virtual_display(
        id: &str,
        status: rshare_core::VirtualDisplayOperationStatus,
    ) {
        if status == rshare_core::VirtualDisplayOperationStatus::Created {
            let _ = rshare_platform::virtual_display::remove_virtual_display(
                &rshare_core::VirtualDisplayRemoveRequest { id: id.to_string() },
            );
        }
    }

    #[test]
    fn virtual_display_manager_records_platform_create_result() {
        let mut manager = VirtualDisplayManager::default();

        let result = manager.create(rshare_core::VirtualDisplayCreateRequest {
            id: Some("vd-1".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        });

        assert_virtual_display_create_status(result.status);
        assert_eq!(manager.list().len(), 1);
        assert_eq!(manager.list()[0].id, "vd-1");
        cleanup_created_virtual_display("vd-1", result.status);
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
        let result = manager.create(request);

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

        let result = manager.create(rshare_core::VirtualDisplayCreateRequest {
            id: Some("vd-1".to_string()),
            width: 2560,
            height: 1440,
            refresh_rate_millihz: Some(144_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        });

        assert_virtual_display_create_status(result.status);
        assert_ne!(
            result.status,
            rshare_core::VirtualDisplayOperationStatus::AlreadyExists
        );
        cleanup_created_virtual_display("vd-1", result.status);
    }

    #[test]
    fn virtual_display_manager_handles_retry_after_platform_create_result() {
        let mut manager = VirtualDisplayManager::default();
        let request = rshare_core::VirtualDisplayCreateRequest {
            id: Some("vd-1".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        };

        let first = manager.create(request.clone());
        let retry = manager.create(request);

        assert_virtual_display_create_status(first.status);
        if first.status == rshare_core::VirtualDisplayOperationStatus::Created {
            assert_eq!(
                retry.status,
                rshare_core::VirtualDisplayOperationStatus::AlreadyExists
            );
        } else {
            assert_eq!(retry.status, first.status);
        }
        assert_eq!(manager.list().len(), 1);
        assert_eq!(manager.list()[0].id, "vd-1");
        cleanup_created_virtual_display("vd-1", first.status);
    }

    #[test]
    fn virtual_display_manager_reuses_default_id_after_existing_default_record() {
        let mut manager = VirtualDisplayManager::default();

        let first = manager.create(rshare_core::VirtualDisplayCreateRequest {
            id: None,
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        });
        let retry = manager.create(rshare_core::VirtualDisplayCreateRequest {
            id: None,
            width: 2560,
            height: 1440,
            refresh_rate_millihz: Some(144_000),
            name: Some("R-ShareMouse Virtual Display".to_string()),
        });

        assert_eq!(
            first.display.as_ref().map(|display| display.id.as_str()),
            Some("rshare-vdisplay-1")
        );
        assert_eq!(
            retry.display.as_ref().map(|display| display.id.as_str()),
            Some("rshare-vdisplay-1")
        );
        assert_eq!(manager.list().len(), 1);
        cleanup_created_virtual_display(DEFAULT_VIRTUAL_DISPLAY_ID, first.status);
        cleanup_created_virtual_display(DEFAULT_VIRTUAL_DISPLAY_ID, retry.status);
    }

    #[test]
    fn virtual_display_manager_rejects_invalid_modes() {
        let mut manager = VirtualDisplayManager::default();

        let result = manager.create(rshare_core::VirtualDisplayCreateRequest {
            id: Some("vd-invalid".to_string()),
            width: 0,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: None,
        });

        assert_eq!(
            result.status,
            rshare_core::VirtualDisplayOperationStatus::InvalidMode
        );
        assert!(manager.list().is_empty());
    }

    #[test]
    fn virtual_display_manager_removes_requested_snapshot() {
        let mut manager = VirtualDisplayManager::default();
        let _ = manager.create(rshare_core::VirtualDisplayCreateRequest {
            id: Some("vd-1".to_string()),
            width: 1920,
            height: 1080,
            refresh_rate_millihz: Some(60_000),
            name: None,
        });

        let result = manager.remove(rshare_core::VirtualDisplayRemoveRequest {
            id: "vd-1".to_string(),
        });

        assert_virtual_display_remove_status(result.status);
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
        };

        let response = display_capture_response_from_result(
            &request,
            Err(anyhow::anyhow!("capture backend failed")),
        );

        match response {
            DaemonResponse::DisplayCapture(result) => {
                assert_eq!(result.status, DisplayOperationStatus::ApplyFailed);
                assert_eq!(result.display_id, "display-1");
                assert!(result.bytes.is_empty());
                assert_eq!(result.message.as_deref(), Some("capture backend failed"));
            }
            other => panic!("expected display capture result, got {other:?}"),
        }
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
        let backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(TestInjectBackend {
                active: true,
                fail: false,
                injected: Vec::new(),
            })));
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
        let backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(RecordingKindInjectBackend {
                kind: BackendKind::VirtualHid,
                injected: injected.clone(),
            })));
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

    #[tokio::test]
    async fn remote_message_injection_marks_immediate_capture_feedback_as_loopback() {
        let backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(TestInjectBackend {
                active: true,
                fail: false,
                injected: Vec::new(),
            })));
        let state = Arc::new(RwLock::new(test_daemon_state()));
        let remote_id = DeviceId::new_v4();

        inject_remote_message(
            &backend,
            &state,
            remote_id,
            Message::Key {
                keycode: 0x20,
                state: rshare_core::KeyState::Pressed,
            },
        )
        .await;

        let feedback =
            state
                .write()
                .await
                .record_local_input_event(&rshare_input::InputEvent::key(
                    rshare_input::KeyCode::Space,
                    rshare_input::ButtonState::Pressed,
                ));

        assert_eq!(feedback.source, LocalInputEventSource::InjectedLoopback);
    }

    #[test]
    fn injected_loopback_capture_is_not_forwarded_back_to_remote_peer() {
        use rshare_core::{Direction, LayoutLink};

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
        state.features.automatic_input_forwarding = true;
        state.layout.upsert_link_for_edge(LayoutLink::new(
            local_id,
            Direction::Right,
            remote_id,
            Direction::Left,
        ));
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let _ = captured_input_forwarding_outcome(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
            true,
        );
        let outcome = captured_input_forwarding_outcome_with_source(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key(
                rshare_input::KeyCode::Space,
                rshare_input::ButtonState::Pressed,
            ),
            LocalInputEventSource::InjectedLoopback,
            true,
        );

        assert!(outcome.messages.is_empty());
        assert_eq!(outcome.target, None);
        assert_eq!(forwarder.target(), Some(remote_id));
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
        let backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(TestInjectBackend {
                active: false,
                fail: false,
                injected: Vec::new(),
            })));
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
        let backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(TestInjectBackend {
                active: true,
                fail: false,
                injected: Vec::new(),
            })));
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
        let backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(RecordingKindInjectBackend {
                kind: BackendKind::Portable,
                injected: injected.clone(),
            })));
        let state = Arc::new(RwLock::new(test_daemon_state()));
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
        let backend: Arc<Mutex<Box<dyn InjectBackend>>> =
            Arc::new(Mutex::new(Box::new(TestInjectBackend {
                active: true,
                fail: false,
                injected: Vec::new(),
            })));
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

    #[test]
    fn input_event_maps_to_forwarding_raw_event() {
        let raw = input_event_to_raw_event(rshare_input::InputEvent::mouse_button(
            rshare_input::MouseButton::Back,
            rshare_input::ButtonState::Pressed,
        ))
        .unwrap();

        match raw {
            rshare_core::engine::RawInputEvent::MouseButton { button, pressed } => {
                assert_eq!(button, 4);
                assert!(pressed);
            }
            _ => panic!("Wrong raw input event"),
        }
    }

    #[test]
    fn gamepad_input_event_maps_to_forwarding_raw_event() {
        let raw = input_event_to_raw_event(rshare_input::InputEvent::gamepad_state(
            rshare_core::GamepadState::neutral(0, 1, 123),
        ))
        .unwrap();

        assert!(matches!(
            raw,
            rshare_core::engine::RawInputEvent::GamepadState { .. }
        ));
    }

    #[test]
    fn keypad_enter_uses_distinct_forwarding_keycode() {
        let raw = input_event_to_raw_event(rshare_input::InputEvent::key(
            rshare_input::KeyCode::KeypadEnter,
            rshare_input::ButtonState::Pressed,
        ))
        .unwrap();

        assert!(matches!(
            raw,
            rshare_core::engine::RawInputEvent::Key {
                keycode: rshare_input::RSHARE_KEYPAD_ENTER_RAW,
                pressed: true,
            }
        ));
    }

    #[test]
    fn forwarded_keypad_enter_restores_key_identity() {
        let input = message_to_input_event(rshare_core::Message::Key {
            keycode: rshare_input::RSHARE_KEYPAD_ENTER_RAW,
            state: rshare_core::KeyState::Pressed,
        })
        .unwrap();

        assert!(matches!(
            input,
            rshare_input::InputEvent::Key {
                keycode: rshare_input::KeyCode::KeypadEnter,
                state: rshare_input::ButtonState::Pressed,
            }
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

    #[test]
    fn input_event_forwarding_requires_connected_target() {
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            DeviceId::new_v4(),
            "local".to_string(),
            "local-host".to_string(),
            "127.0.0.1:27431".to_string(),
            27432,
            1,
        ));
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key(
                rshare_input::KeyCode::Raw(0x20),
                rshare_input::ButtonState::Pressed,
            ),
            true,
        );

        assert!(messages.is_empty());
    }

    #[test]
    fn input_event_forwarding_stays_local_until_edge_activation() {
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
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key(
                rshare_input::KeyCode::Raw(0x20),
                rshare_input::ButtonState::Pressed,
            ),
            true,
        );

        assert!(messages.is_empty());
        assert_eq!(forwarder.target(), None);
    }

    #[test]
    fn captured_input_does_not_auto_forward_when_disabled_by_config() {
        use rshare_core::{Direction, LayoutLink};

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
        state.features.automatic_input_forwarding = false;
        state.layout.upsert_link_for_edge(LayoutLink::new(
            local_id,
            Direction::Right,
            remote_id,
            Direction::Left,
        ));
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );

        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);
        let outcome = captured_input_forwarding_outcome(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::MouseMove { x: 1919, y: 540 },
            true,
        );

        assert!(outcome.messages.is_empty());
        assert_eq!(outcome.target, None);
        assert_eq!(state.session.active_target(), None);
        assert_eq!(forwarder.target(), None);
        assert_eq!(routing.remote_target(), None);
    }

    #[test]
    fn captured_input_forwards_by_default_when_layout_link_is_connected() {
        use rshare_core::{Direction, LayoutLink};

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
        state.layout.upsert_link_for_edge(LayoutLink::new(
            local_id,
            Direction::Right,
            remote_id,
            Direction::Left,
        ));
        state.devices.insert(
            remote_id,
            TrackedDevice {
                id: remote_id,
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );

        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);
        let outcome = captured_input_forwarding_outcome(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::MouseMove { x: 1919, y: 540 },
            true,
        );

        assert_eq!(outcome.target, Some(remote_id));
        assert_eq!(state.session.active_target(), Some(remote_id));
        assert_eq!(forwarder.target(), Some(remote_id));
    }

    #[test]
    fn right_edge_activates_remote_forwarding() {
        use rshare_core::{Direction, LayoutLink};
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
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        // Add layout link for routing
        state
            .layout
            .add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
            true,
        );

        assert_eq!(routing.remote_target(), Some(remote_id));
        assert_eq!(forwarder.target(), Some(remote_id));
        assert!(!messages.is_empty());
        assert!(matches!(
            state.status_snapshot().session_state,
            Some(rshare_core::ControlSessionState::RemoteActive {
                target,
                entered_via: Direction::Right
            }) if target == remote_id
        ));
    }

    #[test]
    fn left_edge_releases_remote_forwarding() {
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
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let _ = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
            true,
        );
        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(0, 500),
            true,
        );

        assert!(messages.is_empty());
        assert_eq!(routing.remote_target(), None);
        assert_eq!(forwarder.target(), None);
    }

    #[test]
    fn left_edge_layout_can_activate_remote_forwarding() {
        use rshare_core::{Direction, LayoutLink};

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
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        state
            .layout
            .add_node(LayoutNode::new(remote_id, -1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Left,
            to_device: remote_id,
            to_edge: Direction::Right,
        });

        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(0, 500),
            true,
        );

        assert_eq!(routing.remote_target(), Some(remote_id));
        assert_eq!(forwarder.target(), Some(remote_id));
        assert!(!messages.is_empty());
    }

    #[test]
    fn input_event_forwarding_targets_first_connected_device() {
        use rshare_core::{Direction, LayoutLink};
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
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        // Add layout link for routing
        state
            .layout
            .add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let _ = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
            true,
        );
        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key(
                rshare_input::KeyCode::Raw(0x20),
                rshare_input::ButtonState::Pressed,
            ),
            true,
        );

        assert_eq!(forwarder.target(), Some(remote_id));
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            messages[0],
            rshare_core::Message::Key {
                keycode: 0x20,
                state: rshare_core::KeyState::Pressed
            }
        ));
    }

    #[test]
    fn key_extended_forwarding_preserves_combo_modifiers() {
        use rshare_core::{Direction, LayoutLink, Message};
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
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        state
            .layout
            .add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let _ = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
            true,
        );
        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key_extended(
                rshare_input::KeyCode::Raw(0x41),
                rshare_input::ButtonState::Pressed,
                true,
                true,
                false,
                false,
            ),
            true,
        );

        assert_eq!(messages.len(), 1);
        assert!(matches!(
            messages[0],
            Message::KeyExtended {
                keycode: 0x41,
                state: rshare_core::KeyState::Pressed,
                shift: true,
                ctrl: true,
                alt: false,
                meta: false
            }
        ));
    }

    #[test]
    fn right_edge_activation_enters_remote_from_opposite_edge() {
        use rshare_core::{Direction, LayoutLink, Message};
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
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        state
            .layout
            .add_node(LayoutNode::new(remote_id, 1920, 0, 2560, 1440));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
            true,
        );

        assert!(matches!(
            messages.first(),
            Some(Message::MouseMove { x: 10, y: 500 })
        ));
    }

    #[test]
    fn quick_return_hotkey_exits_remote_without_forwarding_shortcut() {
        use rshare_core::{Direction, LayoutLink, Message};
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
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        state
            .layout
            .add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let _ = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
            true,
        );
        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key_extended(
                rshare_input::KeyCode::Raw(0x4C),
                rshare_input::ButtonState::Pressed,
                false,
                true,
                true,
                false,
            ),
            true,
        );

        assert_eq!(messages.len(), 2);
        assert!(messages.iter().any(|message| {
            matches!(
                message,
                Message::Key {
                    keycode: 0x11,
                    state: rshare_core::KeyState::Released
                }
            )
        }));
        assert!(messages.iter().any(|message| {
            matches!(
                message,
                Message::Key {
                    keycode: 0x12,
                    state: rshare_core::KeyState::Released
                }
            )
        }));
        let return_edge = routing.take_pending_return_edge().unwrap();
        let _ = state.session.on_return_edge_hit(return_edge);
        routing.clear_remote_target();
        forwarder.clear_target();

        assert_eq!(routing.remote_target(), None);
        assert_eq!(forwarder.target(), None);
        assert!(matches!(
            state.status_snapshot().session_state,
            Some(rshare_core::ControlSessionState::LocalReady)
        ));
    }

    #[test]
    fn ordinary_key_with_tracked_modifiers_forwards_as_combo() {
        use rshare_core::{Direction, LayoutLink, Message};
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
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        state
            .layout
            .add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let _ = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key(
                rshare_input::KeyCode::ControlLeft,
                rshare_input::ButtonState::Pressed,
            ),
            true,
        );
        let _ = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
            true,
        );
        let messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::key(
                rshare_input::KeyCode::Raw(0x43),
                rshare_input::ButtonState::Pressed,
            ),
            true,
        );

        assert!(matches!(
            messages.first(),
            Some(Message::KeyExtended {
                keycode: 0x43,
                state: rshare_core::KeyState::Pressed,
                ctrl: true,
                ..
            })
        ));
    }

    #[test]
    fn gamepad_forwarding_respects_config_after_remote_activation() {
        use rshare_core::{Direction, LayoutLink, Message};
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
                name: "remote".to_string(),
                hostname: "remote-host".to_string(),
                addresses: vec!["127.0.0.1:27431".to_string()],
                connected: true,
                capabilities: DeviceCapabilities::default(),
                last_seen_at: Instant::now(),
            },
        );
        state
            .layout
            .add_node(LayoutNode::new(remote_id, 1920, 0, 1920, 1080));
        state.layout.add_link(LayoutLink {
            from_device: local_id,
            from_edge: Direction::Right,
            to_device: remote_id,
            to_edge: Direction::Left,
        });
        let mut forwarder = rshare_core::engine::ForwardingEngine::new();
        let mut routing = InputRoutingState::for_test(1920, 1080, 10);

        let _ = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::mouse_move(1919, 500),
            false,
        );
        let disabled_messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::gamepad_state(rshare_core::GamepadState::neutral(0, 1, 123)),
            false,
        );
        assert!(disabled_messages.is_empty());

        let enabled_messages = messages_for_input_event(
            &mut state,
            &mut routing,
            &mut forwarder,
            rshare_input::InputEvent::gamepad_state(rshare_core::GamepadState::neutral(0, 2, 456)),
            true,
        );
        assert!(enabled_messages
            .iter()
            .any(|message| matches!(message, Message::GamepadState { .. })));
    }

    #[test]
    fn remote_input_message_maps_to_injectable_input_event() {
        let event = message_to_input_event(rshare_core::Message::MouseButton {
            button: rshare_core::MouseButton::Forward,
            state: rshare_core::ButtonState::Released,
        })
        .unwrap();

        match event {
            rshare_input::InputEvent::MouseButton { button, state } => {
                assert_eq!(button, rshare_input::MouseButton::Forward);
                assert_eq!(state, rshare_input::ButtonState::Released);
            }
            _ => panic!("Wrong input event"),
        }
    }

    #[test]
    fn remote_gamepad_message_maps_to_input_event() {
        let event = message_to_input_event(rshare_core::Message::GamepadState {
            state: rshare_core::GamepadState::neutral(0, 9, 456),
        })
        .unwrap();

        match event {
            rshare_input::InputEvent::GamepadState { state } => {
                assert_eq!(state.gamepad_id, 0);
                assert_eq!(state.sequence, 9);
                assert_eq!(state.timestamp_ms, 456);
            }
            _ => panic!("Wrong input event"),
        }
    }

    #[test]
    fn non_input_message_is_not_injected() {
        let event = message_to_input_event(rshare_core::Message::Heartbeat {
            sequence: 1,
            timestamp: 2,
        });

        assert!(event.is_none());
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
}
