import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
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
