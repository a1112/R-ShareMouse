import test from "node:test";
import assert from "node:assert/strict";

test("buildMonitorContextMenuModel exposes copy and local display actions", async () => {
  const { buildMonitorContextMenuModel } = await import("./monitor-context-menu.mjs");

  const items = buildMonitorContextMenuModel({
    id: "local-display",
    displayId: "DISPLAY1",
    deviceKind: "local",
    name: "GX217UR (DISPLAY1)",
    resWidth: 2160,
    resHeight: 3840,
  });
  const byId = new Map(items.map((item) => [item.id, item]));

  assert.equal(byId.get("copy-name")?.copyText, "GX217UR (DISPLAY1)");
  assert.equal(byId.get("copy-id")?.copyText, "DISPLAY1");
  assert.equal(byId.get("copy-resolution")?.copyText, "2160x3840");
  assert.equal(byId.get("change-resolution")?.enabled, true);
  assert.equal(byId.get("change-orientation")?.enabled, true);
  assert.equal(byId.get("change-scale")?.enabled, true);
  assert.equal(byId.get("edit-position")?.enabled, true);
});

test("buildMonitorContextMenuModel keeps remote display settings actions disabled", async () => {
  const { buildMonitorContextMenuModel } = await import("./monitor-context-menu.mjs");

  const items = buildMonitorContextMenuModel({
    id: "remote-display",
    displayId: "primary",
    deviceKind: "remote",
    name: "Remote screen",
    resWidth: 1920,
    resHeight: 1080,
  });
  const byId = new Map(items.map((item) => [item.id, item]));

  assert.equal(byId.get("copy-name")?.enabled, true);
  assert.equal(byId.get("open-display-settings")?.enabled, false);
  assert.equal(byId.get("change-resolution")?.enabled, false);
  assert.match(byId.get("change-resolution")?.reason, /本机显示器/);
  assert.equal(byId.get("edit-position")?.enabled, true);
});
