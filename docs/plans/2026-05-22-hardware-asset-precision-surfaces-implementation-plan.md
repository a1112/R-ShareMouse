# Hardware Asset Precision Surfaces Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the hardcoded gamepad UI with a built-in Xbox-style manifest asset and add precise, replaceable keyboard/mouse/gamepad region surfaces.

**Architecture:** Keep `rshare-core` as the schema authority, extend the React asset catalog to include gamepads, render all hardware rigs through `HardwareAssetManifest`, and use polygon regions as the first precision surface while preserving mask metadata for later raster clipping.

**Tech Stack:** Rust 2021, serde/serde_json, React 18, TypeScript, Vite, Node `node:test`, OpenAI image generation CLI, Pillow for chroma-key cleanup.

---

### Task 1: Add Core Gamepad And Polygon Contract Coverage

**Files:**
- Modify: `crates/rshare-core/tests/hardware_asset_manifest_contract.rs`

**Step 1: Write the failing test**

Append this test:

```rust
#[test]
fn parses_valid_gamepad_manifest_with_polygon_regions() {
    let manifest: HardwareAssetManifest = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "id": "builtin.gamepad.xbox",
        "name": "Xbox Style Controller",
        "kind": "gamepad",
        "base_size": { "width": 1205, "height": 826 },
        "layers": [
            { "id": "base", "role": "base", "src": "base.png" }
        ],
        "regions": [
            {
                "id": "gamepad.button.a",
                "label": "A",
                "action": {
                    "kind": "gamepad_button",
                    "buttons": ["A", "South", "ButtonSouth"]
                },
                "shape": {
                    "kind": "polygon",
                    "points": [
                        { "x": 0.74, "y": 0.56 },
                        { "x": 0.77, "y": 0.52 },
                        { "x": 0.80, "y": 0.56 },
                        { "x": 0.77, "y": 0.60 }
                    ]
                }
            }
        ],
        "mask": {
            "src": "mask.png",
            "channels": [{ "value": 32, "region_id": "gamepad.button.a" }]
        }
    }))
    .unwrap();

    assert_eq!(manifest.kind, HardwareAssetKind::Gamepad);
    assert!(matches!(
        manifest.regions[0].action,
        HardwareControlAction::GamepadButton { .. }
    ));
    assert!(matches!(
        manifest.regions[0].shape,
        HardwareRegionShape::Polygon { .. }
    ));
    manifest.validate().unwrap();
    assert_eq!(manifest.referenced_paths(), vec!["base.png", "mask.png"]);
}
```

**Step 2: Run the test to verify it fails or proves current support**

Run:

```powershell
cargo test -p rshare-core hardware_asset_manifest_contract
```

Expected: this may already pass because `GamepadButton` and `Polygon` exist. If it passes immediately, keep it as coverage for the new built-in gamepad manifest contract.

**Step 3: Implement only if needed**

If the test fails, update `crates/rshare-core/src/hardware_assets.rs` minimally so:

- `HardwareAssetKind::Gamepad` deserializes from `"gamepad"`.
- `HardwareControlAction::GamepadButton` deserializes from `"gamepad_button"`.
- `HardwareRegionShape::Polygon` validates at least three normalized points.

**Step 4: Run the test**

Run:

```powershell
cargo test -p rshare-core hardware_asset_manifest_contract
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add crates\rshare-core\tests\hardware_asset_manifest_contract.rs crates\rshare-core\src\hardware_assets.rs
git commit -m "Cover gamepad hardware asset manifests"
```

---

### Task 2: Extend Frontend Asset Helpers For Gamepad And Polygon Shapes

**Files:**
- Modify: `other/figma-ui/src/app/hardware-assets.mjs`
- Modify: `other/figma-ui/src/app/hardware-assets.test.mjs`

**Step 1: Write failing helper tests**

Add these tests:

```js
test("built-in hardware manifests include gamepad", () => {
  assert.ok(
    BUILTIN_HARDWARE_ASSET_MANIFESTS.includes(
      "/assets/hardware/live2d/gamepad/manifest.json",
    ),
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
```

