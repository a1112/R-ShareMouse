import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  BUILTIN_HARDWARE_ASSET_MANIFESTS,
  buildHardwareAssetChoices,
  normalizeHardwareAssetManifest,
  resolveActiveHardwareRegions,
  resolveSelectedHardwareAsset,
} from "./hardware-assets.mjs";

const keyboardManifest = {
  schema_version: 1,
  id: "builtin.keyboard.office",
  name: "Office Keyboard",
  kind: "keyboard",
  base_size: { width: 1000, height: 300 },
  layers: [{ id: "base", role: "base", src: "base.png" }],
  regions: [
    {
      id: "key.a",
      label: "A",
      action: { kind: "keyboard_key", codes: ["Char(65)", "Raw(65)"] },
      shape: { kind: "rect", x: 0.1, y: 0.2, w: 0.05, h: 0.1 },
    },
  ],
};

test("built-in hardware manifests include gamepad", () => {
  assert.ok(
    BUILTIN_HARDWARE_ASSET_MANIFESTS.includes(
      "/assets/hardware/live2d/gamepad/manifest.json",
    ),
  );
});

test("normalizeHardwareAssetManifest resolves relative layer urls", () => {
  const asset = normalizeHardwareAssetManifest(
    keyboardManifest,
    "/assets/hardware/keyboard/",
  );

  assert.equal(asset.id, "builtin.keyboard.office");
  assert.equal(asset.baseSize.width, 1000);
  assert.equal(asset.layers[0].src, "/assets/hardware/keyboard/base.png");
});

test("normalizeHardwareAssetManifest accepts a custom relative url resolver", () => {
  const asset = normalizeHardwareAssetManifest(keyboardManifest, {
    resolveUrl: (src) => `asset://local/${src}`,
  });

  assert.equal(asset.layers[0].src, "asset://local/base.png");
});

test("resolveActiveHardwareRegions matches pressed keyboard codes", () => {
  const asset = normalizeHardwareAssetManifest(
    keyboardManifest,
    "/assets/hardware/keyboard/",
  );
  const regions = resolveActiveHardwareRegions(asset, {
    pressedKeys: ["Char(65)"],
    lastKey: null,
    recentButtons: [],
  });

  assert.deepEqual(
    regions.map((region) => region.id),
    ["key.a"],
  );
});

test("buildHardwareAssetChoices groups assets by kind", () => {
  const asset = normalizeHardwareAssetManifest(
    keyboardManifest,
    "/assets/hardware/keyboard/",
  );
  const choices = buildHardwareAssetChoices([asset]);

  assert.deepEqual(
    choices.keyboard.map((choice) => [choice.id, choice.name]),
    [["builtin.keyboard.office", "Office Keyboard"]],
  );
});

test("buildHardwareAssetChoices groups gamepad assets", () => {
  const gamepad = normalizeHardwareAssetManifest({
    schema_version: 1,
    id: "builtin.gamepad.xbox",
    name: "Xbox Style Controller",
    kind: "gamepad",
    base_size: { width: 1205, height: 826 },
    layers: [{ id: "base", role: "base", src: "base.png" }],
    regions: [],
  });
  const choices = buildHardwareAssetChoices([gamepad]);

  assert.deepEqual(
    choices.gamepad.map((choice) => choice.id),
    ["builtin.gamepad.xbox"],
  );
});

