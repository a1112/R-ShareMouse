import assert from "node:assert/strict";
import test from "node:test";

import {
  macosInputPermissionSummary,
  missingMacosInputPermissions,
  normalizeMacosInputPermissions,
} from "./macos-permissions.mjs";

test("normalizes a complete macOS input permission snapshot", () => {
  assert.deepEqual(
    normalizeMacosInputPermissions({
      supported: true,
      input_monitoring: true,
      accessibility: true,
    }),
    {
      supported: true,
      input_monitoring: true,
      accessibility: true,
      ready: true,
    },
  );
  assert.equal(macosInputPermissionSummary({ supported: true, input_monitoring: true, accessibility: true }), "已就绪");
});

test("reports exactly the missing macOS permission panes", () => {
  const snapshot = {
    supported: true,
    input_monitoring: false,
    accessibility: true,
  };

  assert.deepEqual(
    missingMacosInputPermissions(snapshot).map((item) => item.key),
    ["input_monitoring"],
  );
  assert.equal(macosInputPermissionSummary(snapshot), "缺少输入监控");
});

test("does not show a macOS warning for unsupported runtimes", () => {
  assert.equal(normalizeMacosInputPermissions({ supported: false }), null);
  assert.deepEqual(missingMacosInputPermissions({ supported: false }), []);
  assert.equal(macosInputPermissionSummary({ supported: false }), "未检测");
});