**Step 2: Run the tests to verify failure**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/hardware-assets.test.mjs
Pop-Location
```

Expected: FAIL because the built-in list does not include gamepad and polygon shape normalization is not explicit.

**Step 3: Implement helper changes**

In `other/figma-ui/src/app/hardware-assets.mjs`:

- Add `"/assets/hardware/live2d/gamepad/manifest.json"` to `BUILTIN_HARDWARE_ASSET_MANIFESTS`.
- Replace `shape: region.shape ?? legacyRectShape(region)` in `normalizeRegion()` with:

```js
shape: normalizeShape(region.shape ?? legacyRectShape(region)),
```

- Add:

```js
function normalizeShape(shape) {
  if (shape?.kind === "polygon") {
    return {
      kind: "polygon",
      points: (shape.points ?? []).map((point) => ({
        x: Number(point.x ?? 0),
        y: Number(point.y ?? 0),
      })),
    };
  }
  return {
    kind: "rect",
    x: Number(shape?.x ?? 0),
    y: Number(shape?.y ?? 0),
    w: Number(shape?.w ?? 0),
    h: Number(shape?.h ?? 0),
    radius: Number(shape?.radius ?? 7),
  };
}
```

- Keep `gamepadActionMatches()` as the matching path for `gamepad_button`; if alias coverage is too narrow, extend the manifest button arrays rather than adding UI-only truth.

**Step 4: Run the tests**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/hardware-assets.test.mjs
Pop-Location
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add other\figma-ui\src\app\hardware-assets.mjs other\figma-ui\src\app\hardware-assets.test.mjs
git commit -m "Support gamepad hardware asset helpers"
```

---

### Task 3: Add Built-In Gamepad Manifest

**Files:**
- Create: `other/figma-ui/public/assets/hardware/live2d/gamepad/manifest.json`
- Modify: `other/figma-ui/src/app/hardware-assets.test.mjs`

**Step 1: Write the failing checked-in manifest test**

Add:

```js
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
  assert.ok(asset.regions.some((region) => region.id === "gamepad.button.a"));
  assert.ok(asset.regions.some((region) => region.id === "gamepad.dpad.up"));
  assert.ok(asset.regions.some((region) => region.id === "gamepad.trigger.left"));
});
```

