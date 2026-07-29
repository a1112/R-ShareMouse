import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import TestRenderer, { act } from "react-test-renderer";

import {
  createUiStore,
  createUiStateAppBindings,
  createOwnerlessStreamCoordinator,
  selectDashboardPayload,
  selectInputVisuals,
  selectTopologyProjection,
} from "./ui-store.mjs";
import { UiStateClient } from "./ui-state-client.mjs";
import { createUseUiStore } from "./use-ui-store.ts";
import {
  buildDesktopViewModel,
  equalLayoutDevices,
  projectUiInputToLocalControls,
  reconcileLayoutDevices,
} from "./desktop-model.mjs";

const BOOT_ID = "00000000-0000-0000-0000-000000000001";

function snapshot(revision = 0) {
  return {
    protocol_version: 1,
    boot_id: BOOT_ID,
    revision,
    status: { healthy: true, latency_feedback: { latency_ms: 2 } },
    devices: [{ id: "peer-1", connected: true }],
    layout: { version: 7, nodes: [{ device_id: "local" }], links: [] },
    capabilities: { devices: [] },
    display_inventory: { displays: [{ display_id: "display-1" }] },
    dynamic_state: {
      pointer: { x: 1, y: 2, observed_at_ms: 1 },
      gamepads: [],
      pressed_keys: [],
      pressed_mouse_buttons: [],
      pressed_gamepad_buttons: [],
      diagnostics: { latency_ms: 2 },
    },
    active_sessions: { control: null, media_sessions: [] },
  };
}

function envelope(type, payload) {
  return { type, payload };
}

function delta(revision, type, payload) {
  return envelope("delta", {
    boot_id: BOOT_ID,
    revision,
    change: { type, payload },
  });
}

function fakeAnimationFrames() {
  let nextId = 1;
  const callbacks = new Map();
  return {
    request(callback) {
      const id = nextId++;
      callbacks.set(id, callback);
      return id;
    },
    cancel(id) {
      callbacks.delete(id);
    },
    flush() {
      const pending = [...callbacks.values()];
      callbacks.clear();
      for (const callback of pending) callback(0);
    },
    get size() {
      return callbacks.size;
    },
  };
}

function fakeTimers() {
  let nextId = 1;
  let now = 0;
  const tasks = new Map();
  return {
    setTimeout(callback, delay = 0) {
      const id = nextId++;
      tasks.set(id, { at: now + Number(delay), callback });
      return id;
    },
    clearTimeout(id) {
      tasks.delete(id);
    },
    advance(milliseconds) {
      const target = now + milliseconds;
      while (true) {
        const due = [...tasks.entries()]
          .filter(([, task]) => task.at <= target)
          .sort((left, right) => left[1].at - right[1].at)[0];
        if (!due) break;
        const [id, task] = due;
        tasks.delete(id);
        now = task.at;
        task.callback();
      }
      now = target;
    },
  };
}

async function flushMicrotasks(count = 4) {
  for (let index = 0; index < count; index += 1) {
    await Promise.resolve();
  }
}

test("1000 pointer deltas commit the latest pointer once per animation frame", () => {
  const frames = fakeAnimationFrames();
  const store = createUiStore({
    requestAnimationFrame: frames.request,
    cancelAnimationFrame: frames.cancel,
  });
  store.applySnapshot(snapshot());
  let inputCommits = 0;
  let topologyCommits = 0;
  store.subscribe(selectInputVisuals, () => {
    inputCommits += 1;
  });
  store.subscribe(selectTopologyProjection, () => {
    topologyCommits += 1;
  });

  for (let revision = 1; revision <= 1000; revision += 1) {
    store.applyEnvelope(
      delta(revision, "pointer", {
        x: revision,
        y: -revision,
        observed_at_ms: revision,
      }),
    );
  }

  assert.equal(frames.size, 1);
  assert.equal(inputCommits, 0);
  assert.equal(topologyCommits, 0);
  frames.flush();
  assert.equal(inputCommits, 1);
  assert.equal(topologyCommits, 0);
  assert.equal(selectInputVisuals(store.getState()).pointer.x, 1000);
});

test("continuous gamepad deltas use the latest pending slot", () => {
  const frames = fakeAnimationFrames();
  const store = createUiStore({
    requestAnimationFrame: frames.request,
    cancelAnimationFrame: frames.cancel,
  });
  store.applySnapshot(snapshot());
  store.applyEnvelope(delta(1, "gamepads", [{ gamepad_id: 1, left_stick_x: 1 }]));
  store.applyEnvelope(delta(2, "gamepads", [{ gamepad_id: 1, left_stick_x: 99 }]));

  assert.equal(frames.size, 1);
  frames.flush();
  assert.equal(selectInputVisuals(store.getState()).gamepads[0].left_stick_x, 99);
});

