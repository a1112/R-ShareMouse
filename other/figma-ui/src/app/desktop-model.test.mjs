import test from "node:test";
import assert from "node:assert/strict";

import {
  buildDesktopViewModel,
  buildCapabilityOverview,
  buildDeviceGalleryItems,
  buildDeviceTypeSummaries,
  buildEndpointAcceptance,
  buildEndpointInjectSummary,
  buildLocalControlsViewModel,
  buildRemoteLatencySummary,
  endpointEventToLocalControlEvent,
  updateRememberedLayoutFromVisibleMonitors,
} from "./desktop-model.mjs";

test("buildDesktopViewModel returns an offline local-only layout when daemon is unavailable", () => {
  const model = buildDesktopViewModel({ status: null, devices: [] });

  assert.equal(model.service.online, false);
  assert.equal(model.layout.devices.length, 1);
  assert.equal(model.layout.devices[0].kind, "local");
  assert.equal(model.layout.monitors.length, 1);
  assert.equal(model.devices.length, 0);
  assert.equal(model.settings.localDevice.name, "本机");
  assert.equal(model.acceptance.daemonOnline, false);
  assert.equal(model.acceptance.backgroundReady, false);
  assert.equal(model.acceptance.dualMachineReady, false);
  assert.equal(model.acceptance.nextStep, "启动守护进程后进行双机实机验收");
});

test("buildDesktopViewModel exposes daemon capability registry when present", () => {
  const model = buildDesktopViewModel({
    status: {
      device_id: "local-1",
      device_name: "Local",
      hostname: "local-host",
      bind_address: "0.0.0.0:27431",
      discovery_port: 27432,
      healthy: true,
    },
    devices: [],
    capabilities: {
      local_device_id: "local-1",
      generated_at_ms: 42,
      devices: [
        {
          device_id: "local-1",
          device_name: "Local",
          hostname: "local-host",
          connected: true,
          capabilities: [
            {
              kind: "Input",
              state: "Available",
              health_reason: null,
              details: { mode: "Portable" },
            },
            {
              kind: "UsbReceiver",
              state: "Unavailable",
              health_reason: "receiver-side virtual USB bus not implemented",
              details: {},
            },
          ],
        },
      ],
    },
  });

  assert.equal(model.capabilities.available, true);
  assert.equal(model.capabilities.localDeviceId, "local-1");
  assert.equal(model.capabilities.devices[0].capabilities[0].label, "输入");
  assert.equal(model.capabilities.devices[0].capabilities[0].stateLabel, "可用");
  assert.equal(
    model.capabilities.devices[0].capabilities[1].reason,
    "receiver-side virtual USB bus not implemented",
  );
});

test("buildCapabilityOverview falls back cleanly when registry is missing", () => {
  const overview = buildCapabilityOverview(null);

  assert.equal(overview.available, false);
  assert.deepEqual(overview.devices, []);
});

test("buildLocalControlsViewModel maps keyboard mouse gamepad and display panels", () => {
  const model = buildLocalControlsViewModel(
    {
      keyboard: {
        detected: true,
        pressed_keys: ["ShiftLeft"],
        last_key: "ShiftLeft",
        event_count: 3,
        capture_source: "RDev",
      },
      mouse: {
        detected: true,
        x: 40,
        y: 80,
        pressed_buttons: ["Left"],
        wheel_delta_x: 0,
        wheel_delta_y: -1,
        event_count: 9,
      },
      gamepads: [
        {
          gamepad_id: 0,
          name: "Xbox Controller",
          connected: true,
          buttons: [{ button: "South", pressed: true }],
          pressed_buttons: ["South"],
          last_button: "South pressed",
          left_stick_x: 1200,
          left_stick_y: -2400,
          right_stick_x: 0,
          right_stick_y: 0,
          left_trigger: 123,
          right_trigger: 456,
          event_count: 7,
          button_event_count: 2,
          button_press_count: 2,
          button_release_count: 0,
          axis_event_count: 1,
          trigger_event_count: 1,
          last_axis: "left_stick",
        },
      ],
      display: {
        display_count: 2,
        primary_width: 2560,
        primary_height: 1440,
        layout_width: 4480,
        layout_height: 1440,
      },
      capture_backend: { mode: "WindowsNative", health: "Healthy" },
      inject_backend: { mode: "WindowsNative", health: "Healthy" },
      privilege_state: "UnlockedDesktop",
      virtual_gamepad: {
        status: "not_implemented",
        detail: "Virtual HID not implemented",
      },
      recent_events: [
        {
          device_kind: "Mouse",
          summary: "Mouse move 1, 1",
          source: "Hardware",
        },
        {
          device_kind: "Keyboard",
          summary: "Injected ShiftLeft release",
          source: "InjectedLoopback",
        },
      ],
    },
    { confirmingInputTest: "keyboard" },
  );

  assert.equal(model.available, true);
  assert.equal(model.keyboard.status, "capturing");
  assert.equal(model.keyboard.testLabel, "confirm keyboard injection");
  assert.deepEqual(model.mouse.position, { x: 40, y: 80 });
  assert.equal(model.gamepad.status, "gilrs-connected");
  assert.equal(model.gamepad.virtualDetail, "Virtual HID not implemented");
  assert.deepEqual(model.gamepad.pressedButtons, ["South"]);
  assert.equal(model.gamepad.stats.buttonPresses, 2);
  assert.equal(model.gamepad.sticks.left.x, 1200);
  assert.equal(model.gamepad.triggers.right, 456);
  assert.equal(model.display.count, 2);
  assert.equal(model.backend.capture, "WindowsNative Healthy");
  assert.equal(model.latestEvent.deviceKind, "Keyboard");
  assert.equal(model.latestEvent.injectedLoopback, true);
  assert.equal(model.composite.label, "综合");
  assert.equal(model.composite.eventCount, 19);
  assert.equal(model.composite.live, true);
});