**Step 2: Run the test to verify failure**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/hardware-assets.test.mjs
Pop-Location
```

Expected: FAIL because `gamepad/manifest.json` does not exist.

**Step 3: Create the manifest**

Create `other/figma-ui/public/assets/hardware/live2d/gamepad/manifest.json`:

```json
{
  "schema_version": 1,
  "id": "builtin.gamepad.xbox",
  "name": "Xbox Style Controller",
  "kind": "gamepad",
  "base_size": {
    "width": 1205,
    "height": 826
  },
  "layers": [
    {
      "id": "base",
      "role": "base",
      "render": "image",
      "src": "base.png"
    },
    {
      "id": "press-effect",
      "role": "pressEffect",
      "render": "runtime"
    }
  ],
  "regions": [
    {
      "id": "gamepad.button.a",
      "label": "A",
      "action": { "kind": "gamepad_button", "buttons": ["A", "South", "ButtonSouth", "ButtonA"] },
      "shape": { "kind": "rect", "x": 0.744, "y": 0.524, "w": 0.052, "h": 0.074, "radius": 999 }
    },
    {
      "id": "gamepad.button.b",
      "label": "B",
      "action": { "kind": "gamepad_button", "buttons": ["B", "East", "ButtonEast", "ButtonB"] },
      "shape": { "kind": "rect", "x": 0.797, "y": 0.466, "w": 0.052, "h": 0.074, "radius": 999 }
    },
    {
      "id": "gamepad.button.x",
      "label": "X",
      "action": { "kind": "gamepad_button", "buttons": ["X", "West", "ButtonWest", "ButtonX"] },
      "shape": { "kind": "rect", "x": 0.691, "y": 0.466, "w": 0.052, "h": 0.074, "radius": 999 }
    },
    {
      "id": "gamepad.button.y",
      "label": "Y",
      "action": { "kind": "gamepad_button", "buttons": ["Y", "North", "ButtonNorth", "ButtonY"] },
      "shape": { "kind": "rect", "x": 0.744, "y": 0.407, "w": 0.052, "h": 0.074, "radius": 999 }
    },
    {
      "id": "gamepad.dpad.up",
      "label": "Up",
      "action": { "kind": "gamepad_button", "buttons": ["DPadUp", "DpadUp", "Up"] },
      "shape": { "kind": "polygon", "points": [{ "x": 0.259, "y": 0.548 }, { "x": 0.304, "y": 0.548 }, { "x": 0.304, "y": 0.611 }, { "x": 0.259, "y": 0.611 }] }
    },
    {
      "id": "gamepad.dpad.down",
      "label": "Down",
      "action": { "kind": "gamepad_button", "buttons": ["DPadDown", "DpadDown", "Down"] },
      "shape": { "kind": "polygon", "points": [{ "x": 0.259, "y": 0.685 }, { "x": 0.304, "y": 0.685 }, { "x": 0.304, "y": 0.748 }, { "x": 0.259, "y": 0.748 }] }
    },
    {
      "id": "gamepad.dpad.left",
      "label": "Left",
      "action": { "kind": "gamepad_button", "buttons": ["DPadLeft", "DpadLeft", "Left"] },
      "shape": { "kind": "polygon", "points": [{ "x": 0.216, "y": 0.620 }, { "x": 0.272, "y": 0.620 }, { "x": 0.272, "y": 0.680 }, { "x": 0.216, "y": 0.680 }] }
    },
    {
      "id": "gamepad.dpad.right",
      "label": "Right",
      "action": { "kind": "gamepad_button", "buttons": ["DPadRight", "DpadRight", "Right"] },
      "shape": { "kind": "polygon", "points": [{ "x": 0.292, "y": 0.620 }, { "x": 0.348, "y": 0.620 }, { "x": 0.348, "y": 0.680 }, { "x": 0.292, "y": 0.680 }] }
    },
    {
      "id": "gamepad.bumper.left",
      "label": "LB",
      "action": { "kind": "gamepad_button", "buttons": ["LeftBumper", "LeftShoulder", "LB", "L1"] },
      "shape": { "kind": "polygon", "points": [{ "x": 0.213, "y": 0.118 }, { "x": 0.414, "y": 0.118 }, { "x": 0.394, "y": 0.176 }, { "x": 0.233, "y": 0.176 }] }
    },
    {
      "id": "gamepad.bumper.right",
      "label": "RB",
      "action": { "kind": "gamepad_button", "buttons": ["RightBumper", "RightShoulder", "RB", "R1"] },
      "shape": { "kind": "polygon", "points": [{ "x": 0.586, "y": 0.118 }, { "x": 0.787, "y": 0.118 }, { "x": 0.767, "y": 0.176 }, { "x": 0.606, "y": 0.176 }] }
    },
    {
      "id": "gamepad.trigger.left",
      "label": "LT",
      "action": { "kind": "gamepad_button", "buttons": ["LeftTrigger", "LeftTrigger2", "LT", "L2"] },
      "shape": { "kind": "polygon", "points": [{ "x": 0.236, "y": 0.044 }, { "x": 0.394, "y": 0.044 }, { "x": 0.390, "y": 0.101 }, { "x": 0.239, "y": 0.101 }] }
    },
    {
      "id": "gamepad.trigger.right",
      "label": "RT",
      "action": { "kind": "gamepad_button", "buttons": ["RightTrigger", "RightTrigger2", "RT", "R2"] },
      "shape": { "kind": "polygon", "points": [{ "x": 0.606, "y": 0.044 }, { "x": 0.764, "y": 0.044 }, { "x": 0.761, "y": 0.101 }, { "x": 0.610, "y": 0.101 }] }
    },
    {
      "id": "gamepad.stick.left",
      "label": "LS",
      "action": { "kind": "gamepad_button", "buttons": ["LeftStick", "LeftThumb", "LS", "L3"] },
      "shape": { "kind": "rect", "x": 0.238, "y": 0.374, "w": 0.092, "h": 0.134, "radius": 999 }
    },
    {
      "id": "gamepad.stick.right",
      "label": "RS",
      "action": { "kind": "gamepad_button", "buttons": ["RightStick", "RightThumb", "RS", "R3"] },
      "shape": { "kind": "rect", "x": 0.553, "y": 0.622, "w": 0.092, "h": 0.134, "radius": 999 }
    },
    {
      "id": "gamepad.button.select",
      "label": "Select",
      "action": { "kind": "gamepad_button", "buttons": ["Select", "Back", "View"] },
      "shape": { "kind": "rect", "x": 0.397, "y": 0.432, "w": 0.054, "h": 0.038, "radius": 18 }
    },
    {
      "id": "gamepad.button.start",
      "label": "Start",
      "action": { "kind": "gamepad_button", "buttons": ["Start", "Menu"] },
      "shape": { "kind": "rect", "x": 0.549, "y": 0.432, "w": 0.054, "h": 0.038, "radius": 18 }
    },
    {
      "id": "gamepad.button.guide",
      "label": "Guide",
      "action": { "kind": "gamepad_button", "buttons": ["Guide", "Mode", "Home"] },
      "shape": { "kind": "rect", "x": 0.472, "y": 0.469, "w": 0.055, "h": 0.081, "radius": 999 }
    }
  ]
}
```

**Step 4: Run helper tests**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/hardware-assets.test.mjs
Pop-Location
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add other\figma-ui\public\assets\hardware\live2d\gamepad\manifest.json other\figma-ui\src\app\hardware-assets.test.mjs
git commit -m "Add builtin gamepad hardware manifest"
```

