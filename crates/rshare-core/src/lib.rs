//! R-ShareMouse core library
//!
//! This crate contains the core business logic for the R-ShareMouse application,
//! including protocol definitions, device management, configuration, and clipboard handling.

pub mod capabilities;
pub mod clipboard;
pub mod config;
pub mod daemon_client;
pub mod device;
pub mod endpoint_events;
pub mod engine;
pub mod hardware_assets;
pub mod input;
pub mod input_mode;
pub mod input_router;
pub mod ipc;
pub mod layout;
pub mod local_controls;
pub mod perf;
pub mod protocol;
pub mod runtime;
pub mod service;
pub mod session;

// Re-exports from protocol
pub use protocol::{
    heartbeat_message, hello_back_message, hello_message, timestamp_ms, AudioFormat,
    AudioFramePayload, AudioSampleFormat, ButtonState, ControlConnectionId, DeviceCapabilities,
    DeviceId, Direction, GamepadButton, GamepadButtonState, GamepadDeviceInfo, GamepadState,
    HandshakeRejectReason, KeyState, Message, MouseButton, PeerTransportCapabilities, Priority,
    ScreenInfo, UsbConfigurationDescriptor, UsbControlSetupPacket, UsbDeviceClaimRequest,
    UsbDeviceClaimResponse, UsbDeviceDescriptor, UsbDeviceResetKind, UsbDeviceSpeed,
    UsbEndpointDescriptor, UsbFlowControl, UsbForwardingCapabilities, UsbInterfaceDescriptor,
    UsbIsoPacketDescriptor, UsbTransferDirection, UsbTransferFlag, UsbTransferKind,
    UsbTransferPayload, UsbTransferStatus, DISCOVERY_APP_ID, PROTOCOL_VERSION,
};

// Re-exports from device
pub use device::{Device, DevicePosition, DeviceRegistry, DeviceStatus, ScreenLayout};

// Re-exports from config
pub use config::Config;
pub use config::{FeatureConfig, GamepadConfig, GamepadRoutingMode};

// Re-exports from epoch-scoped input contracts
pub use input::{
    AcceptRealtime, AcceptReliable, AuthenticatedInputOwner, InputOwnershipGate,
    PendingReleaseBatch, PressedStateLedger, PressedStateLedgerError, RealtimeInputFrame,
    RealtimeInputPayload, ReleaseAllReason, ReliableInputEvent, ReliableInputFrame, SessionEpoch,
    SessionEpochError, TransferError, INPUT_PROTOCOL_VERSION,
};

// Re-exports from the pure single-owner input router.
pub use input_router::{InputRouter, RouterCommand, RouterInput, RouterMetric, RouterOutput};

// Re-exports from monotonic performance contracts used by input frames
pub use perf::{ClockDomainId, MonotonicStamp};

// Re-exports from hardware asset packs
pub use hardware_assets::{
    validate_asset_relative_path, HardwareAssetKind, HardwareAssetLayer, HardwareAssetManifest,
    HardwareAssetSize, HardwareAssetValidationError, HardwareControlAction, HardwareControlRegion,
    HardwareMaskChannel, HardwareMaskMapping, HardwarePoint, HardwareRegionShape,
    HARDWARE_ASSET_SCHEMA_VERSION,
};

// Re-exports from clipboard
pub use clipboard::ClipboardContent;

// Re-exports from capability registry
pub use capabilities::{
    local_capability_snapshots, remote_capability_snapshots, CapabilityRegistrySnapshot,
    CapabilityState, DeviceCapabilitySnapshot, EndpointCapabilityKind, EndpointCapabilitySnapshot,
};

// Re-exports from local daemon IPC
pub use ipc::{
    default_ipc_addr, default_local_controls_ws_addr, default_local_controls_ws_url,
    default_mobile_gateway_addr, read_json_line, write_json_line, DaemonDeviceSnapshot,
    DaemonRequest, DaemonResponse, LatencyFeedbackSnapshot, LatencyFeedbackStatus,
    LocalInputFeedback, MobileAccessSnapshot, NetworkTransportSnapshot,
    RemoteDeviceLatencyFeedback, RemoteLatencyFeedback, ServiceStatusSnapshot, TransportFeedback,
    UsbDescriptorProbeResult, UsbDescriptorProbeStatus,
};

// Re-exports from endpoint event observation/injection diagnostics
pub use endpoint_events::{
    DeviceAttribution, EndpointDeviceRef, EndpointEvent, EndpointEventDirection,
    EndpointEventFilter, EndpointEventId, EndpointEventKind, EndpointEventPayload,
    EndpointEventSource, EndpointEventStore, EndpointInjectError, EndpointInjectMode,
    EndpointInjectRequest, EndpointInjectResult, EndpointInjectTarget, EventCorrelationId,
    DEFAULT_ENDPOINT_EVENT_LIMIT,
};

// Re-exports from local control diagnostics
pub use local_controls::{
    DisplayCaptureRequest, DisplayCaptureResult, DisplayIdentifyRequest, DisplayIdentifyResult,
    DisplayModeInfo, DisplayOperationStatus, DisplayOrientation, DisplaySettingsUpdateRequest,
    DisplaySettingsUpdateResult, DisplayWriteCapabilities, LocalAudioCaptureSource,
    LocalAudioCaptureState, LocalAudioCaptureStatus, LocalAudioEndpointFormFactor,
    LocalAudioInputDevice, LocalAudioInputKind, LocalAudioOutputDevice, LocalAudioStreamState,
    LocalAudioTestRequest, LocalAudioTestResult, LocalAudioTestStatus, LocalBackendDiagnosticState,
    LocalControlDeviceSnapshot, LocalDisplayInfo, LocalDisplayState, LocalDriverDiagnosticState,
    LocalGamepadState, LocalHardwareDevice, LocalInputDeviceKind, LocalInputDiagnosticEvent,
    LocalInputEventSource, LocalInputTestKind, LocalInputTestRequest, LocalInputTestResult,
    LocalInputTestStatus, LocalKeyboardState, LocalMouseState, LocalVirtualGamepadState,
    RemoteUsbDeviceSnapshot, VirtualDisplayCreateRequest, VirtualDisplayOperationResult,
    VirtualDisplayOperationStatus, VirtualDisplayRemoveRequest, VirtualDisplaySnapshot,
    VirtualDisplayStatus,
};

// Re-exports from input_mode
pub use input_mode::{
    BackendFailureReason, BackendHealth, BackendKind, PrivilegeState, ResolvedInputMode,
};

// Re-exports from runtime
pub use runtime::{
    BackendRuntimeState, BackgroundProcessOwner, BackgroundRunMode, ConnectionState,
    ControlSessionState, DiscoveryState, PeerDirectoryEntry, SuspendReason, TrayRuntimeState,
};

// Re-exports from layout
pub use layout::{
    DisplayNode, LayoutGraph, LayoutLink, LayoutNode, PixelRect, RouteCache, RouteTarget,
    VirtualDesktopGeometry,
};

// Re-exports from session
pub use session::{CaptureSessionStateMachine, TransitionError};