test("buildLocalControlsViewModel reports old daemon or unavailable daemon safely", () => {
  const unavailable = buildLocalControlsViewModel(null, { error: "unsupported request" });
  assert.equal(unavailable.available, false);
  assert.equal(unavailable.error, "unsupported request");
  assert.equal(unavailable.keyboard.status, "missing");
  assert.equal(unavailable.mouse.testLabel, "mouse injection test");
  assert.equal(unavailable.gamepad.virtualStatus, "not_implemented");
  assert.equal(unavailable.display.primary.width, 0);
  assert.equal(unavailable.latestEvent, null);
});

test("endpointEventToLocalControlEvent preserves remote endpoint and physical device identity", () => {
  const event = endpointEventToLocalControlEvent({
    event_id: 9,
    sequence: 9,
    timestamp_ms: 1234,
    endpoint_id: "remote-1",
    origin_endpoint_id: "remote-1",
    device: {
      device_id: "keyboard-hid-1",
      instance_id: "instance-1",
      display_name: "Remote Mechanical Keyboard",
      kind: "Keyboard",
      attribution: "Exact",
    },
    direction: "Observed",
    source: "RemoteMirror",
    kind: "Keyboard",
    payload: {
      kind: "Keyboard",
      data: {
        key: "A",
        state: "Pressed",
      },
    },
    correlation_id: null,
  });

  assert.equal(event.device_kind, "Keyboard");
  assert.equal(event.event_kind, "key");
  assert.equal(event.device_id, "remote-1");
  assert.equal(event.device_instance_id, "instance-1");
  assert.equal(event.source, "Hardware");
  assert.equal(event.payload.key, "A");
  assert.equal(event.payload.state, "Pressed");
  assert.equal(event.payload.device_id, "keyboard-hid-1");
  assert.equal(event.payload.remote_device_id, "remote-1");
  assert.equal(event.payload.device_display_name, "Remote Mechanical Keyboard");
});

test("buildEndpointAcceptance reports dual-machine event mirror and inject readiness", () => {
  const acceptance = buildEndpointAcceptance(
    {
      capture_backend: { active: true },
      inject_backend: { active: true },
      recent_events: [
        {
          sequence: 1,
          device_kind: "Keyboard",
          event_kind: "key",
          summary: "Local key A",
          device_id: null,
          source: "Hardware",
          payload: { key: "A", state: "Pressed" },
        },
        {
          sequence: 2,
          device_kind: "Mouse",
          event_kind: "move",
          summary: "Remote mouse move",
          device_id: "remote-1",
          source: "Hardware",
          payload: { remote_device_id: "remote-1" },
        },
        {
          sequence: 3,
          device_kind: "Keyboard",
          event_kind: "key",
          summary: "Injected ShiftLeft release",
          device_id: "remote-1",
          source: "InjectedLoopback",
          payload: { remote_device_id: "remote-1", correlation_id: "inject-1" },
        },
      ],
    },
    [{ id: "remote-1", name: "Remote", connected: true }],
    { status: "Success", message: "ok" },
  );

  assert.equal(acceptance.ready, true);
  assert.equal(acceptance.remoteEventCount, 2);
  assert.equal(acceptance.remoteInjectedEventCount, 1);
  assert.deepEqual(
    acceptance.checks.map((check) => [check.key, check.state]),
    [
      ["local-events", "pass"],
      ["remote-mirror", "pass"],
      ["remote-inject", "pass"],
      ["endpoint-backend", "pass"],
    ],
  );
});

test("buildEndpointAcceptance keeps remote inject pending until a test or loopback event exists", () => {
  const acceptance = buildEndpointAcceptance(
    {
      capture_backend: { active: true },
      inject_backend: { active: true },
      recent_events: [],
    },
    [{ id: "remote-1", name: "Remote", connected: true }],
    null,
  );

  assert.equal(acceptance.ready, false);
  assert.deepEqual(
    acceptance.checks.map((check) => [check.key, check.state]),
    [
      ["local-events", "warn"],
      ["remote-mirror", "warn"],
      ["remote-inject", "warn"],
      ["endpoint-backend", "pass"],
    ],
  );
});