test("a pending pointer commit cannot roll revision back after a discrete delta", () => {
  const frames = fakeAnimationFrames();
  const store = createUiStore({
    requestAnimationFrame: frames.request,
    cancelAnimationFrame: frames.cancel,
  });
  store.applySnapshot(snapshot());
  store.applyEnvelope(
    delta(1, "pointer", { x: 1, y: 1, observed_at_ms: 1 }),
  );
  store.applyEnvelope(
    delta(2, "key_button", {
      type: "key",
      key_code: 30,
      state: "Pressed",
      observed_at_ms: 2,
    }),
  );
  frames.flush();

  assert.equal(store.getState().revision, 2);
  assert.equal(store.getState().inputVisuals.pointer.x, 1);
  assert.deepEqual(store.getState().inputVisuals.pressedKeys, [30]);
});

test("discrete key and button transitions apply immediately and in order", () => {
  const store = createUiStore();
  store.applySnapshot(snapshot());
  const observed = [];
  store.subscribe(selectInputVisuals, (input) => {
    observed.push({
      keys: [...input.pressedKeys],
      buttons: [...input.pressedMouseButtons],
    });
  });

  store.applyEnvelope(
    delta(1, "key_button", {
      type: "key",
      key_code: 30,
      state: "Pressed",
      observed_at_ms: 1,
    }),
  );
  store.applyEnvelope(
    delta(2, "key_button", {
      type: "mouse_button",
      button: "Left",
      state: "Pressed",
      observed_at_ms: 2,
    }),
  );
  store.applyEnvelope(
    delta(3, "key_button", {
      type: "key",
      key_code: 30,
      state: "Released",
      observed_at_ms: 3,
    }),
  );

  assert.deepEqual(observed, [
    { keys: [30], buttons: [] },
    { keys: [30], buttons: ["Left"] },
    { keys: [], buttons: ["Left"] },
  ]);
});

test("full snapshot cancels stale RAF and atomically replaces every slice", () => {
  const frames = fakeAnimationFrames();
  const store = createUiStore({
    requestAnimationFrame: frames.request,
    cancelAnimationFrame: frames.cancel,
  });
  store.applySnapshot(snapshot());
  store.applyEnvelope(
    delta(1, "pointer", { x: 500, y: 500, observed_at_ms: 2 }),
  );
  const replacement = snapshot(20);
  replacement.layout = { version: 20, nodes: [], links: [] };
  replacement.dynamic_state.pointer = { x: 20, y: 21, observed_at_ms: 20 };
  replacement.active_sessions.media_sessions = [{ session_id: "media-20" }];
  let notifications = 0;
  store.subscribe((state) => state, () => {
    notifications += 1;
  });

  store.applySnapshot(replacement);
  assert.equal(frames.size, 0);
  assert.equal(notifications, 1);
  frames.flush();
  assert.equal(notifications, 1);
  assert.equal(store.getState().topology.layout.version, 20);
  assert.equal(store.getState().inputVisuals.pointer.x, 20);
  assert.equal(store.getState().mediaSession.mediaSessions[0].session_id, "media-20");
});

test("fallback snapshot cannot race an accepted pointer still pending in RAF", async () => {
  const frames = fakeAnimationFrames();
  const store = createUiStore({
    requestAnimationFrame: frames.request,
    cancelAnimationFrame: frames.cancel,
  });
  store.applySnapshot(snapshot());
  let resolveFallback;
  const fallback = new Promise((resolve) => {
    resolveFallback = resolve;
  });
  const bindings = createUiStateAppBindings({
    store,
    loadFallbackSnapshot: () => fallback,
  });
  const pending = bindings.onStatus({ state: "fallback_poll" });
  store.applyEnvelope(
    delta(1, "pointer", { x: 10, y: 10, observed_at_ms: 1 }),
  );
  resolveFallback({
    status: { healthy: false },
    devices: [],
    layout: { version: 99, nodes: [], links: [] },
  });
  await pending;

  assert.equal(store.currentRevision(), 1);
  assert.equal(store.getState().connections.status.healthy, true);
  assert.equal(store.getState().topology.layout.version, 7);
});