---

### Task 4: Generate And Clean The Built-In Xbox-Style Gamepad Image

**Files:**
- Modify: `other/figma-ui/public/assets/hardware/live2d/gamepad/base.png`

**Step 1: Check image generation prerequisites**

Run:

```powershell
if ($env:OPENAI_API_KEY) { "OPENAI_API_KEY is set" } else { "OPENAI_API_KEY is missing" }
```

Expected: key is set before live generation. If missing, stop this task and ask the user to set `OPENAI_API_KEY` locally.

**Step 2: Generate the green-background source image**

Run:

```powershell
python C:\Users\10428\.codex\skills\imagegen\scripts\image_gen.py generate `
  --prompt "Use case: product-mockup. Asset type: desktop hardware visualizer. Primary request: a realistic Xbox-style game controller, no official logo, no brand marks, no text, front-facing top-down product render. Scene/background: flat pure chroma green background (#00ff00) for background removal. Subject: dark graphite wireless game controller with Xbox-style A B X Y buttons, D-pad, thumb sticks, bumpers, triggers, start/select/menu cluster, clean plastic texture. Composition/framing: centered full controller, entire silhouette visible, no crop, orthographic top view, generous transparent-ready margin. Lighting/mood: soft studio light, subtle shadow under device only if separable from green. Constraints: no logos, no text, no watermark, no hands, no cables, no packaging." `
  --size 1536x1024 `
  --quality high `
  --out output\imagegen\rshare-gamepad-xbox-green.png
```

Expected: `output/imagegen/rshare-gamepad-xbox-green.png` is created.

**Step 3: Remove the green background**

Run this Pillow chroma-key cleanup:

```powershell
@'
from pathlib import Path
from PIL import Image

src = Path("output/imagegen/rshare-gamepad-xbox-green.png")
dst = Path("other/figma-ui/public/assets/hardware/live2d/gamepad/base.png")
img = Image.open(src).convert("RGBA")
pixels = img.load()
for y in range(img.height):
    for x in range(img.width):
        r, g, b, a = pixels[x, y]
        green_dominant = g > 120 and g > r * 1.35 and g > b * 1.35
        near_chroma = g > 180 and r < 110 and b < 130
        if green_dominant or near_chroma:
            pixels[x, y] = (r, g, b, 0)
        elif g > r and g > b:
            spill = min(35, max(0, g - max(r, b)))
            pixels[x, y] = (r, max(0, g - spill), b, a)
dst.parent.mkdir(parents=True, exist_ok=True)
img.save(dst)
'@ | python -
```

Expected: `other/figma-ui/public/assets/hardware/live2d/gamepad/base.png` has transparent background.

**Step 4: Verify image dimensions and alpha**

Run:

```powershell
@'
from pathlib import Path
from PIL import Image

path = Path("other/figma-ui/public/assets/hardware/live2d/gamepad/base.png")
img = Image.open(path).convert("RGBA")
alpha_values = img.getchannel("A").getextrema()
print(f"{img.width}x{img.height} alpha={alpha_values}")
assert alpha_values[0] == 0, "expected some transparent pixels"
assert alpha_values[1] == 255, "expected opaque controller pixels"
'@ | python -
```

Expected: command prints dimensions and alpha range `(0, 255)`.

**Step 5: Update manifest base size if needed**

If the generated image dimensions are not `1205x826`, update `other/figma-ui/public/assets/hardware/live2d/gamepad/manifest.json` `base_size` to match the printed dimensions.

**Step 6: Commit**

```powershell
git add other\figma-ui\public\assets\hardware\live2d\gamepad\base.png other\figma-ui\public\assets\hardware\live2d\gamepad\manifest.json
git commit -m "Replace builtin gamepad image asset"
```

---

### Task 5: Render Polygon Region Overlays

**Files:**
- Modify: `other/figma-ui/src/app/App.tsx`
- Modify: `other/figma-ui/src/app/hardware-assets.test.mjs`

**Step 1: Add a failing helper test for drawable polygon active regions**

Add:

```js
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
```

