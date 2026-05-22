# Low-Latency Feedback Monitoring Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a daemon-owned monitoring and feedback system for local input responsiveness, remote latency probes, and transport health before doing latency optimization.

**Architecture:** Add additive IPC types in `rshare-core`, aggregate them in the daemon from existing runtime state and diagnostic events, then render the same daemon-owned summary in CLI and desktop models. Existing `recent_events` and latency probe messages remain the detailed event stream and compatibility fallback.

**Tech Stack:** Rust workspace (`rshare-core`, `rshare-daemon`, `rshare-cli`), serde JSON IPC, Tokio daemon tests, Node.js desktop model tests under `other/figma-ui`.

---

### Task 1: Add Core Feedback Types

**Files:**
- Modify: `crates/rshare-core/src/ipc.rs`
- Test: `crates/rshare-core/tests/ipc_contract.rs`

**Step 1: Write the failing IPC defaults test**

Add a test near the existing status snapshot tests:

```rust
#[test]
fn latency_feedback_defaults_to_safe_unavailable_state() {
    let snapshot: rshare_core::ServiceStatusSnapshot = serde_json::from_str(
        r#"{
            "device_id":"00000000-0000-0000-0000-000000000000",
            "device_name":"desktop",
            "hostname":"desktop-host",
            "bind_address":"0.0.0.0:27431",
            "discovery_port":27432,
            "pid":42,
            "discovered_devices":0,
            "connected_devices":0,
            "healthy":true
        }"#,
    )
    .unwrap();

    assert_eq!(
        snapshot.latency_feedback.transport.status,
        rshare_core::LatencyFeedbackStatus::Unavailable
    );
    assert!(snapshot.latency_feedback.remote_latency.devices.is_empty());
}
```

**Step 2: Run the test to verify it fails**

Run:

```bash
cargo test -p rshare-core latency_feedback_defaults_to_safe_unavailable_state
```

Expected: fail because `ServiceStatusSnapshot.latency_feedback` and feedback types do not exist.

**Step 3: Add minimal serializable types**

