//! R-ShareMouse daemon service.
//!
//! Background service that handles input sharing and local IPC for status queries.

use anyhow::Result;
use futures_util::SinkExt;
use rshare_core::{
    default_ipc_addr, default_local_controls_ws_addr, read_json_line, write_json_line,
    BackendFailureReason, BackendHealth, BackendKind, BackendRuntimeState,
    CaptureSessionStateMachine, Config, ControlSessionState, DaemonDeviceSnapshot, DaemonRequest,
    DaemonResponse, DeviceId, Direction, LayoutGraph, LayoutNode, LocalControlDeviceSnapshot,
    LocalDisplayInfo, LocalDisplayState, LocalGamepadState, LocalInputDeviceKind,
    LocalInputDiagnosticEvent, LocalInputEventSource, LocalInputTestKind, LocalInputTestRequest,
    LocalInputTestResult, LocalInputTestStatus, Message, ResolvedInputMode, ScreenInfo,
    ServiceStatusSnapshot,
};
use rshare_input::{
    BackendCandidate, BackendSelector, CaptureBackend, GamepadListenerConfig, GilrsGamepadListener,
    InjectBackend, InputEvent, PortableCaptureBackend, PortableInjectBackend, RDevInputListener,
};

#[cfg(target_os = "linux")]
use rshare_input::EvdevCaptureBackend;
use rshare_net::{DiscoveredDevice, NetworkEvent, NetworkManager, NetworkManagerConfig};
use tracing_subscriber::prelude::*;

#[cfg(windows)]
use rshare_platform::firewall;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::time::{Duration, Instant};
use tokio_tungstenite::{accept_async, tungstenite::Message as WsMessage};

#[derive(Clone)]
struct TrackedDevice {
    id: DeviceId,
    name: String,
    hostname: String,
    addresses: Vec<String>,
    connected: bool,
    last_seen_at: Instant,
}

struct DaemonState {
    status: ServiceStatusSnapshot,
    devices: HashMap<DeviceId, TrackedDevice>,
    // Layout and routing state
    layout: LayoutGraph,
    session: CaptureSessionStateMachine,
    // Backend state with separate capture/inject health
    backend_state: BackendRuntimeState,
    local_controls: LocalControlDeviceSnapshot,
    pending_keyboard_loopback_until_ms: u64,
    pending_keyboard_loopback_events: u8,
    pending_mouse_loopback_until_ms: u64,
    pending_mouse_loopback_events: u8,
}

impl DaemonState {
    fn new(status: ServiceStatusSnapshot) -> Self {
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
            default_local_control_snapshot(local_screen.width, local_screen.height);

        Self {
            status,
            devices: HashMap::new(),
            layout,
            session: CaptureSessionStateMachine::new(),
            backend_state,
            local_controls,
            pending_keyboard_loopback_until_ms: 0,
            pending_keyboard_loopback_events: 0,
            pending_mouse_loopback_until_ms: 0,
            pending_mouse_loopback_events: 0,
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
                last_seen_at: Instant::now(),
            },
        );
        self.layout
            .merge_discovered_peers_to_right_with_screens([(device.id, screen_info)]);
    }

    fn remove_device(&mut self, id: &DeviceId) {
        self.devices.remove(id);
    }

    fn mark_connected(&mut self, id: &DeviceId, connected: bool) {
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
                    last_seen_at: Instant::now(),
                },
            );
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

    fn reconcile_local_layout_geometry(&mut self) -> bool {
        let local_screen = current_primary_screen_info();
        self.layout.update_primary_display_geometry(
            self.status.device_id,
            local_screen.width,
            local_screen.height,
        )
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

    fn record_local_input_event(&mut self, event: &InputEvent) -> LocalInputDiagnosticEvent {
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

        let event = LocalInputDiagnosticEvent {
            sequence,
            timestamp_ms,
            device_kind,
            event_kind,
            summary,
            device_id: None,
            device_instance_id: None,
            capture_path: None,
            source,
            payload,
        };
        push_recent_local_event(&mut self.local_controls, event.clone());
        event
    }
}

const LOCAL_CONTROL_RECENT_EVENT_LIMIT: usize = 64;
const INJECTION_LOOPBACK_WINDOW_MS: u64 = 750;

