import test from "node:test";
import assert from "node:assert/strict";

import {
  getDesktopPlatform,
  getDesktopShellState,
  getTitlebarDragRegionAttributes,
  isTauriDragRegionPath,
} from "./window-platform.mjs";

test("desktop platform detection is safe without browser globals", () => {
  assert.equal(getDesktopPlatform(null), "unknown");
  assert.deepEqual(getDesktopShellState(null), {
    platform: "unknown",
    isMacOS: false,
    rootClassName: "rshare-desktop-shell--standard",
    dataDesktopPlatform: "unknown",
    dataMacosFrameless: "false",
  });
});

test("desktop platform detection prefers the navigator platform exposed by WebKit", () => {
  assert.equal(getDesktopPlatform({ platform: "MacIntel" }), "macos");
  assert.equal(getDesktopPlatform({ userAgentData: { platform: "Windows" } }), "windows");
  assert.equal(getDesktopPlatform({ userAgent: "X11; Linux x86_64" }), "linux");
});

test("macOS shell state enables only the explicit frameless platform marker", () => {
  assert.deepEqual(getDesktopShellState({ platform: "MacIntel" }), {
    platform: "macos",
    isMacOS: true,
    rootClassName: "rshare-desktop-shell--macos",
    dataDesktopPlatform: "macos",
    dataMacosFrameless: "true",
  });
});

test("titlebar attributes use Tauri's direct, deep, and disabled drag contract", () => {
  assert.deepEqual(getTitlebarDragRegionAttributes(), {
    root: "true",
    blank: "deep",
    interactive: "false",
  });
  assert.equal(isTauriDragRegionPath([{ dragRegion: "" }]), true);
  assert.equal(isTauriDragRegionPath([{ dragRegion: "true" }]), true);
  assert.equal(
    isTauriDragRegionPath([{ clickable: false }, { dragRegion: "true" }]),
    false,
  );
  assert.equal(
    isTauriDragRegionPath([
      { clickable: false },
      { clickable: false },
      { dragRegion: "deep" },
    ]),
    true,
  );
  assert.equal(
    isTauriDragRegionPath([{ dragRegion: "false" }, { dragRegion: "deep" }]),
    false,
  );
  assert.equal(
    isTauriDragRegionPath([{ clickable: true }, { dragRegion: "deep" }]),
    false,
  );
});