test("buildEndpointInjectSummary reports scoped latency for the selected device page", () => {
  const summary = buildEndpointInjectSummary(
    [
      { accepted: true, elapsed_ms: 12 },
      { accepted: true, elapsed_ms: 18 },
    ],
    { kind: "keyboard", targetId: "remote-1" },
  );

  assert.equal(summary.status, "Success");
  assert.equal(summary.kind, "keyboard");
  assert.equal(summary.targetId, "remote-1");
  assert.equal(summary.successCount, 2);
  assert.equal(summary.totalCount, 2);
  assert.equal(summary.averageElapsedMs, 15);
  assert.equal(summary.maxElapsedMs, 18);
  assert.equal(summary.message, "Endpoint 注入完成：2/2 成功，平均 15 ms，最大 18 ms");
});

test("buildRemoteLatencySummary extracts the latest RTT for a selected remote device", () => {
  const summary = buildRemoteLatencySummary(
    {
      recent_events: [
        {
          sequence: 10,
          timestamp_ms: 1000,
          device_kind: "Backend",
          event_kind: "latency_probe_ack",
          summary: "Latency to old: 44 ms RTT / ~22 ms one-way",
          device_id: "remote-1",
          source: "System",
          payload: {
            target_device_id: "remote-1",
            latency_ms: "44",
            estimated_one_way_ms: "22",
            raw_round_trip_ms: "50",
            remote_processing_ms: "6",
            direction: "origin_to_endpoint",
          },
        },
        {
          sequence: 11,
          timestamp_ms: 2000,
          device_kind: "Backend",
          event_kind: "latency_endpoint_switch_ack",
          summary: "Endpoint-side latency to remote: 30 ms RTT / ~15 ms one-way",
          device_id: "remote-1",
          source: "System",
          payload: {
            origin_device_id: "remote-1",
            latency_ms: "30",
            estimated_one_way_ms: "15",
            raw_round_trip_ms: "33",
            remote_processing_ms: "3",
            direction: "endpoint_to_origin",
          },
        },
      ],
    },
    "remote-1",
  );

  assert.equal(summary.state, "pass");
  assert.equal(summary.networkRoundTripMs, 30);
  assert.equal(summary.estimatedOneWayMs, 15);
  assert.equal(summary.rawRoundTripMs, 33);
  assert.equal(summary.remoteProcessingMs, 3);
  assert.equal(summary.direction, "endpoint_to_origin");
});

test("buildRemoteLatencySummary reports pending probe when ACK has not arrived", () => {
  const summary = buildRemoteLatencySummary(
    {
      recent_events: [
        {
          sequence: 5,
          timestamp_ms: 1000,
          device_kind: "Backend",
          event_kind: "latency_endpoint_probe_sent",
          summary: "Dual-end latency probe sent",
          device_id: "remote-2",
          source: "System",
          payload: {
            target_device_id: "remote-2",
          },
        },
      ],
    },
    "remote-2",
  );

  assert.equal(summary.state, "pending");
  assert.equal(summary.networkRoundTripMs, null);
  assert.equal(summary.message, "等待远端 latency ACK");
});

test("buildDeviceTypeSummaries keeps device tabs compact and unitless", () => {
  const tabs = buildDeviceTypeSummaries({
    keyboard: 7,
    mouse: 4,
    gamepad: 1,
    display: 1,
    audio: 15,
    remote: 2,
  });

  assert.deepEqual(
    tabs.map((tab) => [tab.kind, tab.title, tab.detail]),
    [
      ["keyboard", "键盘", "7"],
      ["mouse", "鼠标", "4"],
      ["gamepad", "手柄", "1"],
      ["display", "显示", "1"],
      ["audio", "音频", "15"],
      ["remote", "远端", "2"],
    ],
  );
});