fn default_local_control_snapshot(width: u32, height: u32) -> LocalControlDeviceSnapshot {
    let mut snapshot = LocalControlDeviceSnapshot::default();
    snapshot.display = fallback_display_state(width, height);
    #[cfg(windows)]
    {
        let screens = rshare_platform::windows::get_all_screens();
        if !screens.is_empty() {
            snapshot.display = display_state_from_windows_screens(&screens);
        }
        snapshot.driver = rshare_platform::windows::probe_rshare_driver();
        if snapshot.driver.status == "available" {
            snapshot.keyboard.capture_source = "RShare filter driver + fallback hook".to_string();
            snapshot.mouse.capture_source = "RShare filter driver + fallback hook".to_string();
        }
    }
    #[cfg(windows)]
    match rshare_platform::windows::enumerate_raw_input_devices() {
        Ok((keyboards, mice)) => {
            snapshot.keyboard_devices = keyboards;
            snapshot.mouse_devices = mice;
            if !snapshot.keyboard_devices.is_empty() && snapshot.driver.status != "available" {
                snapshot.keyboard.capture_source = "Windows Raw Input + low-level hook".to_string();
            }
            if !snapshot.mouse_devices.is_empty() && snapshot.driver.status != "available" {
                snapshot.mouse.capture_source = "Windows Raw Input + low-level hook".to_string();
            }
        }
        Err(error) => {
            snapshot.last_error = Some(format!("Raw Input enumeration failed: {error}"));
        }
    }
    snapshot
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

    LocalDisplayState {
        display_count: screens.len(),
        virtual_x: min_x,
        virtual_y: min_y,
        primary_width: primary.width,
        primary_height: primary.height,
        layout_width: max_x.saturating_sub(min_x).max(0) as u32,
        layout_height: max_y.saturating_sub(min_y).max(0) as u32,
        displays: screens
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
            })
            .collect(),
    }
}

