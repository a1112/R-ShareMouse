//! Canonical endpoint event model for observation and injection diagnostics.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

use crate::{
    BackendHealth, BackendKind, DeviceId, LocalInputDeviceKind, LocalInputDiagnosticEvent,
    LocalInputEventSource,
};

pub const DEFAULT_ENDPOINT_EVENT_LIMIT: usize = 512;

const SAMPLED_CONTROL_METRICS_SUMMARY: &str = "Sampled control-path metrics";

pub type EndpointEventId = u64;
pub type EventCorrelationId = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointEvent {
    pub event_id: EndpointEventId,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub endpoint_id: DeviceId,
    pub origin_endpoint_id: DeviceId,
    pub device: EndpointDeviceRef,
    pub direction: EndpointEventDirection,
    pub source: EndpointEventSource,
    pub kind: EndpointEventKind,
    pub payload: EndpointEventPayload,
    #[serde(default)]
    pub correlation_id: Option<EventCorrelationId>,
}

impl EndpointEvent {
    pub fn from_local_diagnostic(endpoint_id: DeviceId, event: LocalInputDiagnosticEvent) -> Self {
        let kind = EndpointEventKind::from(event.device_kind);
        let direction = EndpointEventDirection::from(event.source);
        let source = EndpointEventSource::from(event.source);
        let device_id = event
            .device_id
            .clone()
            .unwrap_or_else(|| aggregate_device_id(kind));
        let attribution = if event.device_id.is_some()
            || event.device_instance_id.is_some()
            || event.capture_path.is_some()
        {
            DeviceAttribution::Exact
        } else {
            DeviceAttribution::Aggregate
        };
        let correlation_id = event.payload.get("correlation_id").cloned();

        Self {
            event_id: event.sequence,
            sequence: event.sequence,
            timestamp_ms: event.timestamp_ms,
            endpoint_id,
            origin_endpoint_id: endpoint_id,
            device: EndpointDeviceRef {
                device_id,
                instance_id: event.device_instance_id.clone(),
                display_name: endpoint_display_name(kind, &event),
                kind,
                attribution,
            },
            direction,
            source,
            kind,
            payload: EndpointEventPayload::from_diagnostic(&event),
            correlation_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointDeviceRef {
    pub device_id: String,
    #[serde(default)]
    pub instance_id: Option<String>,
    pub display_name: String,
    pub kind: EndpointEventKind,
    pub attribution: DeviceAttribution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceAttribution {
    Exact,
    Aggregate,
    Inferred,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointEventDirection {
    Observed,
    Injected,
    InjectedLoopback,
    ForwardedIn,
    ForwardedOut,
    System,
}

impl From<LocalInputEventSource> for EndpointEventDirection {
    fn from(source: LocalInputEventSource) -> Self {
        match source {
            LocalInputEventSource::Injected => Self::Injected,
            LocalInputEventSource::InjectedLoopback => Self::InjectedLoopback,
            LocalInputEventSource::DriverTest | LocalInputEventSource::VirtualDevice => {
                Self::Injected
            }
            LocalInputEventSource::System => Self::System,
            LocalInputEventSource::Hardware => Self::Observed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointEventSource {
    Hardware,
    UserModeHook,
    Driver,
    VirtualHid,
    SendInput,
    Audio,
    Display,
    RemoteMirror,
    Test,
    System,
    Unknown,
}

impl From<LocalInputEventSource> for EndpointEventSource {
    fn from(source: LocalInputEventSource) -> Self {
        match source {
            LocalInputEventSource::Hardware => Self::Hardware,
            LocalInputEventSource::Injected | LocalInputEventSource::InjectedLoopback => {
                Self::SendInput
            }
            LocalInputEventSource::DriverTest => Self::Driver,
            LocalInputEventSource::VirtualDevice => Self::VirtualHid,
            LocalInputEventSource::System => Self::System,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointEventKind {
    Keyboard,
    Mouse,
    Gamepad,
    Usb,
    Display,
    Audio,
    Backend,
    Session,
}

impl From<LocalInputDeviceKind> for EndpointEventKind {
    fn from(kind: LocalInputDeviceKind) -> Self {
        match kind {
            LocalInputDeviceKind::Keyboard => Self::Keyboard,
            LocalInputDeviceKind::Mouse => Self::Mouse,
            LocalInputDeviceKind::Gamepad => Self::Gamepad,
            LocalInputDeviceKind::Usb => Self::Usb,
            LocalInputDeviceKind::Display => Self::Display,
            LocalInputDeviceKind::Audio => Self::Audio,
            LocalInputDeviceKind::Backend => Self::Backend,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum EndpointEventPayload {
    Keyboard {
        key: String,
        state: String,
    },
    TextCommit {
        text: String,
    },
    MouseMove {
        x: i32,
        y: i32,
        #[serde(default)]
        display_id: Option<String>,
    },
    MouseButton {
        button: String,
        state: String,
        x: i32,
        y: i32,
    },
    MouseWheel {
        delta_x: i32,
        delta_y: i32,
        x: i32,
        y: i32,
    },
    Gamepad {
        summary: String,
        fields: BTreeMap<String, String>,
    },
    Display {
        summary: String,
        fields: BTreeMap<String, String>,
    },
    Audio {
        summary: String,
        fields: BTreeMap<String, String>,
    },
    Backend {
        summary: String,
        fields: BTreeMap<String, String>,
    },
    Generic {
        summary: String,
        fields: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointInjectTarget {
    Local,
    Remote(DeviceId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointInjectMode {
    BestEffort,
    RequireHealthyBackend,
    TestLoopback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointInjectRequest {
    pub correlation_id: EventCorrelationId,
    pub device_kind: EndpointEventKind,
    pub payload: EndpointEventPayload,
    pub mode: EndpointInjectMode,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointInjectResult {
    pub correlation_id: EventCorrelationId,
    pub target: EndpointInjectTarget,
    pub accepted: bool,
    pub backend_kind: Option<BackendKind>,
    pub health: BackendHealth,
    pub elapsed_ms: u64,
    pub loopback_event_id: Option<EndpointEventId>,
    pub error: Option<EndpointInjectError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointInjectError {
    BackendUnavailable,
    BackendDegraded,
    PermissionDenied,
    UnsupportedEvent,
    TargetDisconnected,
    Timeout,
    RejectedByPolicy,
    TransportFailed,
    Failed,
}

impl EndpointEventPayload {
    fn from_diagnostic(event: &LocalInputDiagnosticEvent) -> Self {
        match event.device_kind {
            LocalInputDeviceKind::Keyboard if event.event_kind == "text" => Self::Generic {
                summary: event.summary.clone(),
                fields: event.payload.clone(),
            },
            LocalInputDeviceKind::Keyboard => Self::Keyboard {
                key: payload_string(&event.payload, "key", &event.summary),
                state: payload_string(&event.payload, "state", &event.event_kind),
            },
            LocalInputDeviceKind::Mouse if event.event_kind == "move" => Self::MouseMove {
                x: payload_i32(&event.payload, "x"),
                y: payload_i32(&event.payload, "y"),
                display_id: event.payload.get("display_id").cloned(),
            },
            LocalInputDeviceKind::Mouse if event.event_kind == "button" => Self::MouseButton {
                button: payload_string(&event.payload, "button", "Unknown"),
                state: payload_string(&event.payload, "state", &event.event_kind),
                x: payload_i32(&event.payload, "x"),
                y: payload_i32(&event.payload, "y"),
            },
            LocalInputDeviceKind::Mouse if event.event_kind == "wheel" => Self::MouseWheel {
                delta_x: payload_i32(&event.payload, "delta_x"),
                delta_y: payload_i32(&event.payload, "delta_y"),
                x: payload_i32(&event.payload, "x"),
                y: payload_i32(&event.payload, "y"),
            },
            LocalInputDeviceKind::Gamepad => Self::Gamepad {
                summary: event.summary.clone(),
                fields: event.payload.clone(),
            },
            LocalInputDeviceKind::Display => Self::Display {
                summary: event.summary.clone(),
                fields: event.payload.clone(),
            },
            LocalInputDeviceKind::Audio => Self::Audio {
                summary: event.summary.clone(),
                fields: event.payload.clone(),
            },
            LocalInputDeviceKind::Backend => Self::Backend {
                summary: event.summary.clone(),
                fields: event.payload.clone(),
            },
            _ => Self::Generic {
                summary: event.summary.clone(),
                fields: event.payload.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointEventFilter {
    #[serde(default)]
    pub endpoint_id: Option<DeviceId>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub kinds: Vec<EndpointEventKind>,
    #[serde(default)]
    pub sources: Vec<EndpointEventSource>,
    #[serde(default)]
    pub include_loopback: bool,
}

impl EndpointEventFilter {
    pub fn matches(&self, event: &EndpointEvent) -> bool {
        if self
            .endpoint_id
            .map_or(false, |endpoint_id| endpoint_id != event.endpoint_id)
        {
            return false;
        }
        if self
            .device_id
            .as_ref()
            .map_or(false, |device_id| device_id != &event.device.device_id)
        {
            return false;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&event.kind) {
            return false;
        }
        if !self.sources.is_empty() && !self.sources.contains(&event.source) {
            return false;
        }
        if !self.include_loopback
            && matches!(event.direction, EndpointEventDirection::InjectedLoopback)
        {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct EndpointEventStore {
    limit: usize,
    recent: VecDeque<EndpointEvent>,
}

impl EndpointEventStore {
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            recent: VecDeque::new(),
        }
    }

    pub fn push(&mut self, event: EndpointEvent) {
        if self
            .recent
            .back()
            .map_or(false, |existing| existing.sequence == event.sequence)
        {
            return;
        }

        if is_sampled_control_metrics(&event) {
            if let Some(existing) = self.recent.back_mut().filter(|existing| {
                existing.endpoint_id == event.endpoint_id && is_sampled_control_metrics(existing)
            }) {
                // Metrics are a latest-state sample, not a discrete event. Keep
                // the newest sample in the same slot so a 20 Hz metrics stream
                // cannot consume the history budget needed by real input.
                *existing = event;
                return;
            }

            if self.recent.len() >= self.limit {
                // Prefer dropping an older sampled metric over a real endpoint
                // event. If the store is entirely discrete, drop this optional
                // sample instead of evicting keyboard/mouse/gamepad history.
                if let Some(index) = self.recent.iter().position(is_sampled_control_metrics) {
                    self.recent.remove(index);
                } else {
                    return;
                }
            }
        }

        self.recent.push_back(event);
        while self.recent.len() > self.limit {
            self.recent.pop_front();
        }
    }

    pub fn last_sequence(&self) -> Option<u64> {
        self.recent.back().map(|event| event.sequence)
    }

    pub fn query(
        &self,
        filter: &EndpointEventFilter,
        after_sequence: Option<u64>,
        limit: Option<u16>,
    ) -> Vec<EndpointEvent> {
        let limit = usize::from(limit.unwrap_or(128));
        self.recent
            .iter()
            .filter(|event| after_sequence.map_or(true, |after| event.sequence > after))
            .filter(|event| filter.matches(event))
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
}

impl Default for EndpointEventStore {
    fn default() -> Self {
        Self::new(DEFAULT_ENDPOINT_EVENT_LIMIT)
    }
}

/// `EndpointEvent` intentionally does not retain the source diagnostic's
/// `event_kind`. The metrics publication has a stable summary and payload
/// marker, which lets the store distinguish a replaceable sample from a
/// discrete backend diagnostic such as a latency acknowledgement.
fn is_sampled_control_metrics(event: &EndpointEvent) -> bool {
    if event.kind != EndpointEventKind::Backend {
        return false;
    }

    let EndpointEventPayload::Backend { summary, fields } = &event.payload else {
        return false;
    };

    summary == SAMPLED_CONTROL_METRICS_SUMMARY
        || fields.contains_key("snapshot_json")
        || (fields.contains_key("captured") && fields.contains_key("routed"))
}

fn aggregate_device_id(kind: EndpointEventKind) -> String {
    match kind {
        EndpointEventKind::Keyboard => "keyboard-default",
        EndpointEventKind::Mouse => "mouse-default",
        EndpointEventKind::Gamepad => "gamepad-default",
        EndpointEventKind::Usb => "usb-default",
        EndpointEventKind::Display => "display-default",
        EndpointEventKind::Audio => "audio-default",
        EndpointEventKind::Backend => "backend",
        EndpointEventKind::Session => "session",
    }
    .to_string()
}

fn endpoint_display_name(kind: EndpointEventKind, event: &LocalInputDiagnosticEvent) -> String {
    event.device_id.clone().unwrap_or_else(|| match kind {
        EndpointEventKind::Keyboard => "Aggregate Keyboard".to_string(),
        EndpointEventKind::Mouse => "Aggregate Mouse".to_string(),
        EndpointEventKind::Gamepad => "Gamepad".to_string(),
        EndpointEventKind::Usb => "USB Device".to_string(),
        EndpointEventKind::Display => "Display".to_string(),
        EndpointEventKind::Audio => "Audio Endpoint".to_string(),
        EndpointEventKind::Backend => "Backend".to_string(),
        EndpointEventKind::Session => "Session".to_string(),
    })
}

fn payload_string(payload: &BTreeMap<String, String>, key: &str, fallback: &str) -> String {
    payload
        .get(key)
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn payload_i32(payload: &BTreeMap<String, String>, key: &str) -> i32 {
    payload
        .get(key)
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_diagnostic_event(
        endpoint_id: DeviceId,
        sequence: u64,
        device_kind: LocalInputDeviceKind,
        event_kind: &str,
        summary: &str,
        source: LocalInputEventSource,
        payload: BTreeMap<String, String>,
    ) -> EndpointEvent {
        EndpointEvent::from_local_diagnostic(
            endpoint_id,
            LocalInputDiagnosticEvent {
                sequence,
                timestamp_ms: 100 + sequence,
                device_kind,
                event_kind: event_kind.to_string(),
                summary: summary.to_string(),
                device_id: None,
                device_instance_id: None,
                capture_path: None,
                source,
                payload,
            },
        )
    }

    fn sampled_metrics_event(endpoint_id: DeviceId, sequence: u64) -> EndpointEvent {
        local_diagnostic_event(
            endpoint_id,
            sequence,
            LocalInputDeviceKind::Backend,
            "control_metrics_sample",
            SAMPLED_CONTROL_METRICS_SUMMARY,
            LocalInputEventSource::System,
            BTreeMap::from([
                ("snapshot_json".to_string(), "{}".to_string()),
                ("captured".to_string(), sequence.to_string()),
                ("routed".to_string(), "0".to_string()),
            ]),
        )
    }

    fn keyboard_event(sequence: u64) -> EndpointEvent {
        let mut payload = BTreeMap::new();
        payload.insert("key".to_string(), "ShiftLeft".to_string());
        payload.insert("state".to_string(), "Pressed".to_string());
        EndpointEvent::from_local_diagnostic(
            DeviceId::new_v4(),
            LocalInputDiagnosticEvent {
                sequence,
                timestamp_ms: 100 + sequence,
                device_kind: LocalInputDeviceKind::Keyboard,
                event_kind: "key".to_string(),
                summary: "Key ShiftLeft Pressed".to_string(),
                device_id: None,
                device_instance_id: None,
                capture_path: None,
                source: LocalInputEventSource::Hardware,
                payload,
            },
        )
    }

    #[test]
    fn endpoint_event_store_replaces_contiguous_metrics_for_same_endpoint() {
        let endpoint_id = DeviceId::new_v4();
        let mut store = EndpointEventStore::new(8);

        store.push(sampled_metrics_event(endpoint_id, 1));
        store.push(sampled_metrics_event(endpoint_id, 2));
        store.push(sampled_metrics_event(endpoint_id, 3));

        let events = store.query(
            &EndpointEventFilter {
                endpoint_id: Some(endpoint_id),
                kinds: vec![EndpointEventKind::Backend],
                ..EndpointEventFilter::default()
            },
            None,
            Some(8),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 3);
        assert_eq!(store.last_sequence(), Some(3));
    }

    #[test]
    fn endpoint_event_store_does_not_merge_metrics_across_endpoints() {
        let first_endpoint = DeviceId::new_v4();
        let second_endpoint = DeviceId::new_v4();
        let mut store = EndpointEventStore::new(8);

        store.push(sampled_metrics_event(first_endpoint, 1));
        store.push(sampled_metrics_event(second_endpoint, 2));
        store.push(sampled_metrics_event(first_endpoint, 3));

        let events = store.query(&EndpointEventFilter::default(), None, Some(8));

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].endpoint_id, first_endpoint);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[1].endpoint_id, second_endpoint);
        assert_eq!(events[1].sequence, 2);
        assert_eq!(events[2].endpoint_id, first_endpoint);
        assert_eq!(events[2].sequence, 3);
    }

    #[test]
    fn endpoint_event_store_preserves_real_input_when_metrics_fill_the_limit() {
        let endpoint_id = DeviceId::new_v4();
        let mut store = EndpointEventStore::new(4);

        for sequence in 1..=100 {
            store.push(sampled_metrics_event(endpoint_id, sequence));
        }

        let mut keyboard_payload = BTreeMap::new();
        keyboard_payload.insert("key".to_string(), "ShiftLeft".to_string());
        keyboard_payload.insert("state".to_string(), "Pressed".to_string());
        store.push(local_diagnostic_event(
            endpoint_id,
            101,
            LocalInputDeviceKind::Keyboard,
            "key",
            "Key ShiftLeft Pressed",
            LocalInputEventSource::Hardware,
            keyboard_payload,
        ));

        store.push(local_diagnostic_event(
            endpoint_id,
            102,
            LocalInputDeviceKind::Mouse,
            "move",
            "Mouse moved",
            LocalInputEventSource::Hardware,
            BTreeMap::from([
                ("x".to_string(), "10".to_string()),
                ("y".to_string(), "20".to_string()),
            ]),
        ));

        store.push(local_diagnostic_event(
            endpoint_id,
            103,
            LocalInputDeviceKind::Gamepad,
            "state",
            "Gamepad state",
            LocalInputEventSource::Hardware,
            BTreeMap::new(),
        ));

        for sequence in 104..=200 {
            store.push(sampled_metrics_event(endpoint_id, sequence));
        }

        let events = store.query(
            &EndpointEventFilter {
                endpoint_id: Some(endpoint_id),
                ..EndpointEventFilter::default()
            },
            None,
            Some(8),
        );

        assert_eq!(events.len(), 4);
        assert!(events
            .iter()
            .any(|event| { event.sequence == 101 && event.kind == EndpointEventKind::Keyboard }));
        assert!(events
            .iter()
            .any(|event| { event.sequence == 102 && event.kind == EndpointEventKind::Mouse }));
        assert!(events
            .iter()
            .any(|event| { event.sequence == 103 && event.kind == EndpointEventKind::Gamepad }));
        assert_eq!(
            events
                .iter()
                .filter(|event| is_sampled_control_metrics(event))
                .count(),
            1
        );
        assert_eq!(events.last().map(|event| event.sequence), Some(200));
    }

    #[test]
    fn endpoint_event_store_filters_after_sequence() {
        let endpoint_id = DeviceId::new_v4();
        let mut store = EndpointEventStore::new(8);
        let mut first = keyboard_event(1);
        first.endpoint_id = endpoint_id;
        let mut second = keyboard_event(2);
        second.endpoint_id = endpoint_id;
        store.push(first);
        store.push(second);

        let events = store.query(
            &EndpointEventFilter {
                endpoint_id: Some(endpoint_id),
                kinds: vec![EndpointEventKind::Keyboard],
                ..EndpointEventFilter::default()
            },
            Some(1),
            Some(10),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 2);
    }

    #[test]
    fn endpoint_event_filter_excludes_loopback_by_default() {
        let mut event = keyboard_event(1);
        event.direction = EndpointEventDirection::InjectedLoopback;

        assert!(!EndpointEventFilter::default().matches(&event));
        assert!(EndpointEventFilter {
            include_loopback: true,
            ..EndpointEventFilter::default()
        }
        .matches(&event));
    }

    #[test]
    fn text_diagnostic_serialization_keeps_count_without_a_text_field() {
        let mut payload = BTreeMap::new();
        payload.insert("char_count".to_string(), "4".to_string());
        let event = EndpointEvent::from_local_diagnostic(
            DeviceId::new_v4(),
            LocalInputDiagnosticEvent {
                sequence: 1,
                timestamp_ms: 101,
                device_kind: LocalInputDeviceKind::Keyboard,
                event_kind: "text".to_string(),
                summary: "Text commit 4 chars".to_string(),
                device_id: None,
                device_instance_id: None,
                capture_path: None,
                source: LocalInputEventSource::Hardware,
                payload,
            },
        );

        let serialized = serde_json::to_string(&event).unwrap();
        assert!(serialized.contains("char_count"));
        assert!(!serialized.contains("\"text\":"));
    }
}
