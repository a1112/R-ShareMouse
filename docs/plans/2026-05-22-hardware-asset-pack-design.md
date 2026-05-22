# Hardware Asset Pack Design

Date: 2026-05-22

## Goal

R-ShareMouse needs a hardware asset pack format that lets each visual asset define its own key or button mapping. The format must support built-in frontend assets, user-imported downloadable packages, and export back to a distributable archive.

The first implementation targets keyboard and mouse visualizers in the desktop UI, while keeping the schema extensible for gamepads and other endpoint devices.

## Requirements

- Each asset can bind different key or button mappings to its own visual regions.
- Asset geometry can be defined by JSON shapes and, for precise artwork alignment, by grayscale mask images.
- Downloadable packages are zip files containing source images, optional mask images, and a JSON manifest.
- Installed assets are stored as unpacked folders.
- The frontend ships with built-in assets that use the same manifest model as imported assets.
- The core crate owns the schema and validation rules so future clients do not invent incompatible formats.

## Package Layout

An asset package is distributed as `.zip` or `.rshare-asset.zip`. After import it is unpacked into the application data directory:

```text
assets/
  hardware/
    <asset_id>/
      manifest.json
      base.png
      mask.png
      preview.png
      layers/
        pressed.png
        glow.png
```

Only `manifest.json` and at least one base layer are required. The package must not contain absolute paths, parent-directory traversal, or files outside the target folder after extraction.

## Manifest Schema

The manifest is versioned and uses normalized coordinates for portable rendering.

```json
{
  "schema_version": 1,
  "id": "builtin.keyboard.office",
  "name": "Office Keyboard",
  "kind": "keyboard",
  "base_size": { "width": 1694, "height": 544 },
  "layers": [
    { "id": "base", "role": "base", "src": "base.png" }
  ],
  "regions": [
    {
      "id": "key.a",
      "label": "A",
      "action": {
        "kind": "keyboard_key",
        "codes": ["Char(65)", "Raw(65)"]
      },
      "shape": {
        "kind": "rect",
        "x": 0.12,
        "y": 0.48,
        "w": 0.035,
        "h": 0.09
      }
    }
  ],
  "mask": {
    "src": "mask.png",
    "channels": [
      { "value": 32, "region_id": "key.a" }
    ]
  }
}
```

Core types:

- `HardwareAssetManifest`: full manifest.
- `HardwareAssetKind`: `Keyboard`, `Mouse`, `Gamepad`.
- `HardwareAssetLayer`: image layer metadata.
- `HardwareControlRegion`: logical key, button, axis, or control region.
- `HardwareControlAction`: event matching target, such as keyboard key codes or mouse buttons.
- `HardwareRegionShape`: `rect`, `polygon`, and later `ellipse`.
- `HardwareMaskMapping`: grayscale value to region id.

## Region Mapping Model

The renderer resolves active controls in this order:

1. Match incoming activity against `region.action`.
2. If a mask exists, use the mask channel for exact region coverage and highlight shape.
3. Otherwise, draw the JSON `shape`.

Masks are optional because simple assets are easier to author with rects or polygons. When both a mask and shape are present, the shape acts as a fallback and an approximate editor preview.

## Core Responsibilities

`rshare-core` should define the manifest schema and pure validation logic:

- Parse and serialize manifests.
- Validate schema version, ids, kinds, layer references, region ids, coordinates, and mask channel uniqueness.
- Reject unsafe relative paths.
- Provide import/export planning helpers that describe which files should be copied or packed.

The core crate should not decode images, unzip archives, or depend on UI frameworks.

## Desktop/Tauri Responsibilities

The desktop shell handles filesystem operations:

- List built-in and installed assets.
- Import zip packages by extracting to a staging directory, validating the manifest, then moving into `assets/hardware/<asset_id>/`.
- Export an installed asset folder back to a zip package.
- Delete user-installed assets. Built-in assets are read-only.

Errors should identify the failing package and validation problem, without partially installing invalid packages.

## Frontend Responsibilities

The React UI should render hardware rigs from manifests instead of hardcoded constants.

- Built-in assets live under `other/figma-ui/public/assets/hardware/...`.
- The settings page exposes per-kind asset selection.
- The device page uses selected keyboard and mouse assets for live visualizers.
- Input activity is matched against `action.codes` or button identifiers from the manifest.
- Current hardcoded keyboard rows and mouse hotspots become built-in manifest data.

The first version can keep runtime glow and press effects in React/CSS. Image-only effect layers may be added later.

## Built-In Assets

Initial built-ins:

- `builtin.keyboard.office`
- `builtin.keyboard.gaming`
- `builtin.mouse.office`
- `builtin.mouse.gaming`

These replace the current `office`/`gaming` rig variant constants while preserving existing artwork.

## Persistence

Selected asset ids should be stored in frontend local storage initially, matching the existing hardware rig variant preference. If later the selection needs to sync across clients, it can move into daemon-backed config.

Imported asset files should live in the desktop application data directory, not in the repo or generated frontend bundle.

## Compatibility And Migration

Existing manifests under `other/figma-ui/public/assets/hardware/live2d/...` should be migrated to schema version 1. During the transition, the frontend can still fall back to compiled definitions if loading a manifest fails, but the target state is manifest-driven rendering.

## Testing

Core tests:

- Accept a valid keyboard manifest with JSON regions.
- Accept a valid mouse manifest with mask channel mappings.
- Reject duplicate ids, missing layer files, invalid normalized coordinates, duplicate mask values, unknown mask region ids, and unsafe paths.

Frontend model tests:

- Build asset choices from built-in manifests.
- Match keyboard activity to manifest-defined regions.
- Match mouse button activity to manifest-defined regions.
- Preserve current gallery output while replacing hardcoded rig variant metadata.

Desktop/Tauri tests:

- Import validates before install.
- Import rejects path traversal archives.
- Export includes manifest and referenced files.

## Non-Goals

- No in-app visual asset editor in the first pass.
- No remote synchronization of custom assets in the first pass.
- No image decoding or mask raster analysis inside `rshare-core`.
- No replacement of the input routing model; this is a visualization and packaging feature.

## Open Follow-Up

If users need to author masks inside R-ShareMouse, add an editor later that can paint regions and write `mask.png` plus manifest channel mappings. The first version assumes masks are produced by external image tools.
