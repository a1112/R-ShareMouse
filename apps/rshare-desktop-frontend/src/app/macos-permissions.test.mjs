import assert from "node:assert/strict";
import test from "node:test";

import {
  buildMacosInputWarning,
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

test("keeps the macOS header warning visible when the daemon input backend faults", () => {
  assert.deepEqual(
    buildMacosInputWarning(
      { supported: true, input_monitoring: true, accessibility: true },
      {
        runtimeDegraded: true,
        runtimeReason: "macOS native input capture listener stopped or faulted",
      },
    ),
    {
      label: "输入异常⚠️",
      summary: "守护进程输入后端未就绪：macOS native input capture listener stopped or faulted",
      runtimeIssue: "macOS native input capture listener stopped or faulted",
    },
  );
});

test("combines a missing permission and daemon health warning", () => {
  const warning = buildMacosInputWarning(
    { supported: true, input_monitoring: false, accessibility: true },
    { runtimeDegraded: true, runtimeReason: "Unavailable" },
  );

  assert.equal(warning.label, "权限不足⚠️");
  assert.equal(warning.summary, "缺少输入监控；守护进程输入后端未就绪：Unavailable");
});