test("fallback requested before a same-revision UI snapshot cannot replace it", async () => {
  const store = createUiStore();
  let resolveFallback;
  const fallback = new Promise((resolve) => {
    resolveFallback = resolve;
  });
  const bindings = createUiStateAppBindings({
    store,
    loadFallbackSnapshot: () => fallback,
  });
  const pending = bindings.onStatus({ state: "fallback_poll" });
  const authoritative = snapshot(0);
  authoritative.status = { healthy: true, source: "ui-stream" };
  store.applySnapshot(authoritative);
  resolveFallback({
    status: { healthy: false, source: "fallback" },
    devices: [],
    layout: { version: 99, nodes: [], links: [] },
  });
  await pending;

  assert.equal(store.getState().connections.status.source, "ui-stream");
  assert.equal(store.getState().topology.layout.version, 7);
});

test("fallback side effects run only after the snapshot is accepted", async () => {
  const store = createUiStore();
  let resolveFallback;
  const applied = [];
  const fallback = new Promise((resolve) => {
    resolveFallback = resolve;
  });
  const bindings = createUiStateAppBindings({
    store,
    loadFallbackSnapshot: () => fallback,
    onFallbackApplied: (value) => {
      applied.push(value.layout_error);
    },
  });
  const stalePoll = bindings.onStatus({ state: "fallback_poll" });
  store.applySnapshot(snapshot(0));
  resolveFallback({
    status: { healthy: false },
    devices: [],
    layout: { version: 99, nodes: [], links: [] },
    layout_error: "stale error",
  });
  await stalePoll;
  assert.deepEqual(applied, []);

  const acceptedBindings = createUiStateAppBindings({
    store,
    loadFallbackSnapshot: async () => ({
      status: { healthy: false },
      devices: [],
      layout: { version: 3, nodes: [], links: [] },
      layout_error: "accepted error",
    }),
    onFallbackApplied: (value) => {
      applied.push(value.layout_error);
    },
  });
  await acceptedBindings.onStatus({ state: "fallback_poll" });
  assert.deepEqual(applied, ["accepted error"]);
});

test("only one legacy snapshot may commit for an accepted store version", () => {
  const store = createUiStore();
  const version = store.currentVersion();
  const first = {
    status: { device_name: "newer" },
    devices: [],
    layout: { version: 1, nodes: [], links: [] },
  };
  const late = {
    status: { device_name: "late-old" },
    devices: [],
    layout: { version: 2, nodes: [], links: [] },
  };

  assert.equal(store.applyDashboardSnapshot(first, version), true);
  assert.equal(store.applyDashboardSnapshot(late, version), false);
  assert.equal(store.getState().connections.status.device_name, "newer");
  assert.equal(store.currentVersion(), version + 1);
});

test("legacy display truth remains until the first UI snapshot then authority switches", () => {
  const store = createUiStore();
  const legacyDisplay = {
    display_count: 1,
    displays: [
      {
        display_id: "primary",
        friendly_name: "Legacy Physical Panel",
        width: 1920,
        height: 1080,
        raw_dpi_x: 96,
        raw_dpi_y: 96,
        primary: true,
      },
    ],
  };
  store.applyDashboardSnapshot(
    {
      status: {
        device_id: "local",
        device_name: "Local",
        hostname: "local",
        healthy: true,
      },
      devices: [],
      visible_layout: {
        version: 1,
        local_device: "local",
        nodes: [
          {
            device_id: "local",
            displays: [
              {
                display_id: "primary",
                x: 0,
                y: 0,
                width: 1280,
                height: 720,
                primary: true,
              },
            ],
          },
        ],
        links: [],
      },
    },
    store.currentVersion(),
  );

  const legacyPayload = selectDashboardPayload(store.getState());
  assert.equal(legacyPayload.display_inventory, undefined);
  const legacyModel = buildDesktopViewModel(legacyPayload, {
    display: legacyDisplay,
  });
  assert.equal(legacyModel.layout.monitors[0].name, "Legacy Physical Panel");
  assert.equal(legacyModel.layout.monitors[0].resWidth, 1920);

  const authoritative = snapshot(1);
  authoritative.status = {
    device_id: "local",
    device_name: "Local",
    hostname: "local",
    healthy: true,
  };
  authoritative.layout = {
    version: 2,
    local_device: "local",
    nodes: [
      {
        device_id: "local",
        displays: [
          {
            display_id: "primary",
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
            primary: true,
          },
        ],
      },
    ],
    links: [],
  };
  authoritative.display_inventory = {
    display_count: 1,
    displays: [
      {
        display_id: "primary",
        friendly_name: "UI Authoritative Panel",
        width: 2560,
        height: 1440,
        raw_dpi_x: 93,
        raw_dpi_y: 93,
        primary: true,
      },
    ],
  };
  store.applySnapshot(authoritative);

  const uiModel = buildDesktopViewModel(
    selectDashboardPayload(store.getState()),
    { display: legacyDisplay },
  );
  assert.equal(uiModel.layout.monitors[0].name, "UI Authoritative Panel");
  assert.equal(uiModel.layout.monitors[0].resWidth, 2560);
});

