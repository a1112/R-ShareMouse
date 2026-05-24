import test from "node:test";
import assert from "node:assert/strict";

import {
  buildFooterStatus,
  getDeviceConsoleSections,
  getDeviceSimulatorChrome,
  getHeaderMetrics,
  getHardwareAssetPresetOptions,
  getMouseDetailLayoutClasses,
  getMouseSimulatorLayoutClasses,
  getPageLabels,
  getSettingsLayoutSections,
  getThemeModeOptions,
  preventBrowserNavigationEvent,
  shouldPreventBrowserNavigationEvent,
} from "./desktop-shell.mjs";

test("getPageLabels defaults the titlebar tabs to Chinese", () => {
  assert.deepEqual(getPageLabels(), [
    { key: "layout", label: "布局" },
    { key: "devices", label: "设备" },
    { key: "logs", label: "日志" },
    { key: "settings", label: "设置" },
  ]);
});

test("buildFooterStatus moves daemon summary to the footer", () => {
  const footer = buildFooterStatus({
    service: {
      online: false,
      healthy: false,
      connectedDevices: 0,
      discoveredDevices: 0,
    },
    settings: {
      localDevice: {
        bindAddress: "不可用",
        name: "本机",
      },
    },
  });

  assert.equal(footer.summary, "守护进程离线，当前显示本机屏幕");
  assert.equal(footer.endpoint, "不可用");
});

test("buildFooterStatus reports connected and discovered counts in Chinese", () => {
  const footer = buildFooterStatus({
    service: {
      online: true,
      healthy: true,
      connectedDevices: 2,
      discoveredDevices: 3,
    },
    settings: {
      localDevice: {
        bindAddress: "192.168.1.10",
        name: "工作站",
      },
    },
  });

  assert.equal(footer.summary, "工作站 · 已连接 2 台，已发现 3 台");
  assert.equal(footer.endpoint, "192.168.1.10");
});

test("getThemeModeOptions exposes light dark and system in Chinese", () => {
  assert.deepEqual(getThemeModeOptions(), [
    { key: "light", label: "浅色" },
    { key: "dark", label: "深色" },
    { key: "system", label: "系统" },
  ]);
});

test("getHardwareAssetPresetOptions keeps hardware texture presets in settings copy", () => {
  assert.deepEqual(getHardwareAssetPresetOptions(), [
    { key: "office", label: "办公" },
    { key: "gaming", label: "游戏" },
  ]);
});

test("getDeviceConsoleSections hides the local latency feedback strip by default", () => {
  assert.deepEqual(getDeviceConsoleSections(), {
    endpointAcceptance: true,
    localLatencyFeedback: false,
  });
});

test("getDeviceSimulatorChrome keeps simulator devices texture first and unframed", () => {
  assert.deepEqual(getDeviceSimulatorChrome(), {
    textureFirst: true,
    deviceFrames: false,
    annotationFrames: false,
    frontFacingDisplays: true,
    displayWindowTexture: true,
  });
});

test("getMouseDetailLayoutClasses makes narrow mouse detail scroll instead of stacking", () => {
  const detail = getMouseDetailLayoutClasses();
  const simulator = getMouseSimulatorLayoutClasses();

  assert.ok(detail.root.split(" ").includes("overflow-auto"));
  assert.ok(detail.root.split(" ").includes("xl:overflow-hidden"));
  assert.ok(detail.previewPane.split(" ").includes("min-h-[760px]"));
  assert.ok(detail.sidePane.split(" ").includes("min-h-[220px]"));
  assert.ok(!simulator.root.split(" ").includes("h-full"));
  assert.ok(simulator.root.split(" ").includes("min-h-full"));
  assert.ok(simulator.pointerPad.split(" ").includes("min-h-[170px]"));
});

test("shouldPreventBrowserNavigationEvent blocks history mouse and key triggers", () => {
  assert.equal(shouldPreventBrowserNavigationEvent({ type: "mousedown", button: 3 }), true);
  assert.equal(shouldPreventBrowserNavigationEvent({ type: "mouseup", button: 4 }), true);
  assert.equal(shouldPreventBrowserNavigationEvent({ type: "mousedown", button: 0 }), false);
  assert.equal(shouldPreventBrowserNavigationEvent({ type: "keydown", key: "BrowserBack" }), true);
  assert.equal(shouldPreventBrowserNavigationEvent({ type: "keydown", key: "BrowserForward" }), true);
  assert.equal(shouldPreventBrowserNavigationEvent({ type: "keydown", key: "ArrowLeft", altKey: true }), true);
  assert.equal(shouldPreventBrowserNavigationEvent({ type: "keydown", key: "ArrowRight", altKey: true }), true);
  assert.equal(shouldPreventBrowserNavigationEvent({ type: "keydown", key: "ArrowLeft", altKey: false }), false);
});

test("preventBrowserNavigationEvent cancels only browser history triggers", () => {
  let cancelled = 0;
  const blocked = {
    type: "auxclick",
    button: 3,
    preventDefault() {
      cancelled += 1;
    },
  };

  assert.equal(preventBrowserNavigationEvent(blocked), true);
  assert.equal(cancelled, 1);
  assert.equal(preventBrowserNavigationEvent({ type: "mousedown", button: 0 }), false);
  assert.equal(cancelled, 1);
});

test("getSettingsLayoutSections exposes a left navigation order for settings", () => {
  assert.deepEqual(getSettingsLayoutSections(), [
    { key: "local", label: "本机信息", description: "设备名称、主机与监听端口" },
    { key: "service", label: "服务状态", description: "守护进程运行状态" },
    { key: "hardware", label: "硬件资产", description: "贴图和导入包" },
    { key: "input", label: "输入后端", description: "捕获模式与健康度" },
    { key: "appearance", label: "界面风格", description: "主题外观" },
    { key: "acceptance", label: "实机验收", description: "联机前检查项" },
  ]);
});

test("getHeaderMetrics tightens titlebar padding and button density", () => {
  assert.deepEqual(getHeaderMetrics(), {
    headerHeight: 40,
    headerPaddingX: 10,
    navGap: 4,
    navButtonPaddingX: 10,
    navButtonPaddingY: 3,
    actionGap: 6,
    actionButtonPaddingX: 10,
    actionButtonPaddingY: 3,
    windowGap: 0,
    windowButtonSize: 16,
    windowButtonHitSize: 46,
  });
});
