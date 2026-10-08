import test from "node:test";
import assert from "node:assert/strict";
import { createDeviceNotificationTracker, describeDeviceNotification, deviceNotificationSnapshot } from "./device-notifications.mjs";

const peer = (id, connected = false, name = id) => ({ id, connected, name });
const snapshot = (devices, bootId = "boot-a") => ({ ready: true, bootId, devices });

test("initial snapshots and duplicate polls do not announce existing devices", () => {
  const tracker = createDeviceNotificationTracker();
  assert.deepEqual(tracker.observe({ ready: false, devices: [] }), []);
  assert.deepEqual(tracker.observe(snapshot([peer("a", true)])), []);
  assert.deepEqual(tracker.observe(snapshot([peer("a", true)])), []);
  assert.deepEqual(tracker.observe(snapshot([peer("a", true, "renamed")])), []);
});

test("discovery requires a separate authenticated connected transition to join", () => {
  const tracker = createDeviceNotificationTracker();
  tracker.observe(snapshot([]));
  const discovered = tracker.observe(snapshot([peer("a")]));
  assert.deepEqual(discovered, [{ deviceId: "a", name: "a", kind: "discovered" }]);
  assert.match(describeDeviceNotification(discovered[0]).description, /确认连接/);
  assert.deepEqual(tracker.observe(snapshot([peer("a", true)])), [{ deviceId: "a", name: "a", kind: "joined" }]);
  assert.deepEqual(tracker.observe(snapshot([peer("a", true)])), []);
});

test("authenticated peers appearing already connected emit one joined event", () => {
  const tracker = createDeviceNotificationTracker();
  tracker.observe(snapshot([]));
  assert.deepEqual(tracker.observe(snapshot([peer("a", true)])), [{ deviceId: "a", name: "a", kind: "joined" }]);
});

test("disconnect and removal retain identity for a recovery notification", () => {
  const tracker = createDeviceNotificationTracker();
  tracker.observe(snapshot([peer("a", true)]));
  assert.deepEqual(tracker.observe(snapshot([peer("a")])), []);
  assert.deepEqual(tracker.observe(snapshot([peer("a", true)])), [{ deviceId: "a", name: "a", kind: "reconnected" }]);
  tracker.observe(snapshot([]));
  assert.deepEqual(tracker.observe(snapshot([peer("a", true)])), [{ deviceId: "a", name: "a", kind: "reconnected" }]);
});

test("a new daemon boot establishes a silent baseline", () => {
  const tracker = createDeviceNotificationTracker();
  tracker.observe(snapshot([peer("a")]));
  assert.deepEqual(tracker.observe(snapshot([peer("a", true), peer("b", true)], "boot-b")), []);
  tracker.observe(snapshot([peer("a"), peer("b", true)], "boot-b"));
  assert.equal(tracker.observe(snapshot([peer("a", true), peer("b", true)], "boot-b"))[0].kind, "reconnected");
});

test("dashboard restart with an old stream boot and stream recovery both rebaseline silently", () => {
  const tracker = createDeviceNotificationTracker();
  const state = (pid, bootId, devices) => ({ bootId, connections: { status: { pid }, devices } });
  tracker.observe(deviceNotificationSnapshot(state(10, "old-stream-boot", [peer("a")])));
  assert.deepEqual(tracker.observe(deviceNotificationSnapshot(state(11, "old-stream-boot", [peer("a", true)]))), []);
  assert.deepEqual(tracker.observe(deviceNotificationSnapshot(state(11, "new-stream-boot", [peer("a", true)]))), []);
});

test("muting still observes all transitions and enabling does not replay them", () => {
  const tracker = createDeviceNotificationTracker();
  tracker.observe(snapshot([]));
  assert.deepEqual(tracker.observe(snapshot([peer("a")]), { enabled: false }), []);
  assert.deepEqual(tracker.observe(snapshot([peer("a", true)]), { enabled: false }), []);
  assert.deepEqual(tracker.observe(snapshot([peer("a", true)])), []);
  tracker.observe(snapshot([peer("a")]));
  assert.equal(tracker.observe(snapshot([peer("a", true)]))[0].kind, "reconnected");
});

test("batch arrivals emit each identity once and ignore invalid or repeated entries", () => {
  const tracker = createDeviceNotificationTracker();
  tracker.observe(snapshot([]));
  const events = tracker.observe(snapshot([peer("a"), peer("b", true), peer("a"), {}, null]));
  assert.deepEqual(events.map(({ deviceId, kind }) => [deviceId, kind]), [["a", "discovered"], ["b", "joined"]]);
});
