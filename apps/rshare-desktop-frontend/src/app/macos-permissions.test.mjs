import assert from "node:assert/strict";
import test from "node:test";

import {
  MACOS_PERMISSION_ITEMS,
  assessMacosInputRecovery,
  buildMacosInputWarning,
  canRestartMacosInputService,
  macosInputPermissionSummary,
  missingMacosInputPermissions,
  normalizeMacosInputPermissions,
  shouldPromptForMacosInputPermissions,
} from "./macos-permissions.mjs";

test("prompts once the macOS permission check finds a missing grant or fails", () => {
  assert.equal(shouldPromptForMacosInputPermissions(false, null), false);
  assert.equal(shouldPromptForMacosInputPermissions(true, null), true);
  assert.equal(shouldPromptForMacosInputPermissions(true, {
    supported: true, input_monitoring: false, accessibility: true,
  }), true);
  assert.equal(shouldPromptForMacosInputPermissions(true, {
    supported: true, input_monitoring: true, accessibility: true,
  }), false);
});

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

test("reports missing input capabilities without claiming a system permission pane grant", () => {
  const snapshot = {
    supported: true,
    input_monitoring: false,
    accessibility: true,
  };

  assert.deepEqual(
    missingMacosInputPermissions(snapshot).map((item) => item.key),
    ["input_monitoring"],
  );
  assert.equal(macosInputPermissionSummary(snapshot), "输入监听能力不可用");
  assert.equal(macosInputPermissionSummary({
    supported: true, input_monitoring: true, accessibility: false,
  }), "输入注入能力不可用");
  assert.equal(MACOS_PERMISSION_ITEMS[0].label, "输入监听能力");
  assert.equal(MACOS_PERMISSION_ITEMS[1].label, "输入注入能力");
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

  assert.equal(warning.label, "输入能力不足⚠️");
  assert.equal(warning.summary, "输入监听能力不可用；守护进程输入后端未就绪：Unavailable");
});

test("permits a service restart while the old daemon still reports missing input access", () => {
  const stalePermissions = {
    supported: true, input_monitoring: true, accessibility: false,
  };
  assert.equal(canRestartMacosInputService(stalePermissions, {
    restartRequired: true, daemonInputReady: false,
  }), true);
  assert.equal(canRestartMacosInputService(stalePermissions, {
    restartRequired: false, daemonInputReady: false,
  }), true);
  assert.equal(canRestartMacosInputService(stalePermissions, {
    restartRequired: false, daemonInputReady: true,
  }), false);
  assert.equal(canRestartMacosInputService(null, { restartRequired: true }), false);
});

test("accepts restored access only after fresh daemon input health is ready", () => {
  const restoredPermissions = {
    supported: true, input_monitoring: true, accessibility: true,
  };
  const freshStatus = {
    pid: 59867, input_mode: "MacosNative", backend_health: "Healthy",
  };
  assert.deepEqual(assessMacosInputRecovery(restoredPermissions, freshStatus), {
    ready: true, reason: null,
  });
  assert.deepEqual(assessMacosInputRecovery(restoredPermissions, {
    ...freshStatus, pid: 53656,
    backend_health: { Degraded: { reason: "PermissionDenied" } },
  }), {
    ready: false,
    reason: "守护进程输入后端未就绪：PermissionDenied",
  });
  assert.deepEqual(assessMacosInputRecovery(restoredPermissions, null), {
    ready: false, reason: "无法确认重启后的守护进程输入状态，请重新检测。",
  });
  assert.equal(assessMacosInputRecovery(null, freshStatus).ready, false);
});

test("keeps recovery open when posting input remains unavailable even with a healthy backend", () => {
  assert.deepEqual(assessMacosInputRecovery({
    supported: true, input_monitoring: true, accessibility: false,
  }, {
    input_mode: "MacosNative", backend_health: "Healthy",
  }), {
    ready: false,
    reason: "输入注入能力不可用。请确认当前 R-ShareMouse.app 的辅助功能授权后重启服务。",
  });
});
