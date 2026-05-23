# Low-Latency Local Feedback Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add daemon-owned low-latency local feedback for keyboard, mouse, and gamepad activity while keeping QUIC as the peer transport and localhost WebSocket as the UI event stream.

**Architecture:** Extend the existing additive IPC model in `rshare-core`, aggregate gamepad-aware local feedback in `rshare-daemon`, and render the daemon summary in CLI and desktop UI. The desktop UI continues to merge `ws://127.0.0.1:27436/local-controls` events for instant visual updates, but daemon `latency_feedback` remains the product truth.

**Tech Stack:** Rust 2021, serde JSON IPC, Tokio daemon, Tauri/React TypeScript UI, Node `node:test`, Vite.

---

### Task 1: Add Gamepad Fields To Local Input Feedback IPC

**Files:**
- Modify: `crates/rshare-core/src/ipc.rs`
- Test: `crates/rshare-core/tests/ipc_contract.rs`

**Step 1: Write the failing IPC round-trip assertions**

In `crates/rshare-core/tests/ipc_contract.rs`, extend `latency_feedback_status_response_round_trips_populated_payload()` after the existing local input fields:

```rust
snapshot
    .latency_feedback
    .local_input
    .latest_gamepad_event_ms = Some(115);
snapshot.latency_feedback.local_input.latest_gamepad_id = Some(0);
snapshot
    .latency_feedback
    .local_input
    .latest_gamepad_event_kind = Some("state".to_string());
snapshot
    .latency_feedback
    .local_input
    .latest_gamepad_button = Some("South pressed".to_string());
snapshot
    .latency_feedback
    .local_input
    .latest_gamepad_axis = Some("left_stick".to_string());
```

Add assertions after decode:

```rust
let DaemonResponse::Status(decoded_snapshot) = &decoded else {
    panic!("expected status response");
};
assert_eq!(
    decoded_snapshot
        .latency_feedback
        .local_input
        .latest_gamepad_event_ms,
    Some(115)
);
assert_eq!(
    decoded_snapshot.latency_feedback.local_input.latest_gamepad_id,
    Some(0)
);
assert_eq!(
    decoded_snapshot
        .latency_feedback
        .local_input
        .latest_gamepad_event_kind
        .as_deref(),
    Some("state")
);
assert_eq!(
    decoded_snapshot
        .latency_feedback
        .local_input
        .latest_gamepad_button
        .as_deref(),
    Some("South pressed")
);
assert_eq!(
    decoded_snapshot
        .latency_feedback
        .local_input
        .latest_gamepad_axis
        .as_deref(),
    Some("left_stick")
);
```

**Step 2: Run the focused failing test**

Run:

```powershell
cargo test -p rshare-core latency_feedback_status_response_round_trips_populated_payload
```

Expected: FAIL because the new `LocalInputFeedback` fields do not exist.

**Step 3: Add the IPC fields**

In `crates/rshare-core/src/ipc.rs`, add these fields to `LocalInputFeedback` after `latest_mouse_event_ms`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub latest_gamepad_event_ms: Option<u64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub latest_gamepad_id: Option<u8>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub latest_gamepad_event_kind: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub latest_gamepad_button: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub latest_gamepad_axis: Option<String>,
```

Update `impl Default for LocalInputFeedback` with:

```rust
latest_gamepad_event_ms: None,
latest_gamepad_id: None,
latest_gamepad_event_kind: None,
latest_gamepad_button: None,
latest_gamepad_axis: None,
```

**Step 4: Run the focused test**

Run:

```powershell
cargo test -p rshare-core latency_feedback_status_response_round_trips_populated_payload
```

Expected: PASS.

**Step 5: Run IPC compatibility tests**

Run:

```powershell
cargo test -p rshare-core latency_feedback
cargo test -p rshare-core ipc_contract
```

Expected: PASS. `latency_feedback_defaults_to_safe_unavailable_state` proves old snapshots still default safely.

**Step 6: Commit**

```powershell
git add crates\rshare-core\src\ipc.rs crates\rshare-core\tests\ipc_contract.rs
git commit -m "Add gamepad fields to local latency feedback IPC"
```

### Task 2: Aggregate Gamepad Events In Daemon Local Feedback

**Files:**
- Modify: `apps/rshare-daemon/src/main.rs`

**Step 1: Add failing daemon tests**

Add these tests near the existing `local_input_feedback_*` tests in `apps/rshare-daemon/src/main.rs`:

```rust
#[test]
fn local_input_feedback_uses_latest_gamepad_event() {
    let mut state = test_daemon_state();
    state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);
    let mut gamepad = rshare_input::GamepadState::neutral(0, 1, timestamp_ms_now());
    gamepad.buttons.push(rshare_input::GamepadButtonState {
        button: rshare_input::GamepadButton::South,
        pressed: true,
        value: 1.0,
    });

    let event = state.record_local_input_event(&rshare_input::InputEvent::gamepad_state(gamepad));
    let feedback = state.local_input_feedback();

    assert_eq!(feedback.status, LatencyFeedbackStatus::Healthy);
    assert_eq!(feedback.event_count, 1);
    assert_eq!(feedback.latest_sequence, Some(event.sequence));
    assert_eq!(feedback.latest_event_ms, Some(event.timestamp_ms));
    assert_eq!(feedback.latest_gamepad_event_ms, Some(event.timestamp_ms));
    assert_eq!(feedback.latest_gamepad_id, Some(0));
    assert_eq!(feedback.latest_gamepad_event_kind.as_deref(), Some("state"));
    assert!(feedback.latest_gamepad_button.as_deref().is_some_and(|value| {
        value.contains("South")
    }));
}

