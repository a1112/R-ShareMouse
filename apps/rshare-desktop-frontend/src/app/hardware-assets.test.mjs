import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  BUILTIN_HARDWARE_ASSET_MANIFESTS,
  buildGamepadAnalogFeedback,
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

test("resolveActiveHardwareRegions does not keep released keyboard keys active", () => {
  const asset = normalizeHardwareAssetManifest(
    keyboardManifest,
    "/assets/hardware/keyboard/",
  );

  assert.deepEqual(
    resolveActiveHardwareRegions(asset, {
      pressedKeys: [],
      lastKey: "Char(65)",
      keyboardEvents: [
        {
          device_kind: "Keyboard",
          event_kind: "key",
          summary: "Key A Released",
          payload: { key: "KeyA", state: "Released" },
        },
      ],
    }).map((region) => region.id),
    [],
  );

  assert.deepEqual(
    resolveActiveHardwareRegions(asset, {
      pressedKeys: [],
      keyboardEvents: [
        {
          device_kind: "Keyboard",
          event_kind: "key",
          summary: "Key A Pressed",
          payload: { key: "KeyA", state: "Pressed" },
        },
      ],
    }).map((region) => region.id),
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

test("checked-in keyboard manifests align bottom-row key regions to artwork", () => {
  const cases = [
    {
      relative: "../../public/assets/hardware/live2d/keyboard/manifest.json",
      baseUrl: "/assets/hardware/live2d/keyboard/",
      expected: {
        "key.controlleft": { x: 39, y: 430, w: 87, h: 67 },
        "key.superleft": { x: 132, y: 430, w: 83, h: 67 },
        "key.altleft": { x: 223, y: 430, w: 83, h: 67 },
        "key.space": { x: 313, y: 430, w: 467, h: 67 },
        "key.altright": { x: 787, y: 430, w: 83, h: 67 },
        "key.superright": { x: 877, y: 430, w: 84, h: 67 },
        "key.raw.93": { x: 969, y: 430, w: 84, h: 67 },
        "key.controlright": { x: 1059, y: 430, w: 84, h: 67 },
      },
    },
    {
      relative: "../../public/assets/hardware/live2d/keyboard/gaming/manifest.json",
      baseUrl: "/assets/hardware/live2d/keyboard/gaming/",
      expected: {
        "key.controlleft": { x: 37, y: 424, w: 89, h: 68 },
        "key.superleft": { x: 130, y: 424, w: 87, h: 68 },
        "key.altleft": { x: 221, y: 424, w: 87, h: 68 },
        "key.space": { x: 312, y: 424, w: 444, h: 68 },
        "key.altright": { x: 760, y: 424, w: 86, h: 68 },
        "key.superright": { x: 850, y: 424, w: 87, h: 68 },
        "key.raw.93": { x: 941, y: 424, w: 83, h: 68 },
        "key.controlright": { x: 1028, y: 424, w: 91, h: 68 },
      },
    },
  ];

  for (const { relative, baseUrl, expected } of cases) {
    const raw = JSON.parse(readFileSync(new URL(relative, import.meta.url), "utf8"));
    const asset = normalizeHardwareAssetManifest(raw, baseUrl);
    const regionsById = new Map(
      asset.regions.map((region) => [region.id, region]),
    );

    for (const [id, expectedRect] of Object.entries(expected)) {
      const region = regionsById.get(id);
      assert.ok(region, `missing ${id} in ${relative}`);
      assert.equal(region.shape.kind, "rect", `${id} should use a rect shape`);
      const actual = {
        x: region.shape.x * asset.baseSize.width,
        y: region.shape.y * asset.baseSize.height,
        w: region.shape.w * asset.baseSize.width,
        h: region.shape.h * asset.baseSize.height,
      };
      for (const key of ["x", "y", "w", "h"]) {
        assert.ok(
          Math.abs(actual[key] - expectedRect[key]) <= 8,
          `${id} ${key} ${actual[key].toFixed(1)} differs from ${expectedRect[key]} in ${relative}`,
        );
      }
    }
  }
});

test("checked-in keyboard manifests align commonly tested key rows", () => {
  const cases = [
    {
      relative: "../../public/assets/hardware/live2d/keyboard/manifest.json",
      baseUrl: "/assets/hardware/live2d/keyboard/",
      expected: {
        "key.escape": { x: 38, y: 43, w: 72, h: 71 },
        "key.capslock": { x: 38, y: 277, w: 134, h: 72 },
        "key.shiftleft": { x: 38, y: 351, w: 175, h: 72 },
        "key.char.67": { x: 362, y: 351, w: 72, h: 72 },
        "key.char.86": { x: 436, y: 351, w: 72, h: 72 },
        "key.char.66": { x: 509, y: 351, w: 73, h: 72 },
        "key.char.78": { x: 584, y: 351, w: 72, h: 72 },
      },
    },
    {
      relative: "../../public/assets/hardware/live2d/keyboard/gaming/manifest.json",
      baseUrl: "/assets/hardware/live2d/keyboard/gaming/",
      expected: {
        "key.escape": { x: 38, y: 45, w: 72, h: 69 },
        "key.capslock": { x: 38, y: 277, w: 116, h: 58 },
        "key.shiftleft": { x: 38, y: 348, w: 152, h: 73 },
        "key.char.67": { x: 350, y: 348, w: 64, h: 73 },
        "key.char.86": { x: 422, y: 348, w: 64, h: 73 },
        "key.char.66": { x: 494, y: 348, w: 64, h: 73 },
        "key.char.78": { x: 566, y: 348, w: 64, h: 73 },
      },
    },
  ];

  for (const { relative, baseUrl, expected } of cases) {
    const raw = JSON.parse(readFileSync(new URL(relative, import.meta.url), "utf8"));
    const asset = normalizeHardwareAssetManifest(raw, baseUrl);
    const regionsById = new Map(
      asset.regions.map((region) => [region.id, region]),
    );
    for (const [id, expectedRect] of Object.entries(expected)) {
      const region = regionsById.get(id);
      assert.ok(region, `missing ${id} in ${relative}`);
      assert.equal(region.shape.kind, "rect", `${id} should use a rect shape`);
      const actual = {
        x: region.shape.x * asset.baseSize.width,
        y: region.shape.y * asset.baseSize.height,
        w: region.shape.w * asset.baseSize.width,
        h: region.shape.h * asset.baseSize.height,
      };
      for (const key of ["x", "y", "w", "h"]) {
        assert.ok(
          Math.abs(actual[key] - expectedRect[key]) <= 8,
          `${id} ${key} ${actual[key].toFixed(1)} differs from ${expectedRect[key]} in ${relative}`,
        );
      }
    }
  }
});

test("checked-in mouse manifests use precision polygon regions", () => {
  for (const relative of [
    "../../public/assets/hardware/live2d/mouse/manifest.json",
    "../../public/assets/hardware/live2d/mouse/gaming/manifest.json",
  ]) {
    const raw = JSON.parse(readFileSync(new URL(relative, import.meta.url), "utf8"));
    const asset = normalizeHardwareAssetManifest(
      raw,
      "/assets/hardware/live2d/mouse/",
    );
    const primary = asset.regions.filter((region) =>
      ["mouse.left", "mouse.right", "mouse.middle"].includes(region.id),
    );

    assert.equal(primary.length, 3);
    assert.ok(primary.every((region) => region.shape.kind === "polygon"));
    assert.ok(asset.regions.every((region) => region.shape.kind === "polygon"));
  }
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
  assert.deepEqual(activeRegionIds(["Cross"]), ["gamepad.button.a"]);
  assert.deepEqual(activeRegionIds(["Circle"]), ["gamepad.button.b"]);
  assert.deepEqual(activeRegionIds(["Square"]), ["gamepad.button.x"]);
  assert.deepEqual(activeRegionIds(["Triangle"]), ["gamepad.button.y"]);
  assert.deepEqual(activeRegionIds(["Share"]), ["gamepad.button.select"]);
  assert.deepEqual(activeRegionIds(["Options"]), ["gamepad.button.start"]);
  assert.deepEqual(activeRegionIds(["Xbox"]), ["gamepad.button.guide"]);
  assert.deepEqual(activeRegionIds(["LeftThumbstick"]), ["gamepad.stick.left"]);
  assert.deepEqual(activeRegionIds(["RightThumbstick"]), ["gamepad.stick.right"]);

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

test("resolveActiveHardwareRegions does not use released mouse buttons as active feedback", () => {
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
        shape: { kind: "rect", x: 0.2, y: 0.07, w: 0.3, h: 0.4 },
      },
    ],
  });

  assert.deepEqual(
    resolveActiveHardwareRegions(mouse, {
      pressedButtons: [],
      recentButtons: ["Left Released"],
    }).map((region) => region.id),
    [],
  );
  assert.deepEqual(
    resolveActiveHardwareRegions(mouse, {
      pressedButtons: ["Left"],
      recentButtons: ["Left Released"],
    }).map((region) => region.id),
    ["mouse.left"],
  );
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

test("buildGamepadAnalogFeedback normalizes trigger depth and stick offset", () => {
  const feedback = buildGamepadAnalogFeedback({
    pressedButtons: ["South"],
    leftTrigger: 16384,
    rightTrigger: 65535,
    leftStickX: 32767,
    leftStickY: -16384,
    rightStickX: -32767,
    rightStickY: 0,
  });

  assert.equal(feedback.leftTrigger.active, true);
  assert.equal(feedback.rightTrigger.value, 1);
  assert.ok(Math.abs(feedback.leftTrigger.value - 0.25) < 0.01);
  assert.equal(feedback.leftStick.x, 1);
  assert.ok(Math.abs(feedback.leftStick.y + 0.5) < 0.01);
  assert.equal(feedback.rightStick.x, -1);
  assert.deepEqual(
    feedback.pressedButtons.filter((button) =>
      ["South", "LeftTrigger", "RightTrigger", "LeftThumbstick", "RightThumbstick"].includes(button),
    ),
    ["South", "LeftTrigger", "RightTrigger", "LeftThumbstick", "RightThumbstick"],
  );
});

test("resolveActiveHardwareRegions matches gamepad analog trigger and stick movement", () => {
  const asset = normalizeHardwareAssetManifest({
    schema_version: 1,
    id: "builtin.gamepad.xbox",
    name: "Xbox Style Controller",
    kind: "gamepad",
    base_size: { width: 1205, height: 826 },
    layers: [{ id: "base", role: "base", src: "base.png" }],
    regions: [
      {
        id: "gamepad.trigger.right",
        label: "RT",
        action: { kind: "gamepad_button", buttons: ["RightTrigger", "RT"] },
        shape: { kind: "rect", x: 0.67, y: 0.02, w: 0.14, h: 0.05 },
      },
      {
        id: "gamepad.stick.left",
        label: "LS",
        action: { kind: "gamepad_button", buttons: ["LeftThumbstick", "LS"] },
        shape: { kind: "rect", x: 0.18, y: 0.23, w: 0.12, h: 0.17 },
      },
    ],
  });

  const active = resolveActiveHardwareRegions(asset, {
    rightTrigger: 42000,
    leftStickX: 12000,
  });

  assert.deepEqual(
    active.map((region) => region.id),
    ["gamepad.trigger.right", "gamepad.stick.left"],
  );
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