test("buildDeviceGalleryItems lays local and remote devices onto a free canvas", () => {
  const items = buildDeviceGalleryItems(
    {
      keyboard: { detected: true, event_count: 12 },
      mouse: { detected: true, event_count: 20 },
      keyboard_devices: [
        { id: "kbd-1", name: "Keyboard A", connected: true },
        { id: "kbd-2", name: "Keyboard B", connected: true },
      ],
      mouse_devices: [{ id: "mouse-1", name: "Mouse A", connected: true }],
      gamepads: [{ gamepad_id: 0, name: "Pad", connected: true, event_count: 5 }],
      display: {
        display_count: 1,
        primary_width: 2560,
        primary_height: 1440,
        displays: [{ display_id: "primary", width: 2560, height: 1440, primary: true }],
      },
      audio_inputs: [{ id: "mic", name: "Mic", connected: true }],
      audio_outputs: [],
      recent_events: [],
    },
    [{ id: "speaker", name: "Speaker", connected: true }],
    [
      { id: "remote-1", name: "Remote PC", hostname: "remote", connected: false },
      { id: "remote-2", name: "Desk PC", hostname: "desk", connected: true },
    ],
  );

  assert.deepEqual(
    items.map((item) => [item.kind, item.title, item.detail]),
    [
      ["keyboard", "综合键盘", "2 台键盘"],
      ["mouse", "综合鼠标", "1 台鼠标"],
      ["gamepad", "Pad", "手柄"],
      ["display", "主显示", "2560 x 1440"],
      ["audio", "音频矩阵", "2 个端点"],
      ["remote", "Remote PC", "已发现"],
      ["remote", "Desk PC", "已连接"],
    ],
  );
  assert.equal(items[0].x < items[1].x, true);
  assert.equal(items[3].w > items[2].w, true);
});

test("buildDeviceGalleryItems centers the physical device layout around the display", () => {
  const items = buildDeviceGalleryItems({
    keyboard: { detected: true, event_count: 12 },
    mouse: { detected: true, event_count: 20 },
    keyboard_devices: [{ id: "kbd-1", name: "Keyboard A", connected: true }],
    mouse_devices: [{ id: "mouse-1", name: "Mouse A", connected: true }],
    gamepads: [{ gamepad_id: 0, name: "Pad", connected: true, event_count: 5 }],
    display: {
      display_count: 1,
      primary_width: 2560,
      primary_height: 1440,
      displays: [{ display_id: "primary", width: 2560, height: 1440, primary: true }],
    },
    audio_inputs: [{ id: "mic", name: "Mic", connected: true }],
    audio_outputs: [],
    recent_events: [],
  });

  const display = items.find((item) => item.kind === "display");
  const keyboard = items.find((item) => item.kind === "keyboard");
  const mouse = items.find((item) => item.kind === "mouse");
  const gamepad = items.find((item) => item.kind === "gamepad");
  const audio = items.find((item) => item.kind === "audio");

  assert.equal(display.shape, "monitor");
  assert.equal(keyboard.shape, "keyboard");
  assert.equal(mouse.shape, "mouse");
  assert.equal(gamepad.shape, "gamepad");
  assert.equal(audio.shape, "speaker");
  assert.equal(display.x, 620);
  assert.equal(display.y, 260);
  assert.equal(keyboard.y > display.y + display.h, true);
  assert.equal(mouse.x > display.x + display.w, true);
  assert.equal(gamepad.x < display.x, true);
});

test("buildDeviceGalleryItems assigns Live2D hardware rigs to interactive devices", () => {
  const items = buildDeviceGalleryItems(
    {
      keyboard: { detected: true, event_count: 12 },
      mouse: { detected: true, event_count: 20 },
      keyboard_devices: [{ id: "kbd-1", name: "Keyboard A", connected: true }],
      mouse_devices: [{ id: "mouse-1", name: "Mouse A", connected: true }],
      gamepads: [{ gamepad_id: 0, name: "Pad", connected: true, event_count: 5 }],
      display: {
        display_count: 1,
        primary_width: 2560,
        primary_height: 1440,
        displays: [{ display_id: "primary", width: 2560, height: 1440, primary: true }],
      },
      audio_inputs: [{ id: "mic", name: "Mic", connected: true }],
      audio_outputs: [],
      recent_events: [],
    },
    [{ id: "speaker", name: "Speaker", connected: true }],
    [{ id: "remote-1", name: "Remote PC", hostname: "remote", connected: false }],
  );

  assert.deepEqual(
    items.map((item) => [item.kind, item.rigKind, item.rigVariant]),
    [
      ["keyboard", "keyboard", "default"],
      ["mouse", "mouse", "default"],
      ["gamepad", null, null],
      ["display", null, null],
      ["audio", null, null],
      ["remote", null, null],
    ],
  );
});