#[test]
fn local_input_feedback_event_count_includes_gamepads() {
    let mut state = test_daemon_state();
    state.backend_state.selected_mode = Some(ResolvedInputMode::Portable);
    state.local_controls.keyboard.event_count = 2;
    state.local_controls.mouse.event_count = 3;
    state.local_controls.gamepads.push(rshare_core::LocalGamepadState {
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
        event_count: 4,
        button_event_count: 0,
        button_press_count: 0,
        button_release_count: 0,
        axis_event_count: 0,
        trigger_event_count: 0,
        last_axis: None,
        last_seen_ms: 0,
    });

    let feedback = state.local_input_feedback();

    assert_eq!(feedback.event_count, 9);
}
```

If the exact `GamepadButtonState` field names differ, use the existing gamepad tests around `record_local_input_event(&InputEvent::gamepad_state(...))` as the local source of truth.

**Step 2: Run the failing daemon tests**

Run:

```powershell
cargo test -p rshare-daemon local_input_feedback_uses_latest_gamepad_event
cargo test -p rshare-daemon local_input_feedback_event_count_includes_gamepads
```

Expected: FAIL because daemon feedback ignores gamepad events and counts.

**Step 3: Add gamepad event count helper**

In `apps/rshare-daemon/src/main.rs`, near `DaemonState::local_input_feedback()`, add:

```rust
fn local_gamepad_event_count(snapshot: &LocalControlDeviceSnapshot) -> u64 {
    snapshot
        .gamepads
        .iter()
        .fold(0_u64, |sum, gamepad| sum.saturating_add(gamepad.event_count))
}
```

Update the local input `event_count` calculation:

```rust
let event_count = self
    .local_controls
    .keyboard
    .event_count
    .saturating_add(self.local_controls.mouse.event_count)
    .saturating_add(local_gamepad_event_count(&self.local_controls));
```

**Step 4: Include gamepad events in local feedback**

Change `is_eligible_local_input_feedback_event()`:

```rust
matches!(
    event.device_kind,
    LocalInputDeviceKind::Keyboard
        | LocalInputDeviceKind::Mouse
        | LocalInputDeviceKind::Gamepad
) && !event.payload.contains_key("remote_device_id")
    && !event.payload.contains_key("origin_event_device_id")
    && event.capture_path.as_deref() != Some("remote-daemon")
```

In `local_input_feedback()`, add:

```rust
let latest_gamepad = self
    .local_controls
    .recent_events
    .iter()
    .filter(|event| {
        is_eligible_local_input_feedback_event(event)
            && event.device_kind == LocalInputDeviceKind::Gamepad
    })
    .max_by_key(|event| (event.timestamp_ms, event.sequence));
```

Then populate the new fields:

```rust
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
```

Make sure the `Unavailable` return path still includes the aggregate `event_count` and uses `..LocalInputFeedback::default()`.

**Step 5: Run focused daemon tests**

Run:

```powershell
cargo test -p rshare-daemon local_input_feedback
```

Expected: PASS.

**Step 6: Commit**

```powershell
git add apps\rshare-daemon\src\main.rs
git commit -m "Include gamepad activity in daemon local feedback"
```

### Task 3: Show Gamepad Details In CLI Latency Feedback

**Files:**
- Modify: `apps/rshare-cli/src/commands/status.rs`

**Step 1: Add failing CLI formatting test**

Add this test in the existing `#[cfg(test)] mod tests` in `apps/rshare-cli/src/commands/status.rs`:

```rust
#[test]
fn latency_feedback_lines_include_gamepad_local_details() {
    let feedback = rshare_core::LatencyFeedbackSnapshot {
        local_input: rshare_core::LocalInputFeedback {
            status: rshare_core::LatencyFeedbackStatus::Healthy,
            event_count: 4,
            latest_sequence: Some(9),
            latest_event_ms: Some(1_200),
            latest_keyboard_event_ms: None,
            latest_mouse_event_ms: None,
            latest_gamepad_event_ms: Some(1_200),
            latest_gamepad_id: Some(0),
            latest_gamepad_event_kind: Some("state".to_string()),
            latest_gamepad_button: Some("South pressed".to_string()),
            latest_gamepad_axis: Some("left_stick".to_string()),
            capture_path: None,
        },
        ..rshare_core::LatencyFeedbackSnapshot::default()
    };

    let lines = latency_feedback_lines(&feedback);
    let local = lines
        .iter()
        .find(|(key, _)| key == "Local Input")
        .map(|(_, value)| value)
        .unwrap();

    assert!(local.contains("gamepad 0"));
    assert!(local.contains("South pressed"));
    assert!(local.contains("left_stick"));
}
```

**Step 2: Run the failing test**

Run:

```powershell
cargo test -p rshare-cli latency_feedback_lines_include_gamepad_local_details
```

Expected: FAIL because `local_input_latency_feedback_value()` does not print gamepad details.

**Step 3: Add gamepad details to local input formatting**

In `local_input_latency_feedback_value()`, after the sequence detail, add:

```rust
if let Some(gamepad_id) = feedback.latest_gamepad_id {
    parts.push(format!("gamepad {gamepad_id}"));
}
if let Some(kind) = non_empty(feedback.latest_gamepad_event_kind.as_deref()) {
    parts.push(format!("gamepad {kind}"));
}
if let Some(button) = non_empty(feedback.latest_gamepad_button.as_deref()) {
    parts.push(format!("button {button}"));
}
if let Some(axis) = non_empty(feedback.latest_gamepad_axis.as_deref()) {
    parts.push(format!("axis {axis}"));
}
```

**Step 4: Run CLI tests**

Run:

```powershell
cargo test -p rshare-cli latency_feedback
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add apps\rshare-cli\src\commands\status.rs
git commit -m "Show gamepad details in CLI latency feedback"
```

### Task 4: Add Frontend Model Rows For Local Latency Feedback

**Files:**
- Modify: `other/figma-ui/src/app/desktop-model.mjs`
- Modify: `other/figma-ui/src/app/desktop-model.test.mjs`

**Step 1: Add failing model tests**

Import the new helper in `desktop-model.test.mjs`:

```javascript
import {
  buildLocalLatencyFeedbackRows,
  // existing imports...
} from "./desktop-model.mjs";
```

Add tests near the latency summary tests:

```javascript
test("buildLocalLatencyFeedbackRows maps daemon keyboard mouse and gamepad feedback", () => {
  const rows = buildLocalLatencyFeedbackRows({
    local_input: {
      status: "Healthy",
      event_count: 7,
      latest_sequence: 12,
      latest_keyboard_event_ms: 1000,
      latest_mouse_event_ms: 1100,
      latest_gamepad_event_ms: 1200,
      latest_gamepad_id: 0,
      latest_gamepad_event_kind: "state",
      latest_gamepad_button: "South pressed",
      latest_gamepad_axis: "left_stick",
    },
    transport: {
      status: "Healthy",
      transport: "quic",
      datagram_available: true,
      realtime_degraded: false,
      rtt_ms: 12,
    },
  });

  assert.deepEqual(
    rows.map((row) => row.key),
    ["keyboard", "mouse", "gamepad", "transport"],
  );
  assert.equal(rows.find((row) => row.key === "gamepad").state, "pass");
  assert.match(rows.find((row) => row.key === "gamepad").detail, /South pressed/);
  assert.match(rows.find((row) => row.key === "transport").detail, /12 ms RTT/);
});

test("buildLocalLatencyFeedbackRows marks missing gamepad as idle without breaking input status", () => {
  const rows = buildLocalLatencyFeedbackRows({
    local_input: {
      status: "Idle",
      event_count: 0,
    },
    transport: {
      status: "Unavailable",
      transport: "quic",
      datagram_available: false,
      realtime_degraded: true,
    },
  });

  assert.equal(rows.find((row) => row.key === "gamepad").state, "idle");
  assert.match(rows.find((row) => row.key === "gamepad").detail, /waiting/i);
});
```

