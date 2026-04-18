import test from "node:test";
import assert from "node:assert/strict";

import { buildPageChrome, FIGMA_DESKTOP_THEME } from "./desktop-theme.mjs";

test("buildPageChrome makes layout full-bleed so the figma canvas becomes the main surface", () => {
  const chrome = buildPageChrome("layout");

  assert.equal(chrome.fullBleed, true);
  assert.equal(chrome.contentPadding, 0);
  assert.equal(chrome.surface, FIGMA_DESKTOP_THEME.frame);
});

test("buildPageChrome keeps devices and settings on the shared figma panel theme", () => {
  const devicesChrome = buildPageChrome("devices");
  const settingsChrome = buildPageChrome("settings");

  assert.equal(devicesChrome.fullBleed, false);
  assert.equal(settingsChrome.fullBleed, false);
  assert.equal(devicesChrome.contentPadding, 20);
  assert.equal(settingsChrome.panel, FIGMA_DESKTOP_THEME.sidebar);
  assert.equal(devicesChrome.border, FIGMA_DESKTOP_THEME.border);
});