test("buildDeviceGalleryItems carries live input activity for physical simulators", () => {
  const items = buildDeviceGalleryItems({
    keyboard: {
      detected: true,
      event_count: 12,
      pressed_keys: ["A", "Char(73)"],
      last_key: "Enter",
    },
    mouse: {
      detected: true,
      event_count: 20,
      pressed_buttons: ["Left"],
      x: 420,
      y: 240,
      display_relative_x: 320,
      display_relative_y: 180,
      wheel_delta_y: -1,
    },
    keyboard_devices: [{ id: "kbd-1", name: "Keyboard A", connected: true }],
    mouse_devices: [{ id: "mouse-1", name: "Mouse A", connected: true }],
    gamepads: [],
    display: {
      display_count: 1,
      primary_width: 2560,
      primary_height: 1440,
      displays: [{ display_id: "primary", width: 2560, height: 1440, primary: true }],
    },
    recent_events: [
      {
        device_kind: "Keyboard",
        event_kind: "key",
        summary: "Key Enter Released",
        payload: { key: "Enter", state: "Released" },
      },
      {
        device_kind: "Mouse",
        event_kind: "button",
        summary: "Mouse button Right Pressed",
        payload: { button: "Right", state: "Pressed" },
      },
    ],
  });

  const keyboard = items.find((item) => item.kind === "keyboard");
  const mouse = items.find((item) => item.kind === "mouse");
  const display = items.find((item) => item.kind === "display");

  assert.deepEqual(keyboard.activity.pressedKeys, ["A", "Char(73)"]);
  assert.equal(keyboard.activity.lastKey, "Enter");
  assert.equal(keyboard.activity.keyboardEvents.length, 1);
  assert.deepEqual(mouse.activity.pressedButtons, ["Left"]);
  assert.ok(mouse.activity.recentButtons.includes("Right"));
  assert.equal(mouse.activity.x, 420);
  assert.equal(mouse.activity.wheelDeltaY, -1);
  assert.equal(display.activity.pointerVisible, true);
  assert.equal(display.activity.pointerX, 320);
  assert.equal(display.activity.pointerY, 180);
});

test("buildDeviceGalleryItems maps mouse pointer to matching display id", () => {
  const items = buildDeviceGalleryItems({
    mouse: {
      detected: true,
      current_display_id: "right",
      display_relative_x: 48,
      display_relative_y: 64,
    },
    display: {
      display_count: 2,
      primary_width: 1920,
      primary_height: 1080,
      displays: [
        { display_id: "primary", width: 1920, height: 1080, primary: true },
        { display_id: "right", width: 2560, height: 1440, primary: false },
      ],
    },
  });

  const primary = items.find((item) => item.id === "gallery-display-primary");
  const right = items.find((item) => item.id === "gallery-display-right");

  assert.equal(primary.activity.pointerVisible, false);
  assert.equal(right.activity.pointerVisible, true);
  assert.equal(right.activity.pointerX, 48);
  assert.equal(right.activity.pointerY, 64);
});

test("buildDeviceGalleryItems ignores stale mouse button events preserved by daemon", () => {
  const items = buildDeviceGalleryItems({
    mouse: {
      detected: true,
      event_count: 100,
      pressed_buttons: [],
    },
    mouse_devices: [{ id: "mouse-1", name: "Mouse A", connected: true }],
    recent_events: [
      {
        device_kind: "Mouse",
        event_kind: "button",
        timestamp_ms: 1000,
        summary: "Mouse button Left Released",
        payload: { button: "Left", state: "Released" },
      },
      {
        device_kind: "Mouse",
        event_kind: "move",
        timestamp_ms: 5000,
        summary: "Mouse move 10, 10",
        payload: { x: "10", y: "10" },
      },
    ],
  });

  const mouse = items.find((item) => item.kind === "mouse");
  assert.deepEqual(mouse.activity.recentButtons, []);
});

test("buildDesktopViewModel maps daemon devices into layout and device cards", () => {
  const payload = {
    status: {
      device_id: "local-1",
      device_name: "Studio PC",
      hostname: "studio",
      bind_address: "192.168.1.10",
      discovery_port: 4242,
      pid: 999,
      discovered_devices: 2,
      connected_devices: 1,
      healthy: true,
      input_mode: "WindowsNative",
      available_backends: ["Portable", "WindowsNative"],
      backend_health: "Healthy",
      privilege_state: "UnlockedDesktop",
      last_backend_error: null,
    },
    devices: [
      {
        id: "remote-1",
        name: "MacBook Pro",
        hostname: "mbp",
        addresses: ["192.168.1.20"],
        connected: true,
        last_seen_secs: 12,
      },
      {
        id: "remote-2",
        name: "Desk Mini",
        hostname: "desk-mini",
        addresses: ["192.168.1.21"],
        connected: false,
        last_seen_secs: 40,
      },
    ],
  };

  const model = buildDesktopViewModel(payload);

  assert.equal(model.service.online, true);
  assert.equal(model.layout.devices.length, 3);
  assert.equal(model.layout.monitors.length, 3);
  assert.deepEqual(
    model.devices.map((device) => ({
      id: device.id,
      connected: device.connected,
      online: device.online,
    })),
    [
      { id: "remote-1", connected: true, online: true },
      { id: "remote-2", connected: false, online: true },
    ],
  );
  assert.equal(model.layout.devices[1].connected, true);
  assert.equal(model.layout.devices[2].connected, false);
  assert.equal(model.settings.localDevice.name, "Studio PC");
  assert.equal(model.settings.inputMode.current, "WindowsNative");
});

