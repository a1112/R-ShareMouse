import test from "node:test";
import assert from "node:assert/strict";
import { dispatchFileDrop, fileDropTarget, transferPercent, transferIsActive } from "./file-transfer.mjs";

test("native Retina drop hits the correct connected peer and preserves Unicode paths", async () => {
  const devices = [{ id: "peer", connected: true }];
  const document = { elementFromPoint(x, y) {
    assert.equal(x, 120); assert.equal(y, 40);
    return { closest: () => ({ getAttribute: () => "peer" }) };
  } };
  const calls = [];
  await dispatchFileDrop({ position: { x: 240, y: 80 }, paths: ["/tmp/报告.txt"] }, devices, document, 2,
    async (...args) => calls.push(args));
  assert.deepEqual(calls, [["send_files", { deviceId: "peer", paths: ["/tmp/报告.txt"] }]]);
});

test("disconnected, non-target and empty drags never dispatch file sends", async () => {
  let sent = false;
  const document = { elementFromPoint: () => ({ closest: () => ({ getAttribute: () => "peer" }) }) };
  for (const [devices, paths] of [[[{ id: "peer", connected: false }], ["/tmp/a"]], [[], ["/tmp/a"]], [[{ id: "peer", connected: true }], []]]) {
    await assert.rejects(dispatchFileDrop({ position: { x: 1, y: 1 }, paths }, devices, document, 1, () => { sent = true; }));
  }
  assert.equal(sent, false);
  assert.equal(fileDropTarget({}, [], document, 1), null);
});

test("progress waits for receiver commit even for empty files", () => {
  assert.equal(transferPercent({status: "Transferring", total_bytes: 10, transferred_bytes: 10}), 99);
  assert.equal(transferPercent({status: "Completed", total_bytes: 0, transferred_bytes: 0}), 100);
  assert.equal(transferPercent({status: "Waiting", total_bytes: 0, transferred_bytes: 0}), 0);
  assert.equal(transferIsActive("Failed"), false);
  assert.equal(transferIsActive("Waiting"), true);
});