**Step 2: Run tests**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/hardware-assets.test.mjs
Pop-Location
```

Expected: PASS after Task 2. This test locks the active-region behavior before the renderer changes.

**Step 3: Update TypeScript types**

In `other/figma-ui/src/app/App.tsx`:

- Change:

```ts
type HardwareRigKind = "keyboard" | "mouse";
```

to:

```ts
type HardwareRigKind = "keyboard" | "mouse" | "gamepad";
```

- Change `HardwareRigRegion["shape"]` to a union:

```ts
type HardwareRigRegionShape =
  | {
      kind: "rect";
      x: number;
      y: number;
      w: number;
      h: number;
      radius?: number;
    }
  | {
      kind: "polygon";
      points: Array<{ x: number; y: number }>;
    };
```

Then use:

```ts
shape: HardwareRigRegionShape;
```

**Step 4: Add a polygon overlay component**

Add near `HardwareHotspotOverlay`:

```tsx
function HardwarePolygonOverlay({
  points,
  active,
  label,
  accent,
  theme,
  compact = false,
}: {
  points: Array<{ x: number; y: number }>;
  active: boolean;
  label?: string;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact?: boolean;
}) {
  if (!active || points.length < 3) {
    return null;
  }
  const polygon = points
    .map((point) => `${point.x * 100}% ${point.y * 100}%`)
    .join(", ");
  const center = points.reduce(
    (sum, point) => ({ x: sum.x + point.x, y: sum.y + point.y }),
    { x: 0, y: 0 },
  );
  const centerX = (center.x / points.length) * 100;
  const centerY = (center.y / points.length) * 100;

  return (
    <span
      className="pointer-events-none absolute inset-0 hardware-press-flash transition-all duration-75"
      style={{
        clipPath: `polygon(${polygon})`,
        background: `radial-gradient(circle at ${centerX}% ${centerY}%, rgba(255,255,255,0.62), ${accent}cc 28%, ${accent}66 66%, transparent 100%)`,
        border: `1px solid ${accent}`,
        color: "#ffffff",
        boxShadow: `0 0 28px ${accent}aa`,
        mixBlendMode: "screen",
      }}
    >
      {label ? (
        <span
          className="absolute hardware-legend-glow text-[10px] font-semibold"
          style={{
            left: `${centerX}%`,
            top: `${centerY}%`,
            transform: "translate(-50%, -50%)",
            color: "#ffffff",
            fontSize: compact ? 8 : 10,
            textShadow: "0 0 8px rgba(255,255,255,0.75)",
          }}
        >
          {label}
        </span>
      ) : null}
    </span>
  );
}
```

**Step 5: Render polygon regions**

In `HardwareRigRegionOverlays`, replace the `shape.kind !== "rect"` early return with:

```tsx
if (shape.kind === "polygon") {
  return (
    <HardwarePolygonOverlay
      key={region.id}
      points={shape.points}
      active
      label={hardwareRegionLabel(region, activity, compact)}
      accent={accent}
      theme={theme}
      compact={compact}
    />
  );
}
if (shape.kind === "rect") {
  return (
    <HardwareHotspotOverlay
      key={region.id}
      x={shape.x}
      y={shape.y}
      w={shape.w}
      h={shape.h}
      radius={shape.radius ?? 7}
      active
      label={hardwareRegionLabel(region, activity, compact)}
      accent={accent}
      theme={theme}
      compact={compact}
    />
  );
}
return null;
```

**Step 6: Run tests and build**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/hardware-assets.test.mjs
npm run build
Pop-Location
```

Expected: PASS and successful Vite build.

**Step 7: Commit**

```powershell
git add other\figma-ui\src\app\App.tsx other\figma-ui\src\app\hardware-assets.test.mjs
git commit -m "Render polygon hardware asset regions"
```

---

### Task 6: Add Gamepad Asset Selection And Catalog Support

**Files:**
- Modify: `other/figma-ui/src/app/App.tsx`
- Modify: `other/figma-ui/src/app/hardware-assets.mjs`
- Modify: `other/figma-ui/src/app/hardware-assets.test.mjs`

**Step 1: Add failing helper tests for choices and selected gamepad fallback**

Add:

```js
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

  assert.deepEqual(choices.gamepad.map((choice) => choice.id), [
    "builtin.gamepad.xbox",
  ]);
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
```

