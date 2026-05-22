import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  buildHardwareAssetChoices,
  normalizeHardwareAssetManifest,
  resolveActiveHardwareRegions,
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