test("buildDesktopViewModel preserves connection status consistently across pages", () => {
  const payload = {
    status: {
      device_id: "local-1",
      device_name: "Studio PC",
      hostname: "studio",
      bind_address: "127.0.0.1",
      discovery_port: 4242,
      pid: 999,
      discovered_devices: 1,
      connected_devices: 1,
      healthy: true,
      input_mode: "Portable",
      available_backends: ["Portable"],
      backend_health: {
        Degraded: {
          reason: "PermissionDenied",
        },
      },
      privilege_state: "LockedDesktop",
      last_backend_error: "access denied",
    },
    devices: [
      {
        id: "remote-1",
        name: "Travel Laptop",
        hostname: "travel",
        addresses: ["10.0.0.15"],
        connected: true,
        last_seen_secs: null,
      },
    ],
  };

  const model = buildDesktopViewModel(payload);
  const layoutDevice = model.layout.devices.find((device) => device.id === "remote-1");
  const deviceCard = model.devices.find((device) => device.id === "remote-1");

  assert.equal(layoutDevice?.connected, true);
  assert.equal(deviceCard?.connected, true);
  assert.equal(model.settings.inputMode.health, "Degraded");
  assert.equal(model.settings.inputMode.reason, "PermissionDenied");
  assert.equal(model.settings.privilegeState, "LockedDesktop");
  assert.equal(model.service.error, "access denied");
});

test("buildDesktopViewModel renders daemon visible_layout instead of synthesizing device monitors", () => {
  const payload = {
    status: {
      device_id: "local-1",
      device_name: "Studio PC",
      hostname: "studio",
      bind_address: "127.0.0.1",
      discovery_port: 4242,
      pid: 999,
      discovered_devices: 1,
      connected_devices: 0,
      healthy: true,
    },
    devices: [
      {
        id: "remote-1",
        name: "Remote Workstation",
        hostname: "remote",
        addresses: ["192.168.1.30"],
        connected: false,
        last_seen_secs: 2,
      },
    ],
    layout: {
      version: 1,
      local_device: "local-1",
      nodes: [
        {
          device_id: "local-1",
          displays: [{ display_id: "primary", x: 0, y: 0, width: 1280, height: 720, primary: true }],
        },
        {
          device_id: "offline-1",
          displays: [{ display_id: "primary", x: 1280, y: 0, width: 1920, height: 1080, primary: true }],
        },
        {
          device_id: "remote-1",
          displays: [{ display_id: "primary", x: 3200, y: 0, width: 1024, height: 768, primary: true }],
        },
      ],
      links: [],
    },
    visible_layout: {
      version: 1,
      local_device: "local-1",
      nodes: [
        {
          device_id: "local-1",
          displays: [{ display_id: "primary", x: 0, y: 0, width: 1280, height: 720, primary: true }],
        },
        {
          device_id: "remote-1",
          displays: [{ display_id: "primary", x: 1280, y: 0, width: 1024, height: 768, primary: true }],
        },
      ],
      links: [],
    },
  };

  const model = buildDesktopViewModel(payload);

  assert.deepEqual(model.layout.devices.map((device) => device.id), ["local-1", "remote-1"]);
  assert.equal(model.layout.monitors.length, 2);
  assert.equal(model.layout.monitors.some((monitor) => monitor.deviceId === "offline-1"), false);
  assert.equal(model.layout.monitors[0].resWidth, 1280);
  assert.equal(model.layout.monitors[1].resWidth, 1024);
  assert.equal(model.layout.monitors[1].x, 233.6);
  assert.equal(model.layout.remembered.nodes.length, 3);
});

test("buildDesktopViewModel snaps visible layout monitor groups before rendering", () => {
  const model = buildDesktopViewModel({
    status: {
      device_id: "local-1",
      device_name: "Studio PC",
      hostname: "studio",
      bind_address: "127.0.0.1",
      discovery_port: 4242,
      pid: 999,
      discovered_devices: 1,
      connected_devices: 0,
      healthy: true,
    },
    devices: [
      {
        id: "remote-1",
        name: "Remote Workstation",
        hostname: "remote",
        addresses: ["192.168.1.30"],
        connected: false,
        last_seen_secs: 2,
      },
    ],
    layout: {
      version: 1,
      local_device: "local-1",
      nodes: [
        {
          device_id: "local-1",
          displays: [
            { display_id: "left", x: 0, y: 0, width: 2560, height: 1440, primary: true },
            { display_id: "right", x: 2560, y: 0, width: 2560, height: 1440, primary: false },
          ],
        },
        {
          device_id: "remote-1",
          displays: [{ display_id: "primary", x: 5118, y: 0, width: 1920, height: 1080, primary: true }],
        },
      ],
      links: [],
    },
    visible_layout: {
      version: 1,
      local_device: "local-1",
      nodes: [
        {
          device_id: "local-1",
          displays: [
            { display_id: "left", x: 0, y: 0, width: 2560, height: 1440, primary: true },
            { display_id: "right", x: 2560, y: 0, width: 2560, height: 1440, primary: false },
          ],
        },
        {
          device_id: "remote-1",
          displays: [{ display_id: "primary", x: 5118, y: 0, width: 1920, height: 1080, primary: true }],
        },
      ],
      links: [],
    },
  });

  const remoteMonitor = model.layout.monitors.find((monitor) => monitor.deviceId === "remote-1");
  assert.equal(remoteMonitor.x, 80 + 5120 * 0.12);
});

