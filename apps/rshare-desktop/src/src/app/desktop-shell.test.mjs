import test from "node:test";
import assert from "node:assert/strict";

import {
  buildFooterStatus,
  getHeaderMetrics,
  getPageLabels,
  getThemeModeOptions,
} from "./desktop-shell.mjs";

test("getPageLabels defaults the titlebar tabs to Chinese", () => {
  assert.deepEqual(getPageLabels(), [
    { key: "layout", label: "布局" },
    { key: "devices", label: "设备" },
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

test("getHeaderMetrics tightens titlebar padding and button density", () => {
  assert.deepEqual(getHeaderMetrics(), {
    headerPaddingX: 12,
    brandGap: 8,
    navGap: 4,
    navButtonPaddingX: 10,
    navButtonPaddingY: 4,
    actionGap: 6,
    actionButtonPaddingX: 10,
    actionButtonPaddingY: 4,
    windowGap: 0,
    windowButtonSize: 32,
  });
});