test("checked-in office mouse manifest has mapped button regions", () => {
  const raw = JSON.parse(
    readFileSync(
      new URL(
        "../../public/assets/hardware/live2d/mouse/manifest.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  const asset = normalizeHardwareAssetManifest(
    raw,
    "/assets/hardware/live2d/mouse/",
  );

  assert.equal(asset.id, "builtin.mouse.office");
  assert.ok(asset.regions.some((region) => region.id === "mouse.left"));
  assert.equal(
    asset.regions.find((region) => region.id === "mouse.left").action.kind,
    "mouse_button",
  );
});

test("checked-in xbox style gamepad manifest has mapped button regions", () => {
  const raw = JSON.parse(
    readFileSync(
      new URL(
        "../../public/assets/hardware/live2d/gamepad/manifest.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  const asset = normalizeHardwareAssetManifest(
    raw,
    "/assets/hardware/live2d/gamepad/",
  );

  assert.equal(asset.id, "builtin.gamepad.xbox");
  assert.equal(asset.kind, "gamepad");
  assert.ok(asset.layers.some((layer) => layer.src.endsWith("/base.png")));
  const expectedRegionIds = [
    "gamepad.button.a",
    "gamepad.button.b",
    "gamepad.button.x",
    "gamepad.button.y",
    "gamepad.dpad.up",
    "gamepad.dpad.down",
    "gamepad.dpad.left",
    "gamepad.dpad.right",
    "gamepad.bumper.left",
    "gamepad.bumper.right",
    "gamepad.trigger.left",
    "gamepad.trigger.right",
    "gamepad.stick.left",
    "gamepad.stick.right",
    "gamepad.button.select",
    "gamepad.button.start",
    "gamepad.button.guide",
  ];
  const regionsById = new Map(
    asset.regions.map((region) => [region.id, region]),
  );

  assert.deepEqual(
    asset.regions.map((region) => region.id).sort(),
    [...expectedRegionIds].sort(),
  );
  for (const region of asset.regions) {
    assert.equal(region.action.kind, "gamepad_button");
  }

  const activeRegionIds = (pressedButtons) =>
    resolveActiveHardwareRegions(asset, { pressedButtons }).map(
      (region) => region.id,
    );

  assert.deepEqual(activeRegionIds(["South"]), ["gamepad.button.a"]);
  assert.deepEqual(activeRegionIds(["DPadUp"]), ["gamepad.dpad.up"]);
  assert.deepEqual(activeRegionIds(["LeftTrigger"]), ["gamepad.trigger.left"]);

  const regionCenter = (id) => {
    const region = regionsById.get(id);
    assert.ok(region, `missing ${id}`);
    if (region.shape.kind === "rect") {
      return {
        x: (region.shape.x + region.shape.w / 2) * asset.baseSize.width,
        y: (region.shape.y + region.shape.h / 2) * asset.baseSize.height,
      };
    }
    assert.equal(region.shape.kind, "polygon");
    return {
      x:
        (region.shape.points.reduce((sum, point) => sum + point.x, 0) /
          region.shape.points.length) *
        asset.baseSize.width,
      y:
        (region.shape.points.reduce((sum, point) => sum + point.y, 0) /
          region.shape.points.length) *
        asset.baseSize.height,
    };
  };
  const assertRegionCenter = (id, expected, tolerance = 34) => {
    const actual = regionCenter(id);
    assert.ok(
      Math.abs(actual.x - expected.x) <= tolerance,
      `${id} center x ${actual.x} differs from ${expected.x}`,
    );
    assert.ok(
      Math.abs(actual.y - expected.y) <= tolerance,
      `${id} center y ${actual.y} differs from ${expected.y}`,
    );
  };

  assertRegionCenter("gamepad.button.y", { x: 913, y: 188 });
  assertRegionCenter("gamepad.button.x", { x: 834, y: 263 });
  assertRegionCenter("gamepad.button.b", { x: 993, y: 263 });
  assertRegionCenter("gamepad.button.a", { x: 914, y: 339 });
  assertRegionCenter("gamepad.dpad.up", { x: 451, y: 388 });
  assertRegionCenter("gamepad.dpad.down", { x: 451, y: 506 });
  assertRegionCenter("gamepad.dpad.left", { x: 389, y: 447 });
  assertRegionCenter("gamepad.dpad.right", { x: 513, y: 447 });
  assertRegionCenter("gamepad.stick.left", { x: 287, y: 264 });
  assertRegionCenter("gamepad.stick.right", { x: 745, y: 443 });
  assertRegionCenter("gamepad.button.guide", { x: 606, y: 146 });
  assertRegionCenter("gamepad.button.select", { x: 459, y: 163 });
  assertRegionCenter("gamepad.button.start", { x: 747, y: 163 });
  assertRegionCenter("gamepad.bumper.left", { x: 303, y: 67 }, 46);
  assertRegionCenter("gamepad.bumper.right", { x: 902, y: 67 }, 46);
  assertRegionCenter("gamepad.trigger.left", { x: 300, y: 35 }, 46);
  assertRegionCenter("gamepad.trigger.right", { x: 905, y: 35 }, 46);
});

test("resolveActiveHardwareRegions matches mouse boolean button state", () => {
  const mouse = normalizeHardwareAssetManifest({
    schema_version: 1,
    id: "builtin.mouse.office",
    name: "Office Mouse",
    kind: "mouse",
    base_size: { width: 575, height: 1109 },
    layers: [{ id: "base", role: "base", src: "base.png" }],
    regions: [
      {
        id: "mouse.left",
        label: "Left",
        action: { kind: "mouse_button", buttons: ["Left"] },
        shape: { kind: "rect", x: 0.2, y: 0.07, w: 0.3, h: 0.4, radius: 38 },
      },
    ],
  });

  const active = resolveActiveHardwareRegions(mouse, { leftDown: true });

  assert.equal(active[0].id, "mouse.left");
  assert.equal(active[0].shape.radius, 38);
});

test("normalizeHardwareAssetManifest preserves polygon region points", () => {
  const asset = normalizeHardwareAssetManifest({
    schema_version: 1,
    id: "builtin.gamepad.xbox",
    name: "Xbox Style Controller",
    kind: "gamepad",
    base_size: { width: 1205, height: 826 },
    layers: [{ id: "base", role: "base", src: "base.png" }],
    regions: [
      {
        id: "gamepad.button.a",
        label: "A",
        action: { kind: "gamepad_button", buttons: ["A", "South"] },
        shape: {
          kind: "polygon",
          points: [
            { x: 0.74, y: 0.56 },
            { x: 0.77, y: 0.52 },
            { x: 0.8, y: 0.56 },
          ],
        },
      },
    ],
  });

  assert.equal(asset.kind, "gamepad");
  assert.equal(asset.regions[0].shape.kind, "polygon");
  assert.deepEqual(asset.regions[0].shape.points[1], { x: 0.77, y: 0.52 });
});

test("resolveActiveHardwareRegions returns polygon shapes for active buttons", () => {
  const asset = normalizeHardwareAssetManifest({
    schema_version: 1,
    id: "builtin.mouse.precise",
    name: "Precise Mouse",
    kind: "mouse",
    base_size: { width: 600, height: 1000 },
    layers: [{ id: "base", role: "base", src: "base.png" }],
    regions: [
      {
        id: "mouse.left",
        label: "Left",
        action: { kind: "mouse_button", buttons: ["Left"] },
        shape: {
          kind: "polygon",
          points: [
            { x: 0.2, y: 0.08 },
            { x: 0.48, y: 0.08 },
            { x: 0.43, y: 0.42 },
            { x: 0.23, y: 0.48 },
          ],
        },
      },
    ],
  });

  const active = resolveActiveHardwareRegions(asset, { leftDown: true });

  assert.equal(active[0].shape.kind, "polygon");
  assert.equal(active[0].shape.points.length, 4);
});

test("resolveActiveHardwareRegions matches gamepad button aliases", () => {
  const asset = normalizeHardwareAssetManifest({
    schema_version: 1,
    id: "builtin.gamepad.xbox",
    name: "Xbox Style Controller",
    kind: "gamepad",
    base_size: { width: 1205, height: 826 },
    layers: [{ id: "base", role: "base", src: "base.png" }],
    regions: [
      {
        id: "gamepad.button.a",
        label: "A",
        action: { kind: "gamepad_button", buttons: ["A", "South", "ButtonSouth"] },
        shape: { kind: "rect", x: 0.72, y: 0.52, w: 0.05, h: 0.06 },
      },
    ],
  });

  const active = resolveActiveHardwareRegions(asset, {
    pressedButtons: ["South"],
  });

  assert.deepEqual(active.map((region) => region.id), ["gamepad.button.a"]);
});

test("resolveSelectedHardwareAsset falls back to first matching kind", () => {
  const assets = [
    { id: "builtin.keyboard.office", kind: "keyboard", name: "Office" },
    { id: "builtin.keyboard.gaming", kind: "keyboard", name: "Gaming" },
  ];

  assert.equal(
    resolveSelectedHardwareAsset(assets, "keyboard", "missing").id,
    "builtin.keyboard.office",
  );
});

test("resolveSelectedHardwareAsset falls back to first matching gamepad", () => {
  const assets = [
    { id: "builtin.gamepad.xbox", kind: "gamepad", name: "Xbox Style Controller" },
  ];

  assert.equal(
    resolveSelectedHardwareAsset(assets, "gamepad", "missing").id,
    "builtin.gamepad.xbox",
  );
});