test("updateRememberedLayoutFromVisibleMonitors saves visible monitor geometry and preserves offline nodes", () => {
  const remembered = {
    version: 1,
    local_device: "local-1",
    nodes: [
      {
        device_id: "local-1",
        displays: [{ display_id: "primary", x: 0, y: 0, width: 1280, height: 720, primary: true }],
      },
      {
        device_id: "offline-1",
        displays: [{ display_id: "primary", x: 1280, y: 0, width: 1920, height: 1080, primary: true }],
      },
      {
        device_id: "remote-1",
        displays: [{ display_id: "primary", x: 3200, y: 0, width: 1024, height: 768, primary: true }],
      },
    ],
    links: [],
  };

  const updated = updateRememberedLayoutFromVisibleMonitors(remembered, [
    {
      id: "remote-1-primary",
      deviceId: "remote-1",
      displayId: "primary",
      rememberedX: 3200,
      rememberedY: 0,
      visibleX: 1280,
      visibleY: 0,
      x: 80 + 1280 * 0.12 + 120,
      y: 170 + 96 * 0.12,
    },
  ]);

  const remoteDisplay = updated.nodes
    .find((node) => node.device_id === "remote-1")
    .displays.find((display) => display.display_id === "primary");
  const offlineDisplay = updated.nodes
    .find((node) => node.device_id === "offline-1")
    .displays.find((display) => display.display_id === "primary");

  assert.equal(remoteDisplay.x, 4200);
  assert.equal(remoteDisplay.y, 96);
  assert.equal(offlineDisplay.x, 1280);
  assert.deepEqual(
    updated.links.map((link) => [link.from_device, link.from_edge, link.to_device, link.to_edge]),
    [
      ["local-1", "Right", "offline-1", "Left"],
      ["offline-1", "Left", "local-1", "Right"],
      ["offline-1", "Right", "remote-1", "Left"],
      ["remote-1", "Left", "offline-1", "Right"],
    ],
  );
  assert.notEqual(updated, remembered);
});

test("updateRememberedLayoutFromVisibleMonitors snaps visible device groups edge-to-edge", () => {
  const remembered = {
    version: 1,
    local_device: "local-1",
    nodes: [
      {
        device_id: "local-1",
        displays: [
          { display_id: "left", x: 0, y: 0, width: 2560, height: 1440, primary: true },
          { display_id: "right", x: 2560, y: 0, width: 2560, height: 1440, primary: false },
        ],
      },
      {
        device_id: "remote-1",
        displays: [
          { display_id: "primary", x: 5200, y: 0, width: 1920, height: 1080, primary: true },
        ],
      },
    ],
    links: [],
  };

  const updated = updateRememberedLayoutFromVisibleMonitors(remembered, [
    {
      id: "local-1-left",
      deviceId: "local-1",
      displayId: "left",
      rememberedX: 0,
      rememberedY: 0,
      visibleX: 0,
      visibleY: 0,
      x: 80,
      y: 170,
    },
    {
      id: "local-1-right",
      deviceId: "local-1",
      displayId: "right",
      rememberedX: 2560,
      rememberedY: 0,
      visibleX: 2560,
      visibleY: 0,
      x: 80 + 2560 * 0.12,
      y: 170,
    },
    {
      id: "remote-1-primary",
      deviceId: "remote-1",
      displayId: "primary",
      rememberedX: 5200,
      rememberedY: 0,
      visibleX: 5200,
      visibleY: 0,
      x: 80 + (5120 * 0.12) + 9,
      y: 170,
    },
  ]);

  const localNode = updated.nodes.find((node) => node.device_id === "local-1");
  const remoteDisplay = updated.nodes
    .find((node) => node.device_id === "remote-1")
    .displays.find((display) => display.display_id === "primary");

  assert.deepEqual(
    localNode.displays.map((display) => [display.display_id, display.x, display.y]),
    [
      ["left", 0, 0],
      ["right", 2560, 0],
    ],
  );
  assert.equal(remoteDisplay.x, 5120);
  assert.equal(remoteDisplay.y, 0);
});

test("buildDesktopViewModel does not synthesize remote layout when daemon layout is unavailable", () => {
  const model = buildDesktopViewModel({
    status: {
      device_id: "local-1",
      device_name: "Studio PC",
      hostname: "studio",
      bind_address: "127.0.0.1",
      discovery_port: 4242,
      pid: 999,
      discovered_devices: 1,
      connected_devices: 0,
      healthy: true,
    },
    devices: [
      {
        id: "remote-1",
        name: "Remote Workstation",
        hostname: "remote",
        addresses: ["192.168.1.30"],
        connected: false,
        last_seen_secs: 2,
      },
    ],
    layout_error: "layout unavailable",
  });

  assert.deepEqual(model.layout.devices.map((device) => device.id), ["local-1"]);
  assert.equal(model.layout.monitors.length, 1);
  assert.equal(model.service.error, "layout unavailable");
});