**Step 2: Run helper tests**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/hardware-assets.test.mjs
Pop-Location
```

Expected: likely PASS for helper grouping, but this locks the behavior before TypeScript integration.

**Step 3: Add gamepad local storage and default selected id**

In `App.tsx`:

- Add:

```ts
const HARDWARE_ASSET_GAMEPAD_STORAGE_KEY = "rshare.hardwareAsset.gamepad";
```

- Change selected id types to include gamepad:

```ts
selectedIds: Record<HardwareRigKind, string>;
```

- Add default:

```ts
gamepad: "builtin.gamepad.xbox",
```

- Update `hardwareAssetStorageKey(kind)` with a `gamepad` branch.
- Update `loadSelectedHardwareAssetIds()` to read `HARDWARE_ASSET_GAMEPAD_STORAGE_KEY` and fall back to `"builtin.gamepad.xbox"`.

**Step 4: Add a gamepad fallback rig definition**

Extend `HARDWARE_RIGS` with:

```ts
gamepad: {
  office: {
    kind: "gamepad",
    manifest: "/assets/hardware/live2d/gamepad/manifest.json",
    baseSize: { width: 1205, height: 826 },
    display: {
      compactWidth: 280,
      compactHeight: 180,
      fullWidth: 720,
      fullHeight: 430,
    },
    layers: [
      {
        id: "gamepad-base",
        role: "base",
        render: "image",
        src: "/assets/hardware/live2d/gamepad/base.png",
      },
      {
        id: "gamepad-press-effect",
        role: "pressEffect",
        render: "runtime",
      },
    ],
  },
  gaming: {
    kind: "gamepad",
    manifest: "/assets/hardware/live2d/gamepad/manifest.json",
    baseSize: { width: 1205, height: 826 },
    display: {
      compactWidth: 280,
      compactHeight: 180,
      fullWidth: 720,
      fullHeight: 430,
    },
    layers: [
      {
        id: "gamepad-base",
        role: "base",
        render: "image",
        src: "/assets/hardware/live2d/gamepad/base.png",
      },
      {
        id: "gamepad-press-effect",
        role: "pressEffect",
        render: "runtime",
      },
    ],
  },
},
```

**Step 5: Accept gamepad manifests**

In `hardwareRigFromManifest()`, change:

```ts
if (manifest.kind !== "keyboard" && manifest.kind !== "mouse") {
  return null;
}
```

to:

```ts
if (
  manifest.kind !== "keyboard" &&
  manifest.kind !== "mouse" &&
  manifest.kind !== "gamepad"
) {
  return null;
}
```

**Step 6: Add settings UI select**

In `HardwareAssetSettingsPanel`:

- Resolve `selectedGamepad`.
- Change `renderSelect` options branch to use `choices.gamepad` for gamepad.
- Add:

```tsx
{renderSelect(
  "gamepad",
  "手柄资产",
  <Gamepad2 size={14} />,
  selectedGamepad,
)}
```

**Step 7: Keep variant toggle coherent**

In `setHardwareRigVariant`, also set:

```ts
setSelectedId("gamepad", "builtin.gamepad.xbox");
```

The office/gaming toggle remains a keyboard/mouse theme shortcut; gamepad uses one built-in for both.

**Step 8: Run tests and build**

Run:

```powershell
Push-Location other\figma-ui
npm test
npm run build
Pop-Location
```

Expected: PASS and successful Vite build.

**Step 9: Commit**

```powershell
git add other\figma-ui\src\app\App.tsx other\figma-ui\src\app\hardware-assets.mjs other\figma-ui\src\app\hardware-assets.test.mjs
git commit -m "Add gamepad hardware asset selection"
```

---

### Task 7: Replace Hardcoded Gamepad SVG With Manifest Rendering

**Files:**
- Modify: `other/figma-ui/src/app/App.tsx`

**Step 1: Write a build-level failing check**

Before editing, run:

```powershell
Push-Location other\figma-ui
npm run build
Pop-Location
```

Expected: PASS before the refactor. This is the safety baseline.

**Step 2: Simplify `SimulatedGamepad` visual body**

Keep telemetry calculations:

```ts
const connected = Boolean(gamepad?.connected);
const pressed = gamepadPressedButtons(gamepad);
const leftTrigger = triggerFill(gamepad?.left_trigger ?? 0);
const rightTrigger = triggerFill(gamepad?.right_trigger ?? 0);
```

Remove the hardcoded `<svg>` controller body and replace the visual area with:

```tsx
<div className="flex min-h-0 items-center justify-center">
  <HardwareRigView
    kind="gamepad"
    activity={{
      pressedButtons: [
        ...pressed,
        ...(leftTrigger > 2 ? ["LeftTrigger", "LT"] : []),
        ...(rightTrigger > 2 ? ["RightTrigger", "RT"] : []),
      ],
    }}
    accent={theme.accent}
    theme={theme}
    compact={compact}
  />
