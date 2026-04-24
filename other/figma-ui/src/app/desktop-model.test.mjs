import test from "node:test";
import assert from "node:assert/strict";

import {
  buildDesktopViewModel,
  buildLocalControlsViewModel,
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
  assert.equal(model.acceptance.autoStarted, true);
  assert.equal(model.acceptance.checks[0].label, "后台服务");
  assert.equal(model.acceptance.checks[0].state, "pass");
  assert.equal(model.acceptance.checks.at(-1).label, "双机验收");
});
