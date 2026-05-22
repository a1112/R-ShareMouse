# Hardware Asset Precision Surfaces Design

Date: 2026-05-22

## Goal

Replace the gamepad monitor with a real built-in Xbox-style controller asset and improve keyboard and mouse highlight regions so they fit the visible hardware surfaces closely while staying replaceable through the existing hardware asset manifest interface.

## Context

The repository already has the first hardware asset pack implementation:

- `rshare-core` defines `HardwareAssetManifest`, layers, regions, actions, and optional mask metadata.
- `apps/rshare-desktop/src-tauri` can import, list, and export installed hardware asset packages.
- `other/figma-ui` loads built-in keyboard and mouse manifests, stores selected assets in local storage, and renders active regions over base images.

The current gaps are:

- `gamepad` has `base.png` but no built-in manifest and is not in `BUILTIN_HARDWARE_ASSET_MANIFESTS`.
- `SimulatedGamepad` still draws a hardcoded SVG instead of using the asset manifest pipeline.
- Keyboard and mouse region rendering only supports rectangular overlays in `HardwareRigRegionOverlays`.
- Mouse regions are rough rectangles and do not follow the real button contours.
- The UI selection panel only exposes keyboard and mouse assets.

## Requirements

- Use a built-in Xbox-style controller asset with no official Xbox logo or protected branding.
- Keep the controller layout recognizably compatible with Xbox controls: A, B, X, Y, D-pad, LB, RB, LT, RT, Start, Select, Guide, and left/right stick controls.
- Store the generated controller as an asset under `other/figma-ui/public/assets/hardware/live2d/gamepad/`.
- Add `gamepad/manifest.json` with stable id `builtin.gamepad.xbox`.
- Extend frontend hardware asset support from `keyboard | mouse` to `keyboard | mouse | gamepad`.
- Render the gamepad from manifest layers and regions instead of hardcoded SVG.
- Support non-rectangular region shapes for closer visual alignment.
- Keep masks as part of the interface so future assets can use `mask.png` or split pressed-layer images without changing the renderer contract.

## Recommended Approach

Use a manifest-first asset pipeline for all hardware surfaces.

The renderer should load selected assets by kind, draw image layers, then draw active region highlights from manifest `regions`. `rect` remains supported for simple assets. `polygon` becomes the primary precision shape for mouse, gamepad, and any non-uniform keyboard keys. `mask` remains normalized and carried by the manifest model, but the first implementation does not need to rasterize mask pixels to ship the immediate visual improvement.

This fits the existing architecture better than a one-off gamepad component because imported assets already flow through the same manifest model. It also avoids committing to mask-only rendering before the UI has an editor or authoring workflow for masks.

## Manifest Shape Model

Current manifest regions already look like:

```json
{
  "id": "mouse.left",
  "label": "Left",
  "action": {
    "kind": "mouse_button",
    "buttons": ["Left", "left"]
  },
  "shape": {
    "kind": "rect",
    "x": 0.2,
    "y": 0.07,
    "w": 0.3,
    "h": 0.4,
    "radius": 38
  }
}
```

The precision update keeps this format and adds polygon rendering:

```json
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
      { "x": 0.76, "y": 0.54 },
      { "x": 0.79, "y": 0.50 },
      { "x": 0.83, "y": 0.54 },
      { "x": 0.79, "y": 0.58 }
    ]
  }
}
```

Optional mask metadata remains valid:

```json
{
  "mask": {
    "src": "mask.png",
    "channels": [
      { "value": 32, "region_id": "gamepad.button.a" }
    ]
  }
}
```

If a future asset includes both `mask` and `shape`, the shape is the editable preview and fallback. A later mask renderer can use the same `region_id` mapping without changing input matching.

## Asset Generation

Generate a single transparent PNG controller asset:

- Top-down or shallow perspective product-render style.
- No logo, no text, no watermark.
- Green background may be used as an intermediate generation step, but the checked-in asset should have transparent background.
- Neutral dark controller body so active blue highlights remain visible.
- Large enough for desktop rendering, using the existing `base.png` dimensions if the generated image can replace it cleanly.