**Step 2: Run the failing frontend test**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/desktop-model.test.mjs
Pop-Location
```

Expected: FAIL because `buildLocalLatencyFeedbackRows()` does not exist.

**Step 3: Implement the model helper**

In `other/figma-ui/src/app/desktop-model.mjs`, add:

```javascript
const LOCAL_FEEDBACK_LABELS = Object.freeze({
  keyboard: "键盘",
  mouse: "鼠标",
  gamepad: "手柄",
  transport: "QUIC",
});

function latencyStatusState(status) {
  switch (String(status ?? "").toLowerCase()) {
    case "healthy":
      return "pass";
    case "degraded":
    case "pending":
      return "warn";
    case "timeout":
    case "unavailable":
      return "block";
    case "idle":
    default:
      return "idle";
  }
}

function eventAgeDetail(timestampMs, fallback) {
  const value = numberOrNull(timestampMs);
  if (value == null || value <= 0) {
    return fallback;
  }
  return `latest ${value} ms`;
}

export function buildLocalLatencyFeedbackRows(feedback) {
  const local = feedback?.local_input ?? {};
  const transport = feedback?.transport ?? {};
  const inputState = latencyStatusState(local.status);
  const gamepadParts = [
    local.latest_gamepad_id == null ? null : `gamepad ${local.latest_gamepad_id}`,
    local.latest_gamepad_event_kind,
    local.latest_gamepad_button,
    local.latest_gamepad_axis,
  ].filter(Boolean);
  const transportParts = [
    transport.transport ?? "quic",
    transport.rtt_ms == null ? null : `${Number(transport.rtt_ms)} ms RTT`,
    transport.datagram_available ? "datagram" : "no datagram",
  ].filter(Boolean);

  return [
    {
      key: "keyboard",
      label: LOCAL_FEEDBACK_LABELS.keyboard,
      state: inputState,
      metric: String(local.event_count ?? 0),
      detail: eventAgeDetail(local.latest_keyboard_event_ms, "waiting for keyboard"),
    },
    {
      key: "mouse",
      label: LOCAL_FEEDBACK_LABELS.mouse,
      state: inputState,
      metric: String(local.event_count ?? 0),
      detail: eventAgeDetail(local.latest_mouse_event_ms, "waiting for mouse"),
    },
    {
      key: "gamepad",
      label: LOCAL_FEEDBACK_LABELS.gamepad,
      state: local.latest_gamepad_event_ms == null ? "idle" : inputState,
      metric: String(local.event_count ?? 0),
      detail: gamepadParts.length
        ? gamepadParts.join(", ")
        : eventAgeDetail(local.latest_gamepad_event_ms, "waiting for gamepad"),
    },
    {
      key: "transport",
      label: LOCAL_FEEDBACK_LABELS.transport,
      state: latencyStatusState(transport.status),
      metric: transport.status ?? "Unavailable",
      detail: transportParts.join(", "),
    },
  ];
}
```

If `numberOrNull()` already exists earlier in the file, reuse it. If `eventAgeDetail` naming conflicts, choose a local unique name.

**Step 4: Run frontend model tests**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/desktop-model.test.mjs
Pop-Location
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add other\figma-ui\src\app\desktop-model.mjs other\figma-ui\src\app\desktop-model.test.mjs
git commit -m "Model local latency feedback rows"
```

### Task 5: Render Local Feedback Strip On Devices Page

**Files:**
- Modify: `other/figma-ui/src/app/App.tsx`

**Step 1: Run a pre-refactor build**

Run:

```powershell
Push-Location other\figma-ui
npm run build
Pop-Location
```

Expected: PASS before UI wiring.

**Step 2: Import the model helper**

In `other/figma-ui/src/app/App.tsx`, extend the `desktop-model.mjs` import:

```ts
buildLocalLatencyFeedbackRows,
```

**Step 3: Add a compact strip component**

Add near other small Devices page components:

```tsx
function LocalLatencyFeedbackStrip({
  rows,
  theme,
}: {
  rows: Array<{
    key: string;
    label: string;
    state: string;
    metric: string;
    detail: string;
  }>;
  theme: typeof FIGMA_DESKTOP_THEME;
}) {
  const tone = (state: string) => {
    switch (state) {
      case "pass":
        return theme.success;
      case "warn":
        return theme.warning;
      case "block":
        return theme.danger;
      default:
        return theme.textMuted;
    }
  };

  return (
    <div className="grid shrink-0 grid-cols-2 gap-2 lg:grid-cols-4">
      {rows.map((row) => (
        <div
          key={row.key}
          className="min-w-0 rounded-md px-3 py-2"
          style={{
            border: `1px solid ${theme.border}`,
            background: "rgba(255,255,255,0.035)",
          }}
        >
          <div className="flex items-center justify-between gap-2">
            <span className="truncate text-xs" style={{ color: theme.textMuted }}>
              {row.label}
            </span>
            <span
              className="h-1.5 w-1.5 shrink-0 rounded-full"
              style={{ background: tone(row.state) }}
            />
          </div>
          <div className="mt-1 truncate text-sm font-semibold" style={{ color: theme.text }}>
            {row.metric}
          </div>
          <div className="mt-1 truncate text-[11px]" style={{ color: theme.textMuted }} title={row.detail}>
            {row.detail}
          </div>
        </div>
      ))}
    </div>
  );
}
```

Use the existing theme field names. If `theme.warning` or `theme.danger` do not exist, use the existing warning/danger color names from `FIGMA_DESKTOP_THEME`.

**Step 4: Render the strip in `DevicesPageWithLocalControls`**

Inside `DevicesPageWithLocalControls`, after `latencyFeedbackSnapshot` is built, add:

```ts
const localLatencyRows = buildLocalLatencyFeedbackRows(latencyFeedback);
```

Render near the top of the main device panel, before the selected device content:

```tsx
<LocalLatencyFeedbackStrip rows={localLatencyRows} theme={theme} />
```

Keep it outside nested cards. It should be a compact full-width row inside the existing page layout.

**Step 5: Run build**

Run:

```powershell
Push-Location other\figma-ui
npm run build
Pop-Location
```

Expected: PASS. Fix any TypeScript import or theme field errors.

**Step 6: Commit**

```powershell
git add other\figma-ui\src\app\App.tsx
git commit -m "Render local latency feedback strip"
```

### Task 6: Full Verification And Manual Notes

**Files:**
- Modify only if needed: `docs/plans/2026-05-22-low-latency-feedback-manual-validation.md`

**Step 1: Run focused Rust tests**

Run:

```powershell
cargo test -p rshare-core latency_feedback
cargo test -p rshare-core ipc_contract
cargo test -p rshare-daemon local_input_feedback
cargo test -p rshare-cli latency_feedback
```

Expected: PASS.

**Step 2: Run frontend tests**

Run:

```powershell
Push-Location other\figma-ui
npm test
Pop-Location
```

Expected: PASS.

**Step 3: Run frontend build**

Run:

```powershell
Push-Location other\figma-ui
npm run build
Pop-Location
```

Expected: PASS.

**Step 4: Optional workspace smoke**

Run if time allows:

```powershell
cargo test --workspace
```

Expected: PASS. If unrelated pre-existing failures appear, record the failing command and error summary without editing unrelated code.

**Step 5: Update manual validation only if the UI behavior changed enough**

If the Devices page feedback strip needs manual acceptance notes, append to `docs/plans/2026-05-22-low-latency-feedback-manual-validation.md`:

```markdown
## Local Feedback Strip

Expected:

- Keyboard, mouse, and gamepad rows update visually from `ws://127.0.0.1:27436/local-controls` events.
- The status labels remain consistent with `status --detailed`.
- Peer input traffic remains QUIC; WebSocket is localhost-only UI event delivery.
```

**Step 6: Commit verification fixes or docs**

Only commit if fixes or doc updates were needed:

```powershell
git add <changed-files>
git commit -m "Document local feedback strip validation"
```

## Execution Handoff

Plan complete and saved to `docs/plans/2026-05-23-low-latency-local-feedback-implementation-plan.md`. Two execution options:

**1. Subagent-Driven (this session)** - Dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Parallel Session (separate)** - Open a new session with `superpowers:executing-plans`, batch execution with checkpoints.

Which approach?
