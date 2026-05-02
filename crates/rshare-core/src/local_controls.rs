//! Local control-device diagnostics exposed to desktop clients.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    BackendHealth, BackendKind, DeviceId, GamepadButtonState, GamepadState, PrivilegeState,
    ResolvedInputMode, UsbDeviceDescriptor,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalControlDeviceSnapshot {
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub keyboard: LocalKeyboardState,
    #[serde(default)]
    pub mouse: LocalMouseState,
    #[serde(default)]
    pub keyboard_devices: Vec<LocalHardwareDevice>,
    #[serde(default)]
    pub mouse_devices: Vec<LocalHardwareDevice>,
    #[serde(default)]
    pub gamepads: Vec<LocalGamepadState>,
    #[serde(default)]
    pub audio_inputs: Vec<LocalAudioInputDevice>,
    #[serde(default)]
    pub audio_outputs: Vec<LocalAudioOutputDevice>,
    #[serde(default)]
    pub audio_capture_state: LocalAudioCaptureState,
    #[serde(default)]
    pub audio_stream_state: LocalAudioStreamState,
    #[serde(default)]
    pub usb_devices: Vec<UsbDeviceDescriptor>,
    #[serde(default)]
    pub remote_usb_devices: Vec<RemoteUsbDeviceSnapshot>,
    #[serde(default)]
    pub display: LocalDisplayState,
    #[serde(default)]
    pub capture_backend: LocalBackendDiagnosticState,
    #[serde(default)]
    pub inject_backend: LocalBackendDiagnosticState,
    #[serde(default)]
    pub privilege_state: Option<PrivilegeState>,
    #[serde(default)]
    pub virtual_gamepad: LocalVirtualGamepadState,
    #[serde(default)]
    pub driver: LocalDriverDiagnosticState,
    #[serde(default)]
    pub recent_events: Vec<LocalInputDiagnosticEvent>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for LocalControlDeviceSnapshot {
    fn default() -> Self {
        Self {
            sequence: 0,
            keyboard: LocalKeyboardState::default(),
            mouse: LocalMouseState::default(),
            keyboard_devices: Vec::new(),
            mouse_devices: Vec::new(),
            gamepads: Vec::new(),
            audio_inputs: Vec::new(),
            audio_outputs: Vec::new(),
            audio_capture_state: LocalAudioCaptureState::default(),
            audio_stream_state: LocalAudioStreamState::default(),
            usb_devices: Vec::new(),
            remote_usb_devices: Vec::new(),
            display: LocalDisplayState::default(),
            capture_backend: LocalBackendDiagnosticState::default(),
            inject_backend: LocalBackendDiagnosticState::default(),
            privilege_state: None,
            virtual_gamepad: LocalVirtualGamepadState::default(),
            driver: LocalDriverDiagnosticState::default(),
            recent_events: Vec::new(),
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteUsbDeviceSnapshot {
    pub device_id: DeviceId,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub connected: bool,
    pub device: UsbDeviceDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalHardwareDevice {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub driver_detail: Option<String>,
    #[serde(default)]
    pub device_instance_id: Option<String>,
    #[serde(default)]
    pub capture_path: Option<String>,
    #[serde(default)]
    pub event_count: u64,
    #[serde(default)]
    pub last_event_ms: u64,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl Default for LocalHardwareDevice {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            source: String::new(),
            connected: false,
            driver_detail: None,
            device_instance_id: None,
            capture_path: None,
            event_count: 0,
            last_event_ms: 0,
            capabilities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalAudioInputKind {
    Microphone,
    Loopback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAudioInputDevice {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default = "default_audio_input_kind")]
    pub kind: LocalAudioInputKind,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub muted: Option<bool>,
    #[serde(default)]
    pub level_peak: u8,
    #[serde(default)]
    pub level_rms: u8,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub channel_count: Option<u32>,
    #[serde(default)]
    pub driver_detail: Option<String>,
}

impl Default for LocalAudioInputDevice {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            endpoint_id: None,
            kind: LocalAudioInputKind::Microphone,
            source: String::new(),
            connected: false,
            default: false,
            muted: None,
            level_peak: 0,
            level_rms: 0,
            sample_rate: None,
            channel_count: None,
            driver_detail: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAudioOutputDevice {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub muted: Option<bool>,
    #[serde(default)]
    pub volume_percent: Option<u8>,
    #[serde(default)]
    pub channel_count: Option<u32>,
    #[serde(default)]
    pub driver_detail: Option<String>,
}

impl Default for LocalAudioOutputDevice {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            endpoint_id: None,
            source: String::new(),
            connected: false,
            default: false,
            muted: None,
            volume_percent: None,
            channel_count: None,
            driver_detail: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalAudioCaptureSource {
    Microphone,
    Loopback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalAudioCaptureStatus {
    Idle,
    CapturingLocal,
    ForwardingRemote,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAudioCaptureState {
    #[serde(default = "default_audio_capture_status")]
    pub status: LocalAudioCaptureStatus,
    #[serde(default)]
    pub source: Option<LocalAudioCaptureSource>,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub level_peak: u8,
    #[serde(default)]
    pub level_rms: u8,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub channel_count: Option<u32>,
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for LocalAudioCaptureState {
    fn default() -> Self {
        Self {
            status: LocalAudioCaptureStatus::Idle,
            source: None,
            endpoint_id: None,
            level_peak: 0,
            level_rms: 0,
            sample_rate: Some(48_000),
            channel_count: Some(2),
            started_at_ms: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAudioStreamState {
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub target_device_id: Option<String>,
    #[serde(default)]
    pub stream_id: Option<String>,
    #[serde(default)]
    pub frames_sent: u64,
    #[serde(default)]
    pub frames_received: u64,
    #[serde(default)]
    pub underruns: u64,
    #[serde(default)]
    pub overruns: u64,
    #[serde(default)]
    pub latency_ms: Option<u32>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for LocalAudioStreamState {
    fn default() -> Self {
        Self {
            active: false,
            target_device_id: None,
            stream_id: None,
            frames_sent: 0,
            frames_received: 0,
            underruns: 0,
            overruns: 0,
            latency_ms: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalDriverDiagnosticState {
    #[serde(default = "default_driver_status")]
    pub status: String,
    #[serde(default)]
    pub device_path: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub filter_active: bool,
    #[serde(default)]
    pub vhid_active: bool,
    #[serde(default)]
    pub test_signing_required: bool,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for LocalDriverDiagnosticState {
    fn default() -> Self {
        Self {
            status: default_driver_status(),
            device_path: None,
            version: None,
            filter_active: false,
            vhid_active: false,
            test_signing_required: false,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalKeyboardState {
    #[serde(default)]
    pub detected: bool,
    #[serde(default)]
    pub pressed_keys: Vec<String>,
    #[serde(default)]
    pub last_key: Option<String>,
    #[serde(default)]
    pub event_count: u64,
    #[serde(default = "default_capture_source")]
    pub capture_source: String,
}

impl Default for LocalKeyboardState {
    fn default() -> Self {
        Self {
            detected: false,
            pressed_keys: Vec::new(),
            last_key: None,
            event_count: 0,
            capture_source: default_capture_source(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalMouseState {
    #[serde(default)]
    pub detected: bool,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub pressed_buttons: Vec<String>,
    #[serde(default)]
    pub wheel_delta_x: i32,
    #[serde(default)]
    pub wheel_delta_y: i32,
    #[serde(default)]
    pub event_count: u64,
    #[serde(default)]
    pub move_count: u64,
    #[serde(default)]
    pub button_event_count: u64,
    #[serde(default)]
    pub button_press_count: u64,
    #[serde(default)]
    pub button_release_count: u64,
    #[serde(default)]
    pub wheel_event_count: u64,
    #[serde(default)]
    pub wheel_total_x: i64,
    #[serde(default)]
    pub wheel_total_y: i64,
    #[serde(default)]
    pub current_display_index: Option<usize>,
    #[serde(default)]
    pub current_display_id: Option<String>,
    #[serde(default)]
    pub display_relative_x: i32,
    #[serde(default)]
    pub display_relative_y: i32,
    #[serde(default = "default_capture_source")]
    pub capture_source: String,
}

impl Default for LocalMouseState {
    fn default() -> Self {
        Self {
            detected: false,
            x: 0,
            y: 0,
            pressed_buttons: Vec::new(),
            wheel_delta_x: 0,
            wheel_delta_y: 0,
            event_count: 0,
            move_count: 0,
            button_event_count: 0,
            button_press_count: 0,
            button_release_count: 0,
            wheel_event_count: 0,
            wheel_total_x: 0,
            wheel_total_y: 0,
            current_display_index: None,
            current_display_id: None,
            display_relative_x: 0,
            display_relative_y: 0,
            capture_source: default_capture_source(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalGamepadState {
    pub gamepad_id: u8,
    pub name: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub buttons: Vec<GamepadButtonState>,
    #[serde(default)]
    pub pressed_buttons: Vec<String>,
    #[serde(default)]
    pub last_button: Option<String>,
    #[serde(default)]
    pub left_stick_x: i16,
    #[serde(default)]
    pub left_stick_y: i16,
    #[serde(default)]
    pub right_stick_x: i16,
    #[serde(default)]
    pub right_stick_y: i16,
    #[serde(default)]
    pub left_trigger: u16,
    #[serde(default)]
    pub right_trigger: u16,
    #[serde(default)]
    pub event_count: u64,
    #[serde(default)]
    pub button_event_count: u64,
    #[serde(default)]
    pub button_press_count: u64,
    #[serde(default)]
    pub button_release_count: u64,
    #[serde(default)]
    pub axis_event_count: u64,
    #[serde(default)]
    pub trigger_event_count: u64,
    #[serde(default)]
    pub last_axis: Option<String>,
    #[serde(default)]
    pub last_seen_ms: u64,
}

impl LocalGamepadState {
    pub fn from_state(state: &GamepadState, name: Option<String>, connected: bool) -> Self {
        Self {
            gamepad_id: state.gamepad_id,
            name: name.unwrap_or_else(|| format!("Gamepad {}", state.gamepad_id)),
            connected,
            buttons: state.buttons.clone(),
            pressed_buttons: pressed_gamepad_buttons(&state.buttons),
            last_button: None,
            left_stick_x: state.left_stick_x,
            left_stick_y: state.left_stick_y,
            right_stick_x: state.right_stick_x,
            right_stick_y: state.right_stick_y,
            left_trigger: state.left_trigger,
            right_trigger: state.right_trigger,
            event_count: 1,
            button_event_count: 0,
            button_press_count: 0,
            button_release_count: 0,
            axis_event_count: 0,
            trigger_event_count: 0,
            last_axis: None,
            last_seen_ms: state.timestamp_ms,
        }
    }
}

fn pressed_gamepad_buttons(buttons: &[GamepadButtonState]) -> Vec<String> {
    buttons
        .iter()
        .filter(|button| button.pressed)
        .map(|button| format!("{:?}", button.button))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalDisplayState {
    #[serde(default)]
    pub display_count: usize,
    #[serde(default)]
    pub virtual_x: i32,
    #[serde(default)]
    pub virtual_y: i32,
    #[serde(default)]
    pub primary_width: u32,
    #[serde(default)]
    pub primary_height: u32,
    #[serde(default)]
    pub layout_width: u32,
    #[serde(default)]
    pub layout_height: u32,
    #[serde(default)]
    pub displays: Vec<LocalDisplayInfo>,
}

impl Default for LocalDisplayState {
    fn default() -> Self {
        Self {
            display_count: 0,
            virtual_x: 0,
            virtual_y: 0,
            primary_width: 0,
            primary_height: 0,
            layout_width: 0,
            layout_height: 0,
            displays: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalDisplayInfo {
    pub display_id: String,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalBackendDiagnosticState {
    #[serde(default)]
    pub mode: Option<ResolvedInputMode>,
    #[serde(default)]
    pub kind: Option<BackendKind>,
    #[serde(default)]
    pub health: Option<BackendHealth>,
    #[serde(default)]
    pub active: bool,
}

impl Default for LocalBackendDiagnosticState {
    fn default() -> Self {
        Self {
            mode: None,
            kind: None,
            health: None,
            active: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalVirtualGamepadState {
    #[serde(default = "default_virtual_gamepad_status")]
    pub status: String,
    #[serde(default = "default_virtual_gamepad_detail")]
    pub detail: String,
}

impl Default for LocalVirtualGamepadState {
    fn default() -> Self {
        Self {
            status: default_virtual_gamepad_status(),
            detail: default_virtual_gamepad_detail(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalInputDiagnosticEvent {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub device_kind: LocalInputDeviceKind,
    pub event_kind: String,
    pub summary: String,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub device_instance_id: Option<String>,
    #[serde(default)]
    pub capture_path: Option<String>,
    #[serde(default)]
    pub source: LocalInputEventSource,
    #[serde(default)]
    pub payload: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalInputDeviceKind {
    Keyboard,
    Mouse,
    Gamepad,
    Usb,
    Display,
    Audio,
    Backend,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalInputEventSource {
    #[default]
    Hardware,
    Injected,
    InjectedLoopback,
    DriverTest,
    VirtualDevice,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalInputTestRequest {
    pub kind: LocalInputTestKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalInputTestKind {
    KeyboardShift,
    MouseMove,
    VirtualGamepadStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalInputTestResult {
    pub status: LocalInputTestStatus,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAudioTestRequest {
    #[serde(default = "default_audio_capture_source")]
    pub source: LocalAudioCaptureSource,
    #[serde(default)]
    pub endpoint_id: Option<String>,
}

impl Default for LocalAudioTestRequest {
    fn default() -> Self {
        Self {
            source: LocalAudioCaptureSource::Loopback,
            endpoint_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAudioTestResult {
    pub status: LocalAudioTestStatus,
    pub message: String,
}

impl LocalAudioTestResult {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            status: LocalAudioTestStatus::Success,
            message: message.into(),
        }
    }

    pub fn failed(status: LocalAudioTestStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalAudioTestStatus {
    Success,
    DeviceUnavailable,
    PermissionDenied,
    BackendUnavailable,
    Unsupported,
    Failed,
}

impl LocalInputTestResult {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            status: LocalInputTestStatus::Success,
            message: message.into(),
        }
    }

    pub fn failed(status: LocalInputTestStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalInputTestStatus {
    Success,
    PermissionDenied,
    BackendUnavailable,
    Failed,
    Unsupported,
}

fn default_capture_source() -> String {
    "daemon".to_string()
}

fn default_audio_input_kind() -> LocalAudioInputKind {
    LocalAudioInputKind::Microphone
}

fn default_audio_capture_source() -> LocalAudioCaptureSource {
    LocalAudioCaptureSource::Loopback
}

fn default_audio_capture_status() -> LocalAudioCaptureStatus {
    LocalAudioCaptureStatus::Idle
}

fn default_driver_status() -> String {
    "unavailable".to_string()
}

fn default_virtual_gamepad_status() -> String {
    "not_implemented".to_string()
}

fn default_virtual_gamepad_detail() -> String {
    "Virtual HID gamepad injection is not implemented in this build.".to_string()
}