The final asset should be stored as:

```text
other/figma-ui/public/assets/hardware/live2d/gamepad/base.png
other/figma-ui/public/assets/hardware/live2d/gamepad/manifest.json
```

If generation is unavailable because `OPENAI_API_KEY` is not set, create the manifest and renderer work first and leave the existing `gamepad/base.png` as the temporary image until the generated asset can be produced locally.

## Frontend Changes

`other/figma-ui/src/app/hardware-assets.mjs`:

- Add `/assets/hardware/live2d/gamepad/manifest.json` to `BUILTIN_HARDWARE_ASSET_MANIFESTS`.
- Normalize `polygon` shapes without losing points.
- Keep mask metadata normalized.
- Ensure `resolveActiveHardwareRegions` supports `gamepad_button` using existing gamepad button aliases.

`other/figma-ui/src/app/App.tsx`:

- Change `HardwareRigKind` from `keyboard | mouse` to `keyboard | mouse | gamepad`.
- Add per-kind storage key `rshare.hardwareAsset.gamepad`.
- Add gamepad to `selectedIds`, `setSelectedId`, settings select controls, and installed asset rendering.
- Update `hardwareRigFromManifest` to accept gamepad assets.
- Replace `SimulatedGamepad`'s hardcoded SVG drawing with `HardwareRigView kind="gamepad"`.
- Preserve stick and trigger telemetry in surrounding stats; the first pass highlights stick press and trigger button/axis regions, not analog thumb offset animation.
- Extend `HardwareRigRegionOverlays` to render `rect` and `polygon`.
- Keep fallback rendering for built-in keyboard and mouse only when a manifest cannot load.

`other/figma-ui/src/app/desktop-model.mjs`:

- Add gamepad built-in asset metadata to gallery item rig detection so the overview can show the selected asset consistently.

## Region Authoring

Mouse manifests should move from coarse rectangles to polygon approximations:

- `mouse.left`: follows the left button shell, narrowing near the wheel slot.
- `mouse.right`: mirrors the right shell.
- `mouse.middle`: tracks the wheel and center button channel.
- `mouse.back` and `mouse.forward`: match the side button slants.

Keyboard manifests can keep most keycaps as rects because the visible keycaps are mostly rectangular. Wider keys and non-standard clusters may be upgraded to polygons only where the current overlay visibly bleeds outside the key surface.

Gamepad regions should include at least:

- Face buttons: A, B, X, Y.
- D-pad directions: Up, Down, Left, Right.
- Bumpers and triggers: LB, RB, LT, RT.
- Menu cluster: Select, Start, Guide.
- Stick buttons: LeftStick, RightStick.

## Testing

Core:

- Existing `hardware_asset_manifest_contract` tests continue to validate `polygon` and mask references.
- Add a gamepad manifest parsing test if current coverage does not exercise `HardwareAssetKind::Gamepad`.

Frontend model:

- Built-in manifest list includes the gamepad manifest.
- Checked-in gamepad manifest normalizes to `kind === "gamepad"` and has mapped button regions.
- `resolveActiveHardwareRegions` matches gamepad pressed buttons.
- Polygon shapes survive normalization and can be returned for active regions.
- Mouse built-in manifest includes polygon regions for primary controls.

Frontend build:

- `npm test` passes in `other/figma-ui`.
- `npm run build` succeeds.

Visual smoke:

- Devices page gamepad view shows a controller image rather than SVG primitives.
- Pressing gamepad buttons highlights mapped regions.
- Mouse left/right/middle/side highlight contours align with the image.
- Keyboard highlights still work after shape normalization changes.

## Non-Goals

- No in-app mask painter or asset editor in this step.
- No remote synchronization of selected asset ids.
- No official Xbox logo or trademarked marks in generated assets.
- No complete analog stick animation from mask geometry in the first pass.

## Follow-Up

After polygon support lands, add optional mask raster rendering:

1. Load `mask.src` as an image.
2. For each active region, resolve the channel value for that `region_id`.
3. Render an alpha highlight clipped by that channel.
4. Prefer mask highlight over polygon when the mask loads successfully.

