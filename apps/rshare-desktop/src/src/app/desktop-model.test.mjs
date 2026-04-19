import test from "node:test";
import assert from "node:assert/strict";

import { buildDesktopViewModel } from "./desktop-model.mjs";

test("buildDesktopViewModel returns an offline local-only layout when daemon is unavailable", () => {
  const model = buildDesktopViewModel({ status: null, devices: [] });

  assert.equal(model.service.online, false);
  assert.equal(model.layout.devices.length, 1);
  assert.equal(model.layout.devices[0].kind, "local");
  assert.equal(model.layout.monitors.length, 1);
  assert.equal(model.devices.length, 0);
  assert.equal(model.settings.localDevice.name, "本机");
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