</div>
```

**Step 3: Preserve stats**

Keep the existing stats grid below the visual area:

- `已按下`
- `最近按键`
- `按下/抬起`
- `按键事件`
- `摇杆事件`
- `扳机事件`
- `总事件数`
- `摇杆`

Remove unused constants and helper closures from `SimulatedGamepad` after the SVG removal.

**Step 4: Run build**

Run:

```powershell
Push-Location other\figma-ui
npm run build
Pop-Location
```

Expected: PASS. If TypeScript reports unused variables, delete the now-unused locals from `SimulatedGamepad`.

**Step 5: Commit**

```powershell
git add other\figma-ui\src\app\App.tsx
git commit -m "Render gamepad monitor from hardware asset manifest"
```

---

### Task 8: Convert Mouse Regions To Precision Polygons

**Files:**
- Modify: `other/figma-ui/public/assets/hardware/live2d/mouse/manifest.json`
- Modify: `other/figma-ui/public/assets/hardware/live2d/mouse/gaming/manifest.json`
- Modify: `other/figma-ui/src/app/hardware-assets.test.mjs`

**Step 1: Add failing built-in mouse polygon test**

Add:

```js
test("checked-in mouse manifests use precision polygon regions", () => {
  for (const relative of [
    "../../public/assets/hardware/live2d/mouse/manifest.json",
    "../../public/assets/hardware/live2d/mouse/gaming/manifest.json",
  ]) {
    const raw = JSON.parse(
      readFileSync(new URL(relative, import.meta.url), "utf8"),
    );
    const asset = normalizeHardwareAssetManifest(raw, "/assets/hardware/live2d/mouse/");
    const primary = asset.regions.filter((region) =>
      ["mouse.left", "mouse.right", "mouse.middle"].includes(region.id),
    );

    assert.equal(primary.length, 3);
    assert.ok(primary.every((region) => region.shape.kind === "polygon"));
  }
});
```

**Step 2: Run helper tests to verify failure**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/hardware-assets.test.mjs
Pop-Location
```

Expected: FAIL because current mouse regions are rects.

**Step 3: Update office mouse regions**

Replace office mouse `shape` values:

```json
"mouse.left": {
  "kind": "polygon",
  "points": [
    { "x": 0.185, "y": 0.065 },
    { "x": 0.487, "y": 0.058 },
    { "x": 0.467, "y": 0.320 },
    { "x": 0.417, "y": 0.455 },
    { "x": 0.247, "y": 0.492 },
    { "x": 0.175, "y": 0.332 }
  ]
}
```

```json
"mouse.right": {
  "kind": "polygon",
  "points": [
    { "x": 0.513, "y": 0.058 },
    { "x": 0.815, "y": 0.065 },
    { "x": 0.825, "y": 0.332 },
    { "x": 0.753, "y": 0.492 },
    { "x": 0.583, "y": 0.455 },
    { "x": 0.533, "y": 0.320 }
  ]
}
```

```json
"mouse.middle": {
  "kind": "polygon",
  "points": [
    { "x": 0.435, "y": 0.050 },
    { "x": 0.565, "y": 0.050 },
    { "x": 0.575, "y": 0.300 },
    { "x": 0.535, "y": 0.365 },
    { "x": 0.465, "y": 0.365 },
    { "x": 0.425, "y": 0.300 }
  ]
}
```

Use similar polygons for side buttons:

```json
"mouse.back": {
  "kind": "polygon",
  "points": [
    { "x": 0.018, "y": 0.350 },
    { "x": 0.116, "y": 0.325 },
    { "x": 0.128, "y": 0.460 },
    { "x": 0.034, "y": 0.505 }
  ]
}
```

```json
"mouse.forward": {
  "kind": "polygon",
  "points": [
    { "x": 0.030, "y": 0.525 },
    { "x": 0.124, "y": 0.490 },
    { "x": 0.116, "y": 0.640 },
    { "x": 0.018, "y": 0.685 }
  ]
}
```

**Step 4: Update gaming mouse regions**

Use equivalent adjusted values for `mouse/gaming/manifest.json`:

```json
"mouse.left": {
  "kind": "polygon",
  "points": [
    { "x": 0.197, "y": 0.047 },
    { "x": 0.491, "y": 0.044 },
    { "x": 0.470, "y": 0.300 },
    { "x": 0.420, "y": 0.430 },
    { "x": 0.245, "y": 0.465 },
    { "x": 0.190, "y": 0.315 }
  ]
}
```

```json
"mouse.right": {
  "kind": "polygon",
  "points": [
    { "x": 0.509, "y": 0.044 },
    { "x": 0.803, "y": 0.047 },
    { "x": 0.810, "y": 0.315 },
    { "x": 0.755, "y": 0.465 },
    { "x": 0.580, "y": 0.430 },
    { "x": 0.530, "y": 0.300 }
  ]
}
```