test("topology projection is memoized by topology revision", () => {
  const store = createUiStore();
  store.applySnapshot(snapshot());
  const first = selectTopologyProjection(store.getState());
  store.applyEnvelope(delta(1, "diagnostics", { latency_ms: 9 }));
  const second = selectTopologyProjection(store.getState());
  store.applyEnvelope(delta(2, "topology", { version: 8, nodes: [], links: [] }));
  const third = selectTopologyProjection(store.getState());

  assert.equal(second, first);
  assert.notEqual(third, second);
});

test("topology selector never leaks a same-number revision across stores", () => {
  const firstStore = createUiStore();
  const secondStore = createUiStore();
  const firstSnapshot = snapshot();
  const secondSnapshot = snapshot();
  secondSnapshot.layout = { version: 700, nodes: [], links: [] };
  firstStore.applySnapshot(firstSnapshot);
  secondStore.applySnapshot(secondSnapshot);

  assert.equal(selectTopologyProjection(firstStore.getState()).layout.version, 7);
  assert.equal(
    selectTopologyProjection(secondStore.getState()).layout.version,
    700,
  );
});

test("React selector subscriber does not rerender for unrelated slices", () => {
  const store = createUiStore();
  const useTestUiStore = createUseUiStore(store);
  store.applySnapshot(snapshot());
  let renders = 0;

  function TopologyVersion() {
    const topology = useTestUiStore(
      (state) => ({
        version: selectTopologyProjection(state).layout.version,
      }),
      (left, right) => left.version === right.version,
    );
    renders += 1;
    return React.createElement("span", null, topology.version);
  }

  let renderer;
  act(() => {
    renderer = TestRenderer.create(React.createElement(TopologyVersion));
  });
  assert.equal(renders, 1);

  act(() => {
    store.applyEnvelope(delta(1, "diagnostics", { latency_ms: 10 }));
  });
  assert.equal(renders, 1);

  act(() => {
    store.applyEnvelope(delta(2, "topology", { version: 9, nodes: [], links: [] }));
  });
  assert.equal(renders, 2);
  renderer.unmount();
});

test("equal external topology devices do not require MonitorManager replacement", () => {
  const previous = [
    {
      id: "local",
      name: "Local",
      color: "#fff",
      online: true,
      type: "desktop",
      expanded: false,
    },
  ];
  const equalIncoming = [{ ...previous[0], expanded: true }];
  const changed = [{ ...previous[0], name: "Renamed", expanded: true }];

  assert.equal(equalLayoutDevices(previous, equalIncoming), true);
  assert.equal(equalLayoutDevices(previous, changed), false);
  assert.equal(reconcileLayoutDevices(previous, equalIncoming), previous);
  assert.notEqual(reconcileLayoutDevices(previous, changed), previous);
});

test("UI input slice projects into local visuals without mutating fallback truth", () => {
  const fallback = {
    sequence: 4,
    keyboard: { detected: false, pressed_keys: [], event_count: 4 },
    mouse: { x: 1, y: 2, pressed_buttons: [], event_count: 4 },
    gamepads: [],
    display: { display_count: 0, displays: [] },
  };
  const projected = projectUiInputToLocalControls(
    {
      pointer: { x: 80, y: 90, display_id: "display-1", observed_at_ms: 8 },
      gamepads: [{ gamepad_id: 1, name: "Pad" }],
      pressedKeys: [30],
      pressedMouseButtons: ["Left"],
      pressedGamepadButtons: [],
      lastDiscreteTransition: null,
    },
    { displayInventory: { display_count: 1, displays: [{ display_id: "display-1" }] } },
    fallback,
  );

  assert.equal(projected.mouse.x, 80);
  assert.equal(projected.mouse.current_display_id, "display-1");
  assert.deepEqual(projected.keyboard.pressed_keys, ["30"]);
  assert.equal(projected.keyboard.detected, true);
  assert.equal(projected.gamepads[0].name, "Pad");
  assert.equal(projected.display.display_count, 1);
  assert.equal(fallback.mouse.x, 1);
});