test("buildDesktopViewModel exposes desktop acceptance payload for settings checklist", () => {
  const model = buildDesktopViewModel({
    status: {
      device_id: "local-1",
      device_name: "Studio PC",
      hostname: "studio",
      bind_address: "192.168.1.10:24801",
      discovery_port: 4242,
      pid: 999,
      discovered_devices: 1,
      connected_devices: 0,
      healthy: true,
      input_mode: "Portable",
      available_backends: ["Portable"],
      backend_health: "Healthy",
      background_owner: "Daemon",
      background_mode: "BackgroundProcess",
      tray_owner: "Daemon",
      tray_state: "Unavailable",
      started_by_desktop: true,
    },
    devices: [
      {
        id: "remote-1",
        name: "Remote Workstation",
        hostname: "remote",
        addresses: ["192.168.1.30"],
        connected: false,
        last_seen_secs: 2,
      },
    ],
    acceptance: {
      daemon_online: true,
      background_ready: true,
      tray_owned_by_daemon: true,
      tray_state: "Unavailable",
      local_endpoint: "192.168.1.10:24801",
      discovered_devices: 1,
      connected_devices: 0,
      visible_layout_devices: 2,
      local_display_count: 1,
      local_ready: true,
      input_ready: true,
      dual_machine_ready: true,
      next_step: "打开另一台机器并连接设备，开始边缘切换验收",
    },
  });

  assert.equal(model.acceptance.daemonOnline, true);
  assert.equal(model.acceptance.backgroundReady, true);
  assert.equal(model.acceptance.trayOwnedByDaemon, true);
  assert.equal(model.acceptance.trayState, "Unavailable");
  assert.equal(model.acceptance.dualMachineReady, true);
  assert.equal(model.acceptance.localReady, true);
  assert.equal(model.acceptance.localDisplayCount, 1);
  assert.equal(model.acceptance.autoStarted, true);
  assert.equal(model.acceptance.checks[0].label, "后台服务");
  assert.equal(model.acceptance.checks[0].state, "pass");
  assert.equal(model.acceptance.checks.some((check) => check.key === "local"), true);
  assert.equal(model.acceptance.checks.at(-1).label, "双机验收");
});

test("buildDesktopViewModel prioritizes local acceptance when no remote devices are discovered", () => {
  const model = buildDesktopViewModel({
    status: {
      device_id: "local-1",
      device_name: "Studio PC",
      hostname: "studio",
      bind_address: "192.168.1.10:24801",
      discovery_port: 4242,
      pid: 999,
      discovered_devices: 0,
      connected_devices: 0,
      healthy: true,
      input_mode: "Portable",
      available_backends: ["Portable"],
      backend_health: "Healthy",
      background_owner: "Daemon",
      background_mode: "BackgroundProcess",
      tray_owner: "Daemon",
      tray_state: "Running",
    },
    devices: [],
    visible_layout: {
      version: 1,
      local_device: "local-1",
      nodes: [
        {
          device_id: "local-1",
          displays: [
            { display_id: "primary", x: 0, y: 0, width: 2560, height: 1440, primary: true },
            { display_id: "display-2", x: 2560, y: 0, width: 2560, height: 1440, primary: false },
          ],
        },
      ],
      links: [],
    },
    acceptance: {
      daemon_online: true,
      background_ready: true,
      tray_owned_by_daemon: true,
      tray_state: "Running",
      local_endpoint: "192.168.1.10:24801",
      discovered_devices: 0,
      connected_devices: 0,
      visible_layout_devices: 1,
      local_display_count: 2,
      local_ready: true,
      input_ready: true,
      dual_machine_ready: false,
      next_step: "本机能力已就绪，可以进行本机设备监控；双机验收等待局域网发现",
    },
  });

  const localCheck = model.acceptance.checks.find((check) => check.key === "local");
  const layoutCheck = model.acceptance.checks.find((check) => check.key === "layout");
  const discoveryCheck = model.acceptance.checks.find((check) => check.key === "discovery");
  const dualMachineCheck = model.acceptance.checks.find((check) => check.key === "dual-machine");

  assert.equal(model.acceptance.localReady, true);
  assert.equal(model.acceptance.dualMachineReady, false);
  assert.equal(localCheck.state, "pass");
  assert.equal(layoutCheck.state, "pass");
  assert.equal(discoveryCheck.state, "warn");
  assert.equal(dualMachineCheck.state, "warn");
  assert.equal(layoutCheck.detail, "本机显示器 2 块，Layout 当前显示 1 个在线节点");
});