Adjust middle/back/forward similarly from the office values using the existing gaming rect offsets.

**Step 5: Run tests**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/hardware-assets.test.mjs
Pop-Location
```

Expected: PASS.

**Step 6: Commit**

```powershell
git add other\figma-ui\public\assets\hardware\live2d\mouse\manifest.json other\figma-ui\public\assets\hardware\live2d\mouse\gaming\manifest.json other\figma-ui\src\app\hardware-assets.test.mjs
git commit -m "Refine mouse hardware regions with polygons"
```

---

### Task 9: Update Gallery Asset Metadata For Gamepad

**Files:**
- Modify: `other/figma-ui/src/app/desktop-model.mjs`
- Modify: `other/figma-ui/src/app/desktop-model.test.mjs`

**Step 1: Add failing gallery test**

Add:

```js
test("buildDeviceGalleryItems marks gamepad as hardware rig asset", () => {
  const items = buildDeviceGalleryItems({
    gamepads: [
      {
        gamepad_id: 0,
        name: "Xbox Style Controller",
        connected: true,
        pressed_buttons: ["South"],
        left_stick_x: 0,
        left_stick_y: 0,
        right_stick_x: 0,
        right_stick_y: 0,
        left_trigger: 0,
        right_trigger: 0,
        event_count: 1,
      },
    ],
    display: { display_count: 0 },
  });

  const gamepad = items.find((item) => item.kind === "gamepad");
  assert.equal(gamepad.rigKind, "gamepad");
  assert.equal(gamepad.rigVariant, "default");
});
```

**Step 2: Run test to verify failure**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/desktop-model.test.mjs
Pop-Location
```

Expected: FAIL because `HARDWARE_RIG_ASSETS` does not include gamepad.

**Step 3: Add gamepad metadata**

In `desktop-model.mjs`, update:

```js
export const HARDWARE_RIG_ASSETS = Object.freeze({
  keyboard: {
    manifest: "/assets/hardware/live2d/keyboard/manifest.json",
    base: "/assets/hardware/live2d/keyboard/base.png",
  },
  mouse: {
    manifest: "/assets/hardware/live2d/mouse/manifest.json",
    base: "/assets/hardware/live2d/mouse/base.png",
  },
  gamepad: {
    manifest: "/assets/hardware/live2d/gamepad/manifest.json",
    base: "/assets/hardware/live2d/gamepad/base.png",
  },
});
```

**Step 4: Run tests**

Run:

```powershell
Push-Location other\figma-ui
npm test -- src/app/desktop-model.test.mjs
Pop-Location
```

Expected: PASS.

**Step 5: Commit**

```powershell
git add other\figma-ui\src\app\desktop-model.mjs other\figma-ui\src\app\desktop-model.test.mjs
git commit -m "Expose gamepad hardware rig gallery metadata"
```

---

### Task 10: Final Verification

**Files:**
- Verify only unless fixes are required.

**Step 1: Run focused Rust tests**

Run:

```powershell
cargo test -p rshare-core hardware_asset_manifest_contract
```

Expected: PASS.

**Step 2: Run frontend tests**

Run:

```powershell
Push-Location other\figma-ui
npm test
Pop-Location
```

Expected: PASS.

**Step 3: Run frontend build**

Run:

```powershell
Push-Location other\figma-ui
npm run build
Pop-Location
```

Expected: PASS.

**Step 4: Run broader workspace tests if time allows**

Run:

```powershell
cargo test --workspace
```

Expected: PASS. If unrelated existing failures appear, capture them and do not hide them.

**Step 5: Visual smoke check**

Run a local frontend server:

```powershell
Push-Location other\figma-ui
npm run dev -- --host 127.0.0.1
```

Open the shown Vite URL in the in-app browser and verify:

- Settings shows keyboard, mouse, and gamepad asset selectors.
- Gamepad device page renders the image asset, not the old SVG controller.
- Gamepad pressed button activity highlights the correct manifest region.
- Mouse left/right/middle/side button highlights follow polygon contours.
- Keyboard highlights still align with the visible keys.

Stop the dev server after verification.

**Step 6: Commit any verification fixes**

If verification required fixes:

```powershell
git add <fixed-files>
git commit -m "Fix hardware precision surface verification issues"
```

**Step 7: Final status**

Run:

```powershell
git status --short
```

Expected: clean except for unrelated pre-existing user changes.