test("UI input projection does not invent keyboard or mouse detection", () => {
  const projected = projectUiInputToLocalControls(
    {
      pointer: null,
      gamepads: [],
      pressedKeys: [],
      pressedMouseButtons: [],
      pressedGamepadButtons: [],
      lastDiscreteTransition: null,
    },
    { displayInventory: { displays: [] } },
    null,
  );

  assert.equal(projected.keyboard.detected, false);
  assert.equal(projected.mouse.detected, false);
});

test("UI input projection preserves fallback identity before the first snapshot", () => {
  const fallback = {
    gamepads: [{ gamepad_id: 7 }],
    display: { display_count: 1, displays: [{ display_id: "fallback" }] },
  };

  const projected = projectUiInputToLocalControls(
    {
      pointer: null,
      gamepads: [],
      pressedKeys: [],
      pressedMouseButtons: [],
      pressedGamepadButtons: [],
      lastDiscreteTransition: null,
    },
    { displayInventory: { displays: [] } },
    fallback,
    { authoritative: false },
  );

  assert.equal(projected, fallback);
});

test("healthy UI push causes no fallback dashboard or endpoint polls over ten seconds", async () => {
  const calls = { dashboard: 0 };
  const store = createUiStore();
  const bindings = createUiStateAppBindings({
    store,
    loadFallbackSnapshot: async () => {
      calls.dashboard += 1;
      return snapshot(99);
    },
  });
  const timers = fakeTimers();
  const originalSetTimeout = globalThis.setTimeout;
  const originalClearTimeout = globalThis.clearTimeout;
  globalThis.setTimeout = timers.setTimeout;
  globalThis.clearTimeout = timers.clearTimeout;
  const connection = {};
  const client = new UiStateClient({
    connect: async ({ onEnvelope }) => {
      queueMicrotask(() => onEnvelope(envelope("snapshot", snapshot())));
      return connection;
    },
    onEnvelope: bindings.onEnvelope,
    onStatus: bindings.onStatus,
    heartbeatTimeoutMs: 12_000,
  });

  try {
    await client.start();
    await Promise.resolve();
    await Promise.resolve();
    timers.advance(10_000);
    await Promise.resolve();
    await client.stop();
  } finally {
    globalThis.setTimeout = originalSetTimeout;
    globalThis.clearTimeout = originalClearTimeout;
  }

  assert.deepEqual(calls, { dashboard: 0 });
  assert.equal(store.getState().revision, 0);
});

test("ownerless stream coordinator finishes A before starting B", async () => {
  let resolveStartA;
  let owner = null;
  let starts = 0;
  const events = [];
  const startA = new Promise((resolve) => {
    resolveStartA = resolve;
  });
  const coordinator = createOwnerlessStreamCoordinator({
    start: async () => {
      starts += 1;
      const label = starts === 1 ? "A" : "B";
      events.push(`start${label}`);
      if (label === "A") await startA;
      owner = label;
    },
    stop: async () => {
      events.push(`stop${owner}`);
      owner = null;
    },
  });

  const leaseA = coordinator.acquire();
  await flushMicrotasks();
  assert.deepEqual(events, ["startA"]);
  const releasingA = coordinator.release(leaseA);
  const leaseB = coordinator.acquire();
  assert.deepEqual(events, ["startA"]);

  resolveStartA();
  assert.equal(await leaseA.ready, false);
  await releasingA;
  assert.equal(await leaseB.ready, true);
  assert.deepEqual(events, ["startA", "stopA", "startB"]);
  assert.equal(leaseB.active, true);
  assert.equal(owner, "B");

  await coordinator.release(leaseB);
  assert.deepEqual(events, ["startA", "stopA", "startB", "stopB"]);
});

test("cancelled ownerless start failure is clean and does not block the next lease", async () => {
  let rejectStartA;
  let starts = 0;
  const startA = new Promise((_, reject) => {
    rejectStartA = reject;
  });
  const coordinator = createOwnerlessStreamCoordinator({
    start: async () => {
      starts += 1;
      if (starts === 1) await startA;
    },
    stop: async () => {},
  });

  const leaseA = coordinator.acquire();
  await flushMicrotasks();
  assert.equal(starts, 1);
  const releasingA = coordinator.release(leaseA);
  const leaseB = coordinator.acquire();
  rejectStartA(new Error("late cancelled failure"));

  assert.equal(await leaseA.ready, false);
  await releasingA;
  assert.equal(await leaseB.ready, true);
  await coordinator.release(leaseB);
});