fn push_recent_local_event(
    snapshot: &mut LocalControlDeviceSnapshot,
    event: LocalInputDiagnosticEvent,
) {
    snapshot.recent_events.push(event);
    if snapshot.recent_events.len() > LOCAL_CONTROL_RECENT_EVENT_LIMIT {
        let overflow = snapshot.recent_events.len() - LOCAL_CONTROL_RECENT_EVENT_LIMIT;
        snapshot.recent_events.drain(0..overflow);
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

fn timestamp_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn short_device_id(id: DeviceId) -> String {
    id.to_string().chars().take(8).collect()
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
            let health = if result.degraded {
                BackendHealth::Degraded {
                    reason: BackendFailureReason::Unavailable,
                }
            } else {
                BackendHealth::Healthy
            };
            (
                Some(mode),
                available_kinds,
                health,
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

fn is_device_connected(state: &DaemonState, id: DeviceId) -> bool {
    state
        .devices
        .get(&id)
        .map(|device| device.connected)
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
struct InputRoutingState {
    remote_target: Option<DeviceId>,
    screen_width: u32,
    screen_height: u32,
    edge_threshold: u32,
}

impl InputRoutingState {
    fn new(screen_width: u32, screen_height: u32, edge_threshold: u32) -> Self {
        Self {
            remote_target: None,
            screen_width: screen_width.max(1),
            screen_height: screen_height.max(1),
            edge_threshold: edge_threshold.max(1),
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
        rshare_input::InputEvent::KeyExtended { keycode, state, .. } => {
            Some(rshare_core::engine::RawInputEvent::Key {
                keycode: keycode.to_raw(),
                pressed: state.is_pressed(),
            })
        }
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

    // Get connected peers set
    let connected_peers: std::collections::HashSet<_> = state
        .devices
        .values()
        .filter(|device| device.connected)
        .map(|device| device.id)
        .collect();
    let local_id = state.status.device_id;
    let edge_hits = routing.hit_edges(&event);

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
    let Some(raw_event) = input_event_to_raw_event(event) else {
        return Vec::new();
    };

    let mut messages = forwarder.process_event(raw_event);
    if activated_on_this_event && messages.is_empty() {
        messages = forwarder.flush_batch();
    }
    messages
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
            rshare_input::KeyCode::Raw(keycode),
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
            rshare_input::KeyCode::Raw(keycode),
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
    from: DeviceId,
    message: Message,
) {
    let Some(event) = message_to_input_event(message) else {
        return;
    };

    let result = {
        let mut backend = inject_backend.lock().await;
        backend.inject(event)
    };

    if let Err(error) = result {
        tracing::warn!("Failed to inject input from {}: {}", from, error);
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

async fn run_local_input_test(
    inject_backend: &Arc<Mutex<Box<dyn InjectBackend>>>,
    state: &Arc<RwLock<DaemonState>>,
    local_events_tx: &broadcast::Sender<LocalInputDiagnosticEvent>,
    test: LocalInputTestRequest,
) -> LocalInputTestResult {
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
            let is_virtual_hid = {
                #[cfg(target_os = "windows")]
                {
                    backend.kind() == BackendKind::VirtualHid
                }
                #[cfg(not(target_os = "windows"))]
                {
                    false
                }
            };
            let ((first_x, first_y), (second_x, second_y)) = if is_virtual_hid {
                ((8, 8), (-8, -8))
            } else {
                ((x.saturating_add(8), y.saturating_add(8)), (x, y))
            };
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
            let event = record_injected_test_event(state, test.kind).await;
            let _ = local_events_tx.send(event);
            LocalInputTestResult::success("Local input injection test completed.")
        }
        Err(error) => {
            LocalInputTestResult::failed(input_test_failure_status(&error), error.to_string())
        }
    }
}

async fn record_injected_test_event(
    state: &Arc<RwLock<DaemonState>>,
    kind: LocalInputTestKind,
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
        payload: BTreeMap::new(),
    };
    push_recent_local_event(&mut state.local_controls, event.clone());
    event
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
    mut input_rx: tokio::sync::mpsc::UnboundedReceiver<InputEvent>,
    state: Arc<RwLock<DaemonState>>,
    network_manager: Arc<Mutex<NetworkManager>>,
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
    edge_threshold: u32,
    gamepad_forwarding_enabled: bool,
) -> Result<()> {
    let mut forwarder = rshare_core::engine::ForwardingEngine::new();
    let mut routing = InputRoutingState::default_with_threshold(edge_threshold);
    let mut flush_interval = tokio::time::interval(Duration::from_millis(8));

    loop {
        tokio::select! {
            event = input_rx.recv() => {
                let Some(event) = event else {
                    break;
                };

                let (target, messages) = {
                    let mut state = state.write().await;
                    let local_event = state.record_local_input_event(&event);
                    let _ = local_events_tx.send(local_event);
                    let messages = messages_for_input_event(
                        &mut state,
                        &mut routing,
                        &mut forwarder,
                        event,
                        gamepad_forwarding_enabled,
                    );
                    let target = state.session.active_target();
                    (target, messages)
                };

                if let Some(target) = target {
                    send_forwarded_messages(&network_manager, target, messages).await;
                }
            }
            _ = flush_interval.tick() => {
                if !forwarder.should_flush_batch() {
                    continue;
                }

                let target = {
                    let state = state.read().await;
                    state
                        .session
                        .active_target()
                        .filter(|target| is_device_connected(&state, *target))
                };

                let Some(target) = target else {
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

    Ok(())
}

#[cfg(windows)]
fn input_event_from_windows_driver_event(
    event: &rshare_platform::windows::WindowsDriverInputEvent,
) -> Option<InputEvent> {
    use rshare_platform::windows::{WindowsDriverDeviceKind, WindowsDriverEventKind};

    match (event.device_kind, event.event_kind) {
        (WindowsDriverDeviceKind::Keyboard, WindowsDriverEventKind::Key)
        | (WindowsDriverDeviceKind::Keyboard, WindowsDriverEventKind::Synthetic) => {
            Some(InputEvent::key(
                rshare_input::KeyCode::Raw(event.value0 as u32),
                if event.value1 != 0 {
                    rshare_input::ButtonState::Pressed
                } else {
                    rshare_input::ButtonState::Released
                },
            ))
        }
        (WindowsDriverDeviceKind::Mouse, WindowsDriverEventKind::MouseMove)
        | (WindowsDriverDeviceKind::Mouse, WindowsDriverEventKind::Synthetic) => {
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
                let Some(input_event) = input_event_from_windows_driver_event(&driver_event) else {
                    continue;
                };

                let (target, messages) = {
                    let mut state = state.write().await;
                    let mut local_event = state.record_local_input_event(&input_event);
                    local_event.device_id = Some(driver_event.device_id.clone());
                    local_event.device_instance_id = Some(driver_event.device_instance_id.clone());
                    local_event.capture_path = Some("rshare-filter".to_string());
                    local_event.source = local_source_from_windows_driver_event(driver_event.source);
                    local_event.payload.insert("driver_flags".to_string(), driver_event.flags.to_string());
                    update_driver_device_from_event(&mut state.local_controls, &driver_event, local_event.timestamp_ms);
                    replace_recent_local_event(&mut state.local_controls, local_event.clone());
                    let _ = local_events_tx.send(local_event);
                    let messages = messages_for_input_event(
                        &mut state,
                        &mut routing,
                        &mut forwarder,
                        input_event,
                        true,
                    );
                    let target = state.session.active_target();
                    (target, messages)
                };

                if let Some(target) = target {
                    send_forwarded_messages(&network_manager, target, messages).await;
                }
            }
            _ = shutdown_rx.recv() => break,
        }
    }

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
    let mut has_keyboard = false;
    let mut has_mouse = false;

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
    tx: tokio::sync::mpsc::UnboundedSender<InputEvent>,
) -> Result<tokio::task::JoinHandle<()>> {
    use rshare_platform::EvdevDriverEvent;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

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

            let input_event = match evdev_event {
                EvdevDriverEvent::MouseMove { x, y } => InputEvent::MouseMove { x, y },
                EvdevDriverEvent::MouseButton { button, pressed } => {
                    use rshare_input::ButtonState;
                    use rshare_input::MouseButton;
                    let button_code = match button {
                        0 => MouseButton::Left,
                        1 => MouseButton::Right,
                        2 => MouseButton::Middle,
                        3 => MouseButton::Back,
                        4 => MouseButton::Forward,
                        _ => MouseButton::Other(button as u8),
                    };
                    let state = if pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    };
                    InputEvent::MouseButton {
                        button: button_code,
                        state,
                    }
                }
                EvdevDriverEvent::MouseWheel { delta_x, delta_y } => {
                    InputEvent::MouseWheel { delta_x, delta_y }
                }
                EvdevDriverEvent::Key { keycode, pressed } => {
                    use rshare_input::{ButtonState, KeyCode};
                    let key = KeyCode::Raw(keycode);
                    let state = if pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    };
                    InputEvent::Key {
                        keycode: key,
                        state,
                    }
                }
            };

            if tx.send(input_event).is_err() {
                tracing::warn!("Failed to send input event through channel");
            }
        };

        // Start the listener
        if let Err(e) = listener.start(callback) {
            tracing::error!("Evdev listener error: {:?}", e);
        }

        // Keep the thread alive until shutdown
        while running.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }

        let _ = listener.stop();
    });

    Ok(handle)
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

    // Configure firewall on Windows to allow discovery and service ports
    #[cfg(windows)]
    {
        match firewall::configure_firewall() {
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

    let config = load_config_with_env_overrides()?;

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

    let mut daemon_state = DaemonState::new(ServiceStatusSnapshot::new(
        device_id,
        device_name.clone(),
        hostname.clone(),
        bind_address.clone(),
        27432,
        pid,
    ));
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

    // Input capture: try Evdev on Linux for kernel-level access, fallback to RDev
    #[cfg(target_os = "linux")]
    let (input_rx, mut input_channel) = {
        use tokio::sync::mpsc;

        let (tx, rx) = mpsc::unbounded_channel::<InputEvent>();

        // Try to use EvdevCaptureBackend for kernel-level input capture
        match try_start_evdev_capture(tx.clone()) {
            Ok(evdev_task) => {
                tracing::info!("Using Evdev backend for input capture (kernel-level)");
                // Evdev capture is running in the background task
                (rx, None)
            }
            Err(e) => {
                tracing::warn!(
                    "Evdev capture unavailable: {:?}, using RDev (Portable) backend",
                    e
                );
                // Fallback to RDev (Portable) backend
                let mut input_listener = RDevInputListener::new();
                let rx = input_listener.receiver();
                let channel = Some(input_listener);
                let _ = tx; // Keep tx in scope
                (rx, channel)
            }
        }
    };

    #[cfg(not(target_os = "linux"))]
    let (input_rx, mut input_channel) = {
        let mut input_listener = RDevInputListener::new();
        let rx = input_listener.receiver();
        (rx, Some(input_listener))
    };

    let mut gamepad_listener_config = GamepadListenerConfig::from(&config.gamepad);
    gamepad_listener_config.enabled = true;
    let mut gamepad_listener = {
        #[cfg(target_os = "linux")]
        {
            // On Linux with Evdev, we need to create a separate channel for gamepad events
            use rshare_input::InputEventChannel;
            let (gamepad_channel, _gamepad_rx) = InputEventChannel::new();
            GilrsGamepadListener::new(gamepad_channel, gamepad_listener_config)
        }
        #[cfg(not(target_os = "linux"))]
        {
            GilrsGamepadListener::new(
                input_channel.as_ref().unwrap().channel(),
                gamepad_listener_config,
            )
        }
    };
    gamepad_listener.start()?;

    // Start RDev listener if we're using it
    #[cfg(not(target_os = "linux"))]
    if let Some(ref mut listener) = input_channel {
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
        local_events_tx.clone(),
        layout_path.clone(),
        shutdown_tx.clone(),
    ));
    let local_controls_ws_task = tokio::spawn(run_local_controls_ws_server(
        state.clone(),
        local_events_tx.clone(),
        shutdown_tx.subscribe(),
    ));

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
    let _windows_driver_capture_task = tokio::spawn(run_windows_driver_capture_loop(
        state.clone(),
        network_manager.clone(),
        local_events_tx.clone(),
        shutdown_tx.subscribe(),
        config.edge_threshold(),
    ));

    let event_task = {
        let state = state.clone();
        let inject_backend = inject_backend.clone();
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
                        let mut state = state.write().await;
                        state.mark_connected(&id, true)
                    }
                    NetworkEvent::DeviceDisconnected(id) => {
                        let mut state = state.write().await;
                        // Notify session state machine of target disconnection
                        state.session.on_target_disconnect(id);
                        state.remove_device(&id);
                    }
                    NetworkEvent::MessageReceived { from, message } => {
                        inject_remote_message(&inject_backend, from, message).await;
                    }
                    NetworkEvent::ConnectionError { device_id, error } => {
                        tracing::warn!("Connection error to {}: {}", device_id, error);
                        let mut state = state.write().await;
                        state.mark_connected(&device_id, false);
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
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
    layout_path: Arc<PathBuf>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let network_manager = network_manager.clone();
        let inject_backend = inject_backend.clone();
        let local_events_tx = local_events_tx.clone();
        let layout_path = layout_path.clone();
        let shutdown_tx = shutdown_tx.clone();

        tokio::spawn(async move {
            if let Err(err) = handle_ipc_client(
                stream,
                state,
                network_manager,
                inject_backend,
                local_events_tx,
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
        let state = state.read().await;
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
    local_events_tx: broadcast::Sender<LocalInputDiagnosticEvent>,
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

    let response = match request {
        DaemonRequest::Status => {
            let state = state.read().await;
            DaemonResponse::Status(state.status_snapshot())
        }
        DaemonRequest::Devices => {
            let state = state.read().await;
            DaemonResponse::Devices(state.device_snapshots())
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
                            state.write().await.mark_connected(&device_id, true);
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
                    state.write().await.mark_connected(&device_id, false);
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
        DaemonRequest::LocalControls => {
            let state = state.read().await;
            DaemonResponse::LocalControls(state.local_control_snapshot())
        }
        DaemonRequest::RunLocalInputTest { test } => {
            let result =
                run_local_input_test(&inject_backend, &state, &local_events_tx, test).await;
            DaemonResponse::LocalInputTest(result)
        }
        DaemonRequest::SubscribeLocalControls => unreachable!("handled before response match"),
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
                },
                LocalDisplayInfo {
                    display_id: "primary".to_string(),
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    primary: true,
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
    async fn injected_test_marks_immediate_capture_feedback_as_loopback() {
        let state = Arc::new(RwLock::new(test_daemon_state()));

        record_injected_test_event(&state, LocalInputTestKind::KeyboardShift).await;
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
    fn healthy_candidates_remain_visible_after_selection() {
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
        assert!(matches!(
            health,
            BackendHealth::Degraded {
                reason: BackendFailureReason::Unavailable
            }
        ));
        assert!(error.unwrap().contains("using Portable"));
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

    #[cfg(windows)]
    #[test]
    fn windows_driver_event_maps_to_local_input_event() {
        let event = rshare_platform::windows::WindowsDriverInputEvent {
            source: rshare_platform::windows::WindowsDriverEventSource::DriverTest,
            device_kind: rshare_platform::windows::WindowsDriverDeviceKind::Keyboard,
            event_kind: rshare_platform::windows::WindowsDriverEventKind::Key,
            device_id: "driver-keyboard".to_string(),
            device_instance_id: "instance".to_string(),
            value0: 0x10,
            value1: 1,
            value2: 0,
            flags: 0,
            timestamp_us: 1,
        };

        let input = input_event_from_windows_driver_event(&event).unwrap();

        assert!(matches!(
            input,
            rshare_input::InputEvent::Key {
                keycode: rshare_input::KeyCode::Raw(0x10),
                state: rshare_input::ButtonState::Pressed
            }
        ));
        assert_eq!(
            local_source_from_windows_driver_event(event.source),
            LocalInputEventSource::DriverTest
        );
    }

    #[test]
    fn mark_connected_tracks_unknown_inbound_device() {
        let remote_id = DeviceId::new_v4();
        let mut state = DaemonState::new(ServiceStatusSnapshot::new(
            DeviceId::new_v4(),
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
