import test from "node:test";
import assert from "node:assert/strict";
import { availableWakePeers, buildWakeRows, suggestedWakeIpv4 } from "./wake-model.mjs";

test("saved peer remains in wake list after disappearing from discovery", () => {
  const targets = [{ id: "wake-1", name: "Desk", mac: "02:11:22:33:44:55", peer_id: "peer-1", ipv4: null }];
  const online = buildWakeRows(targets, [{ id: "peer-1", connected: true }], {});
  assert.equal(online[0].online, true);
  const offline = buildWakeRows(targets, [], {});
  assert.equal(offline.length, 1);
  assert.equal(offline[0].online, false);
  assert.equal(offline[0].name, "Desk");
});

test("peer IPv4 suggestion ignores unknown and IPv6 addresses", () => {
  assert.equal(suggestedWakeIpv4("192.168.1.50"), "192.168.1.50");
  assert.equal(suggestedWakeIpv4("未知"), "");
  assert.equal(suggestedWakeIpv4("fe80::1234"), "");
});

test("editing an offline saved peer keeps its selection visible", () => {
  const options = availableWakePeers([], [{ peer_id: "peer-1" }], "peer-1", "Desk");
  assert.deepEqual(options, [{ id: "peer-1", name: "Desk（离线）" }]);
});

test("new peer targets can be created from currently online devices", () => {
  const devices = [
    { id: "old", name: "Old", online: false, connected: false },
    { id: "live", name: "Live", online: true, connected: false },
  ];
  assert.deepEqual(availableWakePeers(devices, [], null, ""), [{ id: "live", name: "Live" }]);
});