In `crates/rshare-core/src/ipc.rs`, add:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LatencyFeedbackStatus {
    Idle,
    Pending,
    Healthy,
    Degraded,
    Timeout,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LatencyFeedbackSnapshot {
    pub generated_at_ms: u64,
    pub local_input: LocalInputFeedback,
    pub remote_latency: RemoteLatencyFeedback,
    pub transport: TransportFeedback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalInputFeedback {
    pub status: LatencyFeedbackStatus,
    pub event_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_event_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_keyboard_event_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_mouse_event_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteLatencyFeedback {
    pub status: LatencyFeedbackStatus,
    pub devices: Vec<RemoteDeviceLatencyFeedback>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteDeviceLatencyFeedback {
    pub device_id: DeviceId,
    pub status: LatencyFeedbackStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_probe_sent_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ack_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_round_trip_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_round_trip_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_one_way_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_processing_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportFeedback {
    pub status: LatencyFeedbackStatus,
    pub transport: String,
    pub datagram_available: bool,
    pub realtime_degraded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_datagram_rx_ms: Option<u64>,
    pub datagram_tx_dropped: u64,
    pub reliable_stream_reset_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_trust_state: Option<String>,
}
```

Add `Default` implementations:

```rust
impl Default for LatencyFeedbackSnapshot {
    fn default() -> Self {
        Self {
            generated_at_ms: 0,
            local_input: LocalInputFeedback::default(),
            remote_latency: RemoteLatencyFeedback::default(),
            transport: TransportFeedback::default(),
        }
    }
}

impl Default for LocalInputFeedback {
    fn default() -> Self {
        Self {
            status: LatencyFeedbackStatus::Unavailable,
            event_count: 0,
            latest_sequence: None,
            latest_event_ms: None,
            latest_keyboard_event_ms: None,
            latest_mouse_event_ms: None,
            capture_path: None,
        }
    }
}

impl Default for RemoteLatencyFeedback {
    fn default() -> Self {
        Self {
            status: LatencyFeedbackStatus::Unavailable,
            devices: Vec::new(),
        }
    }
}

impl Default for TransportFeedback {
    fn default() -> Self {
        Self {
            status: LatencyFeedbackStatus::Unavailable,
            transport: "quic".to_string(),
            datagram_available: false,
            realtime_degraded: true,
            rtt_ms: None,
            last_datagram_rx_ms: None,
            datagram_tx_dropped: 0,
            reliable_stream_reset_count: 0,
            cert_trust_state: None,
        }
    }
}
```

Add the field to `ServiceStatusSnapshot`:

```rust
#[serde(default)]
pub latency_feedback: LatencyFeedbackSnapshot,
```

Initialize it in `ServiceStatusSnapshot::new()`.

**Step 4: Re-export the new types**

Modify `crates/rshare-core/src/lib.rs` to re-export:

```rust
LatencyFeedbackSnapshot, LatencyFeedbackStatus, LocalInputFeedback,
RemoteDeviceLatencyFeedback, RemoteLatencyFeedback, TransportFeedback,
```

**Step 5: Run tests**

Run:

```bash
cargo test -p rshare-core latency_feedback
cargo test -p rshare-core ipc_contract
```

Expected: pass.

**Step 6: Commit**

```bash
git add crates/rshare-core/src/ipc.rs crates/rshare-core/src/lib.rs crates/rshare-core/tests/ipc_contract.rs
git commit -m "Add latency feedback IPC types"
```

### Task 2: Aggregate Transport Feedback In The Daemon

**Files:**
- Modify: `apps/rshare-daemon/src/main.rs`

**Step 1: Write the failing daemon transport tests**

Add tests near `latency_payload_subtracts_remote_processing_time()`:

```rust
#[test]
fn transport_feedback_reports_unavailable_without_connections() {
    let network = network_snapshot_from_connections(&[]);
    let feedback = transport_feedback_from_network(&network, 0);

    assert_eq!(feedback.status, LatencyFeedbackStatus::Unavailable);
    assert!(feedback.realtime_degraded);
}

#[test]
fn transport_feedback_reports_healthy_realtime_connection() {
    let network = NetworkTransportSnapshot {
        datagram_available: true,
        realtime_degraded: false,
        rtt_ms: Some(12),
        ..NetworkTransportSnapshot::default()
    };

    let feedback = transport_feedback_from_network(&network, 1);

    assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
    assert_eq!(feedback.rtt_ms, Some(12));
}

#[test]
fn transport_feedback_degrades_when_realtime_is_degraded() {
    let network = NetworkTransportSnapshot {
        datagram_available: false,
        realtime_degraded: true,
        rtt_ms: Some(22),
        ..NetworkTransportSnapshot::default()
    };

    let feedback = transport_feedback_from_network(&network, 1);

    assert_eq!(feedback.status, LatencyFeedbackStatus::Degraded);
}
```

**Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test -p rshare-daemon transport_feedback
```

Expected: fail because `transport_feedback_from_network` does not exist.

**Step 3: Implement transport aggregation**

Add constants near daemon helper constants:

```rust
const LATENCY_HEALTHY_RTT_MS: u64 = 50;
const LATENCY_DEGRADED_RTT_MS: u64 = 120;
```

Add helper:

```rust
fn transport_feedback_from_network(
    network: &NetworkTransportSnapshot,
    connected_devices: usize,
) -> TransportFeedback {
    let status = if connected_devices == 0 {
        LatencyFeedbackStatus::Unavailable
    } else if network.realtime_degraded
        || !network.datagram_available
        || network.datagram_tx_dropped > 0
        || network.reliable_stream_reset_count > 0
        || network
            .rtt_ms
            .is_some_and(|rtt| rtt >= LATENCY_DEGRADED_RTT_MS)
    {
        LatencyFeedbackStatus::Degraded
    } else if network
        .rtt_ms
        .is_some_and(|rtt| rtt > LATENCY_HEALTHY_RTT_MS)
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
```

Import the new types from `rshare_core` at the top of `apps/rshare-daemon/src/main.rs`.

**Step 4: Run tests**

Run:

```bash
cargo test -p rshare-daemon transport_feedback
```

Expected: pass.

**Step 5: Commit**

```bash
git add apps/rshare-daemon/src/main.rs
git commit -m "Add daemon transport feedback summary"
```

### Task 3: Aggregate Local Input Feedback

**Files:**
- Modify: `apps/rshare-daemon/src/main.rs`

**Step 1: Write the failing local input feedback tests**

Add tests near `local_input_event_updates_diagnostic_snapshot()`:

```rust
#[test]
fn local_input_feedback_is_idle_when_backend_is_healthy_without_events() {
    let state = test_daemon_state();

    let feedback = state.local_input_feedback();

    assert_eq!(feedback.status, LatencyFeedbackStatus::Idle);
    assert_eq!(feedback.event_count, 0);
}

#[test]
fn local_input_feedback_uses_latest_keyboard_and_mouse_events() {
    let mut state = test_daemon_state();
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
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p rshare-daemon local_input_feedback
```

Expected: fail because `DaemonState::local_input_feedback` does not exist.

**Step 3: Implement local input aggregation**

Add an inherent method on `DaemonState`:

```rust
fn local_input_feedback(&self) -> LocalInputFeedback {
    if !self.backend_state.has_end_to_end_path() {
        return LocalInputFeedback {
            status: LatencyFeedbackStatus::Unavailable,
            event_count: self.local_controls.keyboard.event_count
                + self.local_controls.mouse.event_count,
            ..LocalInputFeedback::default()
        };
    }

    let latest_event = self.local_controls.recent_events.iter().max_by_key(|event| {
        (event.timestamp_ms, event.sequence)
    });
    let latest_keyboard = self
        .local_controls
        .recent_events
        .iter()
        .filter(|event| event.device_kind == LocalInputDeviceKind::Keyboard)
        .max_by_key(|event| (event.timestamp_ms, event.sequence));
    let latest_mouse = self
        .local_controls
        .recent_events
        .iter()
        .filter(|event| event.device_kind == LocalInputDeviceKind::Mouse)
        .max_by_key(|event| (event.timestamp_ms, event.sequence));
    let event_count = self.local_controls.keyboard.event_count
        + self.local_controls.mouse.event_count;

    LocalInputFeedback {
        status: if event_count == 0 {
            LatencyFeedbackStatus::Idle
        } else {
            LatencyFeedbackStatus::Healthy
        },
        event_count,
        latest_sequence: latest_event.map(|event| event.sequence),
        latest_event_ms: latest_event.map(|event| event.timestamp_ms),
        latest_keyboard_event_ms: latest_keyboard.map(|event| event.timestamp_ms),
        latest_mouse_event_ms: latest_mouse.map(|event| event.timestamp_ms),
        capture_path: latest_event.and_then(|event| event.capture_path.clone()),
    }
}
```

If `test_daemon_state()` does not set `selected_mode`, update the test setup only enough to make the healthy idle case truthful.

**Step 4: Run tests**

Run:

```bash
cargo test -p rshare-daemon local_input_feedback
```

Expected: pass.

**Step 5: Commit**

```bash
git add apps/rshare-daemon/src/main.rs
git commit -m "Add daemon local input feedback summary"
```

### Task 4: Aggregate Remote Latency Feedback

**Files:**
- Modify: `apps/rshare-daemon/src/main.rs`

**Step 1: Write failing remote latency tests**

Add tests covering event-derived feedback:

```rust
#[test]
fn remote_latency_feedback_reports_pending_probe() {
    let remote_id = Uuid::new_v4();
    let mut state = test_daemon_state();
    mark_test_device_connected(&mut state, remote_id, "remote");
    state.pending_latency_probes.insert(
        7,
        PendingLatencyProbe {
            target: remote_id,
            sent_at_ms: 1_000,
            role: PendingLatencyProbeRole::LocalRequested,
        },
    );

    let feedback = state.remote_latency_feedback(1_250);
    let device = feedback.devices.iter().find(|item| item.device_id == remote_id).unwrap();

    assert_eq!(device.status, LatencyFeedbackStatus::Pending);
    assert_eq!(device.pending_duration_ms, Some(250));
}

#[test]
fn remote_latency_feedback_reports_timeout_for_stale_pending_probe() {
    let remote_id = Uuid::new_v4();
    let mut state = test_daemon_state();
    mark_test_device_connected(&mut state, remote_id, "remote");
    state.pending_latency_probes.insert(
        7,
        PendingLatencyProbe {
            target: remote_id,
            sent_at_ms: 1_000,
            role: PendingLatencyProbeRole::LocalRequested,
        },
    );

    let feedback = state.remote_latency_feedback(3_000);
    let device = feedback.devices.iter().find(|item| item.device_id == remote_id).unwrap();

    assert_eq!(device.status, LatencyFeedbackStatus::Timeout);
}

#[test]
fn remote_latency_feedback_reports_ack_metrics() {
    let remote_id = Uuid::new_v4();
    let mut state = test_daemon_state();
    mark_test_device_connected(&mut state, remote_id, "remote");
    let mut payload = BTreeMap::new();
    payload.insert("target_device_id".to_string(), remote_id.to_string());
    payload.insert("network_round_trip_ms".to_string(), "24".to_string());
    payload.insert("raw_round_trip_ms".to_string(), "30".to_string());
    payload.insert("estimated_one_way_ms".to_string(), "12".to_string());
    payload.insert("remote_processing_ms".to_string(), "6".to_string());
    payload.insert("direction".to_string(), "origin_to_endpoint".to_string());
    record_latency_diagnostic_event(
        &mut state,
        remote_id,
        "latency_probe_ack",
        "Latency to remote: 24 ms RTT / ~12 ms one-way",
        payload,
    );

    let feedback = state.remote_latency_feedback(timestamp_ms_now());
    let device = feedback.devices.iter().find(|item| item.device_id == remote_id).unwrap();

    assert_eq!(device.status, LatencyFeedbackStatus::Healthy);
    assert_eq!(device.network_round_trip_ms, Some(24));
    assert_eq!(device.estimated_one_way_ms, Some(12));
}
```

Add a tiny test helper if needed:

```rust
fn mark_test_device_connected(state: &mut DaemonState, device_id: DeviceId, name: &str) {
    state.devices.insert(
        device_id,
        PeerDirectoryEntry {
            id: device_id,
            name: name.to_string(),
            hostname: format!("{name}-host"),
            addresses: vec!["127.0.0.1:27431".to_string()],
            discovery_state: DiscoveryState::Discovered,
            connection_state: ConnectionState::Connected,
            last_seen_secs: 0,
            last_error: None,
        },
    );
}
```

**Step 2: Run tests to verify failure**

Run:

```bash
cargo test -p rshare-daemon remote_latency_feedback
```

Expected: fail because aggregation does not exist.

**Step 3: Implement remote latency aggregation**

Add constant:

```rust
const LATENCY_PROBE_TIMEOUT_MS: u64 = 1_500;
```

Add helpers:

```rust
fn parse_payload_u64(payload: &BTreeMap<String, String>, key: &str) -> Option<u64> {
    payload.get(key).and_then(|value| value.parse::<u64>().ok())
}

fn latency_event_target(event: &LocalInputDiagnosticEvent) -> Option<DeviceId> {
    event
        .payload
        .get("target_device_id")
        .or_else(|| event.payload.get("origin_device_id"))
        .or(event.device_id.as_ref())
        .and_then(|value| value.parse::<DeviceId>().ok())
}
```

Add `DaemonState::remote_latency_feedback(&self, now_ms: u64) -> RemoteLatencyFeedback`.

Implementation rules:

- Iterate `self.devices` so disconnected known devices can report `Unavailable`.
- For each connected device, find newest pending probe in `self.pending_latency_probes`.
- Find newest ACK event in `self.local_controls.recent_events` whose target matches the device and kind is `latency_probe_ack` or `latency_endpoint_switch_ack`.
- Pending probe wins when it is newer than the latest ACK.
- Pending older than `LATENCY_PROBE_TIMEOUT_MS` reports `Timeout`.
- ACK with RTT above `LATENCY_HEALTHY_RTT_MS` reports `Degraded`; otherwise `Healthy`.
- Connected device with no pending and no ACK reports `Idle`.
- Aggregate `RemoteLatencyFeedback.status` should be the worst status among devices, with `Unavailable` only when there are no devices.

**Step 4: Run tests**

Run:

```bash
cargo test -p rshare-daemon remote_latency_feedback
```

Expected: pass.

**Step 5: Commit**

```bash
git add apps/rshare-daemon/src/main.rs
git commit -m "Add daemon remote latency feedback summary"
```

### Task 5: Wire Feedback Into Status Snapshots

**Files:**
- Modify: `apps/rshare-daemon/src/main.rs`
- Test: existing daemon tests in the same file

**Step 1: Write the failing status snapshot test**

Add:

```rust
#[test]
fn status_snapshot_includes_latency_feedback() {
    let mut state = test_daemon_state();
    state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);

    let snapshot = state.status_snapshot();

    assert_eq!(snapshot.latency_feedback.local_input.status, LatencyFeedbackStatus::Idle);
    assert_eq!(
        snapshot.latency_feedback.transport.status,
        LatencyFeedbackStatus::Unavailable
    );
}
```

**Step 2: Run the test to verify it fails**

Run:

```bash
cargo test -p rshare-daemon status_snapshot_includes_latency_feedback
```

Expected: fail because `status_snapshot()` does not populate feedback yet.

**Step 3: Implement `DaemonState::latency_feedback_snapshot`**

Add:

```rust
fn latency_feedback_snapshot(&self, network: &NetworkTransportSnapshot) -> LatencyFeedbackSnapshot {
    let now_ms = timestamp_ms_now();
    LatencyFeedbackSnapshot {
        generated_at_ms: now_ms,
        local_input: self.local_input_feedback(),
        remote_latency: self.remote_latency_feedback(now_ms),
        transport: transport_feedback_from_network(network, self.status.connected_devices),
    }
}
```

Update `status_snapshot()` after `snapshot.connected_devices` and `snapshot.network` are set:

```rust
snapshot.latency_feedback = self.latency_feedback_snapshot(&snapshot.network);
```

If `snapshot.network` is populated later outside `status_snapshot()`, move the feedback update to the same call site that sets network diagnostics so transport and feedback use the same data.

**Step 4: Run tests**

Run:

```bash
cargo test -p rshare-daemon status_snapshot_includes_latency_feedback
cargo test -p rshare-daemon latency_feedback
```

Expected: pass.

**Step 5: Commit**

```bash
git add apps/rshare-daemon/src/main.rs
git commit -m "Expose latency feedback in daemon status"
```

### Task 6: Show Latency Feedback In CLI Status

**Files:**
- Modify: `apps/rshare-cli/src/commands/status.rs`

**Step 1: Write or extend CLI formatting tests**

If `status.rs` has no tests, first extract rendering into a small pure helper:

```rust
fn latency_feedback_lines(feedback: &rshare_core::LatencyFeedbackSnapshot) -> Vec<(String, String)> {
    // implement in Step 3
}
```

Add tests in `status.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_feedback_lines_include_remote_metrics() {
        let remote_id = uuid::Uuid::nil();
        let feedback = rshare_core::LatencyFeedbackSnapshot {
            remote_latency: rshare_core::RemoteLatencyFeedback {
                status: rshare_core::LatencyFeedbackStatus::Healthy,
                devices: vec![rshare_core::RemoteDeviceLatencyFeedback {
                    device_id: remote_id,
                    device_name: Some("remote".to_string()),
                    status: rshare_core::LatencyFeedbackStatus::Healthy,
                    last_probe_sent_ms: None,
                    last_ack_ms: Some(1_000),
                    pending_duration_ms: None,
                    network_round_trip_ms: Some(24),
                    raw_round_trip_ms: None,
                    estimated_one_way_ms: Some(12),
                    remote_processing_ms: None,
                    direction: Some("origin_to_endpoint".to_string()),
                    summary: Some("Latency to remote: 24 ms RTT / ~12 ms one-way".to_string()),
                }],
            },
            ..rshare_core::LatencyFeedbackSnapshot::default()
        };

        let lines = latency_feedback_lines(&feedback);

        assert!(lines.iter().any(|(key, value)| {
            key == "remote" && value.contains("24 ms RTT") && value.contains("12 ms one-way")
        }));
    }
}
```

**Step 2: Run test to verify failure**

Run:

```bash
cargo test -p rshare-cli latency_feedback_lines_include_remote_metrics
```

Expected: fail because helper/output does not exist.

**Step 3: Implement CLI output**

In `print_detailed_status()`, add a section after `Network`:

```rust
println!();
println!("{}", "Latency Feedback".bold());
for (key, value) in latency_feedback_lines(&status.latency_feedback) {
    kv(&key, &value);
}
```

Helper behavior:

- Always print local input and transport status.
- For each remote device, print name or short ID plus status and metrics.
- Use `"pending {ms} ms"` when pending.
- Use `"timeout"` when timed out.
- Use `"unavailable"` when unavailable.

**Step 4: Run tests**

Run:

```bash
cargo test -p rshare-cli latency_feedback
```

Expected: pass.

**Step 5: Commit**

```bash
git add apps/rshare-cli/src/commands/status.rs
git commit -m "Show latency feedback in CLI status"
```

### Task 7: Prefer Daemon Feedback In Desktop Model

**Files:**
- Modify: `other/figma-ui/src/app/desktop-model.mjs`
- Test: `other/figma-ui/src/app/desktop-model.test.mjs`

**Step 1: Write failing frontend model tests**

Add tests near the existing remote latency summary tests:

```javascript
test("buildRemoteLatencySummary prefers daemon latency feedback", () => {
  const deviceId = "00000000-0000-0000-0000-000000000001";
  const snapshot = {
    latency_feedback: {
      remote_latency: {
        devices: [
          {
            device_id: deviceId,
            status: "Healthy",
            summary: "Latency to remote: 24 ms RTT / ~12 ms one-way",
            network_round_trip_ms: 24,
            estimated_one_way_ms: 12,
            raw_round_trip_ms: 30,
            remote_processing_ms: 6,
            direction: "origin_to_endpoint",
            last_ack_ms: 1000,
          },
        ],
      },
    },
    recent_events: [
      {
        device_kind: "Backend",
        event_kind: "latency_probe_ack",
        timestamp_ms: 900,
        device_id: deviceId,
        payload: { network_round_trip_ms: "99" },
      },
    ],
  };

  const summary = buildRemoteLatencySummary(snapshot, deviceId);

  assert.equal(summary.state, "pass");
  assert.equal(summary.networkRoundTripMs, 24);
  assert.equal(summary.estimatedOneWayMs, 12);
});

test("buildRemoteLatencySummary maps daemon timeout feedback", () => {
  const deviceId = "00000000-0000-0000-0000-000000000001";
  const summary = buildRemoteLatencySummary(
    {
      latency_feedback: {
        remote_latency: {
          devices: [
            {
              device_id: deviceId,
              status: "Timeout",
              pending_duration_ms: 1800,
            },
          ],
        },
      },
      recent_events: [],
    },
    deviceId,
  );

  assert.equal(summary.state, "fail");
  assert.match(summary.message, /超时/);
});
```

**Step 2: Run tests to verify failure**

Run:

```bash
cd other/figma-ui
npm test -- desktop-model
```

Expected: fail because `buildRemoteLatencySummary()` ignores `latency_feedback`.

**Step 3: Implement frontend fallback order**

At the top of `buildRemoteLatencySummary(snapshot, deviceId)`:

```javascript
const feedback = snapshot?.latency_feedback?.remote_latency?.devices?.find(
  (device) => device?.device_id === deviceId,
);
if (feedback) {
  const status = String(feedback.status ?? "").toLowerCase();
  if (status === "healthy" || status === "degraded") {
    return {
      state: status === "healthy" ? "pass" : "warn",
      message: feedback.summary ?? "Latency ACK received",
      networkRoundTripMs: numberOrNull(feedback.network_round_trip_ms),
      estimatedOneWayMs: numberOrNull(feedback.estimated_one_way_ms),
      rawRoundTripMs: numberOrNull(feedback.raw_round_trip_ms),
      remoteProcessingMs: numberOrNull(feedback.remote_processing_ms),
      direction: feedback.direction ?? null,
      timestampMs: Number(feedback.last_ack_ms ?? 0) || null,
    };
  }
  if (status === "pending") {
    return {
      state: "pending",
      message: "等待远端 latency ACK",
      networkRoundTripMs: null,
      estimatedOneWayMs: null,
      rawRoundTripMs: null,
      remoteProcessingMs: null,
      direction: feedback.direction ?? null,
      timestampMs: Number(feedback.last_probe_sent_ms ?? 0) || null,
    };
  }
  if (status === "timeout") {
    return {
      state: "fail",
      message: `远端 latency ACK 超时${feedback.pending_duration_ms ? `：${feedback.pending_duration_ms} ms` : ""}`,
      networkRoundTripMs: null,
      estimatedOneWayMs: null,
      rawRoundTripMs: null,
      remoteProcessingMs: null,
      direction: feedback.direction ?? null,
      timestampMs: Number(feedback.last_probe_sent_ms ?? 0) || null,
    };
  }
}
```

Keep the current `recent_events` logic as fallback.

**Step 4: Run tests**

Run:

```bash
cd other/figma-ui
npm test -- desktop-model
```

Expected: pass.

**Step 5: Commit**

```bash
git add other/figma-ui/src/app/desktop-model.mjs other/figma-ui/src/app/desktop-model.test.mjs
git commit -m "Use daemon latency feedback in desktop model"
```

### Task 8: Full Verification

**Files:**
- No code changes expected.

**Step 1: Run focused Rust tests**

Run:

```bash
cargo test -p rshare-core latency_feedback
cargo test -p rshare-core ipc_contract
cargo test -p rshare-daemon latency_feedback
cargo test -p rshare-cli latency_feedback
```

Expected: all pass.

**Step 2: Run focused frontend tests**

Run:

```bash
cd other/figma-ui
npm test -- desktop-model
```

Expected: pass.

**Step 3: Run workspace smoke test if time allows**

Run:

```bash
cargo test --workspace
```

Expected: pass. If unrelated pre-existing failures appear, capture the failing command and error summary without modifying unrelated files.

**Step 4: Commit verification-only adjustments if any**

Only commit if verification required small fixes:

```bash
git add <changed-files>
git commit -m "Fix latency feedback verification issues"
```

### Task 9: Manual Acceptance Notes

**Files:**
- Modify only if the project already has a suitable manual validation doc: `docs/roadmap.md` or a new dated note under `docs/plans/`

**Step 1: Record manual validation expectations**

After implementation, the operator should be able to:

- run `cargo run -p rshare-cli -- status --detailed`;
- see local input feedback status;
- see transport feedback status;
- trigger a remote latency test from the desktop UI;
- see pending feedback immediately;
- see ACK metrics or timeout without inspecting logs.

**Step 2: Commit documentation only if changed**

```bash
git add docs/plans/<manual-note>.md
git commit -m "Document latency feedback validation"
```

## Execution Handoff

Plan complete and saved to `docs/plans/2026-05-22-low-latency-feedback-implementation-plan.md`. Two execution options:

**1. Subagent-Driven (this session)** - Dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Parallel Session (separate)** - Open a new session with `superpowers:executing-plans`, batch execution with checkpoints.

Which approach?
