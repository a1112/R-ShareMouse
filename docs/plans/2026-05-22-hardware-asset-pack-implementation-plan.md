# Hardware Asset Pack Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a manifest-driven hardware asset pack system with core schema validation, desktop import/export, built-in frontend assets, and manifest-based keyboard/mouse rendering.

**Architecture:** `rshare-core` owns the portable manifest schema and validation. `apps/rshare-desktop/src-tauri` owns zip extraction, app-data storage, and export. `other/figma-ui` owns built-in manifests, user selection, and rendering active regions from asset-defined key/button mappings.

**Tech Stack:** Rust 2021, serde/serde_json, anyhow/thiserror, Tauri 2 commands, zip archive handling in the desktop crate, React 18, Vite, Node `node:test`.

---

## Task 1: Add Core Manifest Types And Basic Validation

**Files:**
- Create: `crates/rshare-core/src/hardware_assets.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Create: `crates/rshare-core/tests/hardware_asset_manifest_contract.rs`

**Step 1: Write the failing test**

Create `crates/rshare-core/tests/hardware_asset_manifest_contract.rs`:

```rust
use rshare_core::{
    HardwareAssetKind, HardwareAssetManifest, HardwareControlAction, HardwareRegionShape,
};

fn valid_keyboard_manifest() -> HardwareAssetManifest {
    serde_json::from_value(serde_json::json!({
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
                "action": { "kind": "keyboard_key", "codes": ["Char(65)", "Raw(65)"] },
                "shape": { "kind": "rect", "x": 0.12, "y": 0.48, "w": 0.035, "h": 0.09 }
            }
        ]
    }))
    .unwrap()
}

#[test]
fn parses_valid_keyboard_manifest() {
    let manifest = valid_keyboard_manifest();

    assert_eq!(manifest.kind, HardwareAssetKind::Keyboard);
    assert_eq!(manifest.layers[0].src.as_deref(), Some("base.png"));
    assert_eq!(manifest.regions[0].id, "key.a");
    assert!(matches!(
        manifest.regions[0].action,
        HardwareControlAction::KeyboardKey { .. }
    ));
    assert!(matches!(
        manifest.regions[0].shape,
        HardwareRegionShape::Rect { .. }
    ));
    manifest.validate().unwrap();
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p rshare-core hardware_asset_manifest_contract
```

Expected: compile failure because `HardwareAssetManifest` and related types do not exist.

**Step 3: Write minimal implementation**

Create `crates/rshare-core/src/hardware_assets.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

pub const HARDWARE_ASSET_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareAssetKind {
    Keyboard,
    Mouse,
    Gamepad,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareAssetSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareAssetManifest {
    pub schema_version: u16,
    pub id: String,
    pub name: String,
    pub kind: HardwareAssetKind,
    pub base_size: HardwareAssetSize,
    #[serde(default)]
    pub layers: Vec<HardwareAssetLayer>,
    #[serde(default)]
    pub regions: Vec<HardwareControlRegion>,
    #[serde(default)]
    pub mask: Option<HardwareMaskMapping>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareAssetLayer {
    pub id: String,
    pub role: String,
    #[serde(default)]
    pub src: Option<String>,
    #[serde(default)]
    pub opacity: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareControlRegion {
    pub id: String,
    pub label: String,
    pub action: HardwareControlAction,
    pub shape: HardwareRegionShape,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HardwareControlAction {
    KeyboardKey { codes: Vec<String> },
    MouseButton { buttons: Vec<String> },
    GamepadButton { buttons: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HardwareRegionShape {
    Rect { x: f32, y: f32, w: f32, h: f32 },
    Polygon { points: Vec<HardwarePoint> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwarePoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareMaskMapping {
    pub src: String,
    #[serde(default)]
    pub channels: Vec<HardwareMaskChannel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareMaskChannel {
    pub value: u8,
    pub region_id: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HardwareAssetValidationError {
    #[error("unsupported hardware asset schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("hardware asset id is empty")]
    EmptyId,
    #[error("hardware asset must include at least one layer")]
    MissingLayer,
    #[error("duplicate id: {0}")]
    DuplicateId(String),
    #[error("invalid normalized geometry for region {0}")]
    InvalidGeometry(String),
}

impl HardwareAssetManifest {
    pub fn validate(&self) -> Result<(), HardwareAssetValidationError> {
        if self.schema_version != HARDWARE_ASSET_SCHEMA_VERSION {
            return Err(HardwareAssetValidationError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if self.id.trim().is_empty() {
            return Err(HardwareAssetValidationError::EmptyId);
        }
        if self.layers.is_empty() {
            return Err(HardwareAssetValidationError::MissingLayer);
        }
        let mut ids = HashSet::new();
        for layer in &self.layers {
            if !ids.insert(layer.id.clone()) {
                return Err(HardwareAssetValidationError::DuplicateId(layer.id.clone()));
            }
        }
        for region in &self.regions {
            if !ids.insert(region.id.clone()) {
                return Err(HardwareAssetValidationError::DuplicateId(region.id.clone()));
            }
            if !region.shape.is_valid_normalized() {
                return Err(HardwareAssetValidationError::InvalidGeometry(region.id.clone()));
            }
        }
        Ok(())
    }
}

impl HardwareRegionShape {
    fn is_valid_normalized(&self) -> bool {
        match self {
            Self::Rect { x, y, w, h } => {
                finite_unit(*x)
                    && finite_unit(*y)
                    && finite_positive(*w)
                    && finite_positive(*h)
                    && *x + *w <= 1.0
                    && *y + *h <= 1.0
            }
            Self::Polygon { points } => {
                points.len() >= 3 && points.iter().all(|point| finite_unit(point.x) && finite_unit(point.y))
            }
        }
    }
}

fn finite_unit(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn finite_positive(value: f32) -> bool {
    value.is_finite() && value > 0.0 && value <= 1.0
}
```

Modify `crates/rshare-core/src/lib.rs`:

```rust
pub mod hardware_assets;

pub use hardware_assets::{
    HardwareAssetKind, HardwareAssetLayer, HardwareAssetManifest, HardwareAssetSize,
    HardwareAssetValidationError, HardwareControlAction, HardwareControlRegion,
    HardwareMaskChannel, HardwareMaskMapping, HardwarePoint, HardwareRegionShape,
    HARDWARE_ASSET_SCHEMA_VERSION,
};
```

**Step 4: Run test to verify it passes**

Run:

```bash
cargo test -p rshare-core hardware_asset_manifest_contract
```

Expected: PASS.

**Step 5: Commit**

```bash
git add crates/rshare-core/src/hardware_assets.rs crates/rshare-core/src/lib.rs crates/rshare-core/tests/hardware_asset_manifest_contract.rs
git commit -m "Add hardware asset manifest schema"
```

---

## Task 2: Add Core Validation For Paths, Masks, And Region References

**Files:**
- Modify: `crates/rshare-core/src/hardware_assets.rs`
- Modify: `crates/rshare-core/tests/hardware_asset_manifest_contract.rs`

**Step 1: Write failing validation tests**

Append tests:

```rust
use rshare_core::HardwareAssetValidationError;

#[test]
fn rejects_unsafe_layer_paths() {
    let mut manifest = valid_keyboard_manifest();
    manifest.layers[0].src = Some("../escape.png".to_string());

    assert_eq!(
        manifest.validate().unwrap_err(),
        HardwareAssetValidationError::UnsafePath("../escape.png".to_string())
    );
}

#[test]
fn rejects_mask_channels_that_reference_unknown_regions() {
    let mut manifest = valid_keyboard_manifest();
    manifest.mask = Some(serde_json::from_value(serde_json::json!({
        "src": "mask.png",
        "channels": [{ "value": 32, "region_id": "missing" }]
    })).unwrap());

    assert_eq!(
        manifest.validate().unwrap_err(),
        HardwareAssetValidationError::UnknownMaskRegion("missing".to_string())
    );
}

#[test]
fn rejects_duplicate_mask_values() {
    let mut manifest = valid_keyboard_manifest();
    manifest.mask = Some(serde_json::from_value(serde_json::json!({
        "src": "mask.png",
        "channels": [
            { "value": 32, "region_id": "key.a" },
            { "value": 32, "region_id": "key.a" }
        ]
    })).unwrap());

    assert_eq!(
        manifest.validate().unwrap_err(),
        HardwareAssetValidationError::DuplicateMaskValue(32)
    );
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p rshare-core hardware_asset_manifest_contract
```

Expected: compile failure for missing validation error variants or failing assertions.

**Step 3: Implement minimal validation**

Extend `HardwareAssetValidationError`:

```rust
#[error("unsafe hardware asset path: {0}")]
UnsafePath(String),
#[error("mask references unknown region: {0}")]
UnknownMaskRegion(String),
#[error("duplicate mask channel value: {0}")]
DuplicateMaskValue(u8),
```

Add helpers:

```rust
pub fn validate_asset_relative_path(path: &str) -> bool {
    let trimmed = path.trim();
    !trimmed.is_empty()
        && !trimmed.starts_with('/')
        && !trimmed.starts_with('\\')
        && !trimmed.contains(':')
        && !trimmed
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
}
```

Call it from `validate()` for each `layer.src` and `mask.src`. Build a `HashSet<String>` of region ids, then validate each mask channel has a known region and a unique grayscale value.

**Step 4: Run tests**

Run:

```bash
cargo test -p rshare-core hardware_asset_manifest_contract
```

Expected: PASS.

**Step 5: Commit**

```bash
git add crates/rshare-core/src/hardware_assets.rs crates/rshare-core/tests/hardware_asset_manifest_contract.rs
git commit -m "Validate hardware asset paths and mask mappings"
```

---

## Task 3: Add Desktop Asset Store Commands

**Files:**
- Modify: `apps/rshare-desktop/src-tauri/Cargo.toml`
- Create: `apps/rshare-desktop/src-tauri/src/hardware_assets.rs`
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`

**Step 1: Add failing desktop tests**

Create a test module in `apps/rshare-desktop/src-tauri/src/hardware_assets.rs` while writing the module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sample_package_bytes() -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", options).unwrap();
        writer.write_all(br#"{
            "schema_version": 1,
            "id": "user.keyboard.sample",
            "name": "Sample",
            "kind": "keyboard",
            "base_size": { "width": 100, "height": 50 },
            "layers": [{ "id": "base", "role": "base", "src": "base.png" }],
            "regions": []
        }"#).unwrap();
        writer.start_file("base.png", options).unwrap();
        writer.write_all(b"png-bytes").unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn import_zip_installs_unpacked_asset_folder() {
        let temp = tempfile::tempdir().unwrap();
        let installed = import_hardware_asset_package(temp.path(), &sample_package_bytes()).unwrap();

        assert_eq!(installed.id, "user.keyboard.sample");
        assert!(temp.path().join("hardware").join("user.keyboard.sample").join("manifest.json").exists());
        assert!(temp.path().join("hardware").join("user.keyboard.sample").join("base.png").exists());
    }

    #[test]
    fn import_zip_rejects_path_traversal() {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("../escape.txt", options).unwrap();
        writer.write_all(b"bad").unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        assert!(import_hardware_asset_package(tempfile::tempdir().unwrap().path(), &bytes).is_err());
    }
}
```

**Step 2: Add dev/runtime dependencies and run failing tests**

Modify `apps/rshare-desktop/src-tauri/Cargo.toml`:

```toml
zip = "2"

[dev-dependencies]
tempfile = "3"
```

Run:

```bash
cargo test -p rshare-desktop hardware_assets
```

Expected: compile failure because module and functions do not exist.

**Step 3: Implement asset store functions**

Create `apps/rshare-desktop/src-tauri/src/hardware_assets.rs`:

```rust
use anyhow::{anyhow, Context, Result};
use rshare_core::{HardwareAssetKind, HardwareAssetManifest};
use serde::Serialize;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledHardwareAsset {
    pub id: String,
    pub name: String,
    pub kind: HardwareAssetKind,
    pub manifest_path: String,
    pub folder_path: String,
}

pub fn hardware_asset_root(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("hardware")
}

pub fn import_hardware_asset_package(
    app_data_dir: &Path,
    package_bytes: &[u8],
) -> Result<InstalledHardwareAsset> {
    let root = hardware_asset_root(app_data_dir);
    fs::create_dir_all(&root)?;
    let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&staging)?;

    let result = extract_and_validate(&staging, package_bytes)
        .and_then(|manifest| {
            let target = root.join(&manifest.id);
            if target.exists() {
                fs::remove_dir_all(&target)?;
            }
            fs::rename(&staging, &target)?;
            Ok(installed_asset_from_manifest(target, manifest))
        });

    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn extract_and_validate(target: &Path, package_bytes: &[u8]) -> Result<HardwareAssetManifest> {
    let mut archive = zip::ZipArchive::new(Cursor::new(package_bytes))?;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let enclosed = file
            .enclosed_name()
            .ok_or_else(|| anyhow!("archive contains unsafe path"))?
            .to_path_buf();
        let output = target.join(enclosed);
        if file.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        fs::write(output, bytes)?;
    }

    let manifest_path = target.join("manifest.json");
    let manifest: HardwareAssetManifest = serde_json::from_slice(
        &fs::read(&manifest_path).context("hardware asset package missing manifest.json")?,
    )?;
    manifest.validate()?;
    for relative in manifest.referenced_paths() {
        if !target.join(relative).is_file() {
            return Err(anyhow!("hardware asset references missing file: {relative}"));
        }
    }
    Ok(manifest)
}
```

Add the small missing helpers during implementation:

- `installed_asset_from_manifest(folder: PathBuf, manifest: HardwareAssetManifest)`.
- `export_hardware_asset_package(app_data_dir: &Path, asset_id: &str) -> Result<Vec<u8>>`.
- `list_installed_hardware_assets(app_data_dir: &Path) -> Result<Vec<InstalledHardwareAsset>>`.

Add `HardwareAssetManifest::referenced_paths()` in `rshare-core/src/hardware_assets.rs`, test it in Task 2 or this task.

**Step 4: Expose Tauri commands**

Modify `apps/rshare-desktop/src-tauri/src/main.rs`:

```rust
mod hardware_assets;

#[tauri::command]
async fn list_hardware_assets(app: AppHandle) -> Result<Vec<hardware_assets::InstalledHardwareAsset>, String> {
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    hardware_assets::list_installed_hardware_assets(&dir).map_err(|err| err.to_string())
}

#[tauri::command]
async fn import_hardware_asset(app: AppHandle, bytes: Vec<u8>) -> Result<hardware_assets::InstalledHardwareAsset, String> {
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    hardware_assets::import_hardware_asset_package(&dir, &bytes).map_err(|err| err.to_string())
}

#[tauri::command]
async fn export_hardware_asset(app: AppHandle, asset_id: String) -> Result<Vec<u8>, String> {
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    hardware_assets::export_hardware_asset_package(&dir, &asset_id).map_err(|err| err.to_string())
}
```

Register the commands in the existing `invoke_handler`.

**Step 5: Run tests**

Run:

```bash
cargo test -p rshare-core hardware_asset_manifest_contract
cargo test -p rshare-desktop hardware_assets
```

Expected: PASS.

**Step 6: Commit**

```bash
git add apps/rshare-desktop/src-tauri/Cargo.toml apps/rshare-desktop/src-tauri/src/hardware_assets.rs apps/rshare-desktop/src-tauri/src/main.rs crates/rshare-core/src/hardware_assets.rs crates/rshare-core/tests/hardware_asset_manifest_contract.rs
git commit -m "Add desktop hardware asset import export"
```

---

## Task 4: Add Frontend Asset Model Helpers

**Files:**
- Create: `other/figma-ui/src/app/hardware-assets.mjs`
- Create: `other/figma-ui/src/app/hardware-assets.test.mjs`
- Modify: `other/figma-ui/src/app/desktop-model.mjs`
- Modify: `other/figma-ui/src/app/desktop-model.test.mjs`

**Step 1: Write failing helper tests**

Create `other/figma-ui/src/app/hardware-assets.test.mjs`:

```js
import test from "node:test";
import assert from "node:assert/strict";

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
  const asset = normalizeHardwareAssetManifest(keyboardManifest, "/assets/hardware/keyboard/");

  assert.equal(asset.id, "builtin.keyboard.office");
  assert.equal(asset.baseSize.width, 1000);
  assert.equal(asset.layers[0].src, "/assets/hardware/keyboard/base.png");
});

test("resolveActiveHardwareRegions matches pressed keyboard codes", () => {
  const asset = normalizeHardwareAssetManifest(keyboardManifest, "/assets/hardware/keyboard/");
  const regions = resolveActiveHardwareRegions(asset, {
    pressedKeys: ["Char(65)"],
    lastKey: null,
    recentButtons: [],
  });

  assert.deepEqual(regions.map((region) => region.id), ["key.a"]);
});

test("buildHardwareAssetChoices groups assets by kind", () => {
  const asset = normalizeHardwareAssetManifest(keyboardManifest, "/assets/hardware/keyboard/");
  const choices = buildHardwareAssetChoices([asset]);

  assert.deepEqual(choices.keyboard.map((choice) => [choice.id, choice.name]), [
    ["builtin.keyboard.office", "Office Keyboard"],
  ]);
});
```

**Step 2: Run test to verify it fails**

Run:

```bash
cd other/figma-ui
npm test -- src/app/hardware-assets.test.mjs
```

Expected: module not found.

**Step 3: Implement helpers**

Create `other/figma-ui/src/app/hardware-assets.mjs`:

```js
export const BUILTIN_HARDWARE_ASSET_MANIFESTS = Object.freeze([
  "/assets/hardware/live2d/keyboard/manifest.json",
  "/assets/hardware/live2d/keyboard/gaming/manifest.json",
  "/assets/hardware/live2d/mouse/manifest.json",
  "/assets/hardware/live2d/mouse/gaming/manifest.json",
]);

export function normalizeHardwareAssetManifest(raw, baseUrl = "") {
  const baseSize = raw.base_size ?? raw.baseSize ?? { width: 1, height: 1 };
  return {
    id: String(raw.id),
    name: String(raw.name ?? raw.id),
    kind: String(raw.kind),
    schemaVersion: Number(raw.schema_version ?? raw.schemaVersion ?? 1),
    baseSize: {
      width: Number(baseSize.width ?? 1),
      height: Number(baseSize.height ?? 1),
    },
    layers: (raw.layers ?? []).map((layer) => ({
      id: String(layer.id),
      role: String(layer.role),
      render: layer.render ?? (layer.src ? "image" : "runtime"),
      src: layer.src ? resolveAssetUrl(baseUrl, layer.src) : null,
      opacity: layer.opacity == null ? 1 : Number(layer.opacity),
    })),
    regions: (raw.regions ?? raw.hotspots ?? []).map(normalizeRegion),
    mask: raw.mask ?? null,
    readonly: Boolean(raw.readonly ?? raw.builtin),
  };
}

export function buildHardwareAssetChoices(assets = []) {
  return {
    keyboard: assets.filter((asset) => asset.kind === "keyboard").map(assetChoice),
    mouse: assets.filter((asset) => asset.kind === "mouse").map(assetChoice),
    gamepad: assets.filter((asset) => asset.kind === "gamepad").map(assetChoice),
  };
}

export function resolveActiveHardwareRegions(asset, activity = {}) {
  return (asset?.regions ?? []).filter((region) => regionMatchesActivity(region, activity));
}

function resolveAssetUrl(baseUrl, src) {
  if (/^(https?:|data:|blob:|\/)/i.test(src)) {
    return src;
  }
  return `${baseUrl.replace(/\/?$/, "/")}${src}`;
}

function normalizeRegion(region) {
  return {
    id: String(region.id),
    label: String(region.label ?? region.id),
    action: region.action ?? inferLegacyAction(region),
    shape: region.shape ?? legacyRectShape(region),
  };
}

function assetChoice(asset) {
  return { id: asset.id, name: asset.name, kind: asset.kind, readonly: Boolean(asset.readonly) };
}
```

Add `regionMatchesActivity`, `keyboardActionMatches`, `mouseActionMatches`, `normalizeKeyToken`, `inferLegacyAction`, and `legacyRectShape` with the current App.tsx matching behavior.

**Step 4: Run tests**

Run:

```bash
cd other/figma-ui
npm test -- src/app/hardware-assets.test.mjs
npm test -- src/app/desktop-model.test.mjs
```

Expected: PASS.

**Step 5: Commit**

```bash
git add other/figma-ui/src/app/hardware-assets.mjs other/figma-ui/src/app/hardware-assets.test.mjs other/figma-ui/src/app/desktop-model.mjs other/figma-ui/src/app/desktop-model.test.mjs
git commit -m "Add frontend hardware asset model helpers"
```

---

## Task 5: Migrate Built-In Asset Manifests To Schema Version 1

**Files:**
- Modify: `other/figma-ui/public/assets/hardware/live2d/keyboard/manifest.json`
- Modify: `other/figma-ui/public/assets/hardware/live2d/keyboard/gaming/manifest.json`
- Modify: `other/figma-ui/public/assets/hardware/live2d/mouse/manifest.json`
- Modify: `other/figma-ui/public/assets/hardware/live2d/mouse/gaming/manifest.json`
- Modify: `other/figma-ui/src/app/hardware-assets.test.mjs`

**Step 1: Add failing tests for built-in ids**

Append:

```js
test("built-in asset manifests expose stable ids and mapped regions", () => {
  const keyboard = normalizeHardwareAssetManifest({
    schema_version: 1,
    id: "builtin.keyboard.office",
    name: "Office Keyboard",
    kind: "keyboard",
    base_size: { width: 1694, height: 544 },
    layers: [{ id: "base", role: "base", src: "base.png" }],
    regions: [
      { id: "key.escape", label: "Esc", action: { kind: "keyboard_key", codes: ["Escape"] }, shape: { kind: "rect", x: 0, y: 0, w: 0.05, h: 0.1 } },
    ],
  });

  assert.equal(keyboard.id, "builtin.keyboard.office");
  assert.equal(keyboard.regions[0].action.kind, "keyboard_key");
});
```

**Step 2: Run test**

Run:

```bash
cd other/figma-ui
npm test -- src/app/hardware-assets.test.mjs
```

Expected: PASS for helper behavior; built-in JSON is not wired yet.

**Step 3: Update JSON manifests**

For each built-in manifest:

- Add `schema_version`, `id`, `name`.
- Keep `kind`, `base_size`, and `layers`.
- For keyboard assets, encode regions generated from current `KEYBOARD_ROWS` with per-key `keyboard_key` actions.
- For mouse assets, encode five button regions with `mouse_button` actions.
- Keep optional legacy `hotspots` only if needed during transition; do not rely on it after Task 6.

Example mouse region:

```json
{
  "id": "mouse.left",
  "label": "Left",
  "action": { "kind": "mouse_button", "buttons": ["Left", "left"] },
  "shape": { "kind": "rect", "x": 0.20, "y": 0.07, "w": 0.30, "h": 0.40, "radius": 38 }
}
```

**Step 4: Add an asset fixture loader test**

Node tests can read repo files:

```js
import { readFileSync } from "node:fs";

test("checked-in office mouse manifest has mapped button regions", () => {
  const raw = JSON.parse(readFileSync(new URL("../../public/assets/hardware/live2d/mouse/manifest.json", import.meta.url), "utf8"));
  const asset = normalizeHardwareAssetManifest(raw, "/assets/hardware/live2d/mouse/");

  assert.equal(asset.id, "builtin.mouse.office");
  assert.ok(asset.regions.some((region) => region.id === "mouse.left"));
});
```

**Step 5: Run tests**

Run:

```bash
cd other/figma-ui
npm test -- src/app/hardware-assets.test.mjs
```

Expected: PASS.

**Step 6: Commit**

```bash
git add other/figma-ui/public/assets/hardware/live2d other/figma-ui/src/app/hardware-assets.test.mjs
git commit -m "Migrate built in hardware asset manifests"
```

---

## Task 6: Render Keyboard And Mouse From Asset Manifests

**Files:**
- Modify: `other/figma-ui/src/app/App.tsx`
- Modify: `other/figma-ui/src/app/hardware-assets.mjs`
- Modify: `other/figma-ui/src/app/hardware-assets.test.mjs`

**Step 1: Add failing model test for active region shapes**

Append to `hardware-assets.test.mjs`:

```js
test("resolveActiveHardwareRegions returns drawable shapes for active mouse buttons", () => {
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
        label: "L",
        action: { kind: "mouse_button", buttons: ["Left"] },
        shape: { kind: "rect", x: 0.2, y: 0.07, w: 0.3, h: 0.4, radius: 38 },
      },
    ],
  });

  const active = resolveActiveHardwareRegions(mouse, { pressedButtons: ["Left"] });

  assert.equal(active[0].shape.kind, "rect");
  assert.equal(active[0].shape.radius, 38);
});
```

**Step 2: Run test**

Run:

```bash
cd other/figma-ui
npm test -- src/app/hardware-assets.test.mjs
```

Expected: FAIL until mouse matching includes `pressedButtons`.

**Step 3: Update helper matching**

Make `regionMatchesActivity` support:

- `keyboard_key`: `pressedKeys`, `lastKey`, and recent keyboard events.
- `mouse_button`: `pressedButtons`, `recentButtons`, and wheel-as-middle if configured.
- Preserve existing key normalization for `Raw(...)`, `Char(...)`, and named keys.

**Step 4: Refactor `App.tsx` rendering**

Replace these hardcoded definitions with manifest-driven data:

- `HARDWARE_RIGS`
- `MOUSE_RIG_HOTSPOTS`
- `KeyboardRigHotspots`
- `MouseRigHotspots`

Keep `HardwareHotspotOverlay`, but feed it from active manifest regions:

```tsx
function HardwareRegionOverlays({
  asset,
  activity,
  accent,
  theme,
  compact,
}: {
  asset: HardwareAssetDefinition;
  activity: HardwareRigActivity;
  accent: string;
  theme: typeof FIGMA_DESKTOP_THEME;
  compact: boolean;
}) {
  const activeRegions = resolveActiveHardwareRegions(asset, activity);
  return (
    <>
      {activeRegions.map((region) => (
        <HardwareHotspotOverlay
          key={region.id}
          x={region.shape.x}
          y={region.shape.y}
          w={region.shape.w}
          h={region.shape.h}
          radius={region.shape.radius ?? 7}
          active
          label={region.label}
          accent={accent}
          theme={theme}
          compact={compact}
        />
      ))}
    </>
  );
}
```

`HardwareRigView` should accept an `asset` object:

```tsx
function HardwareRigView({ asset, activity, accent, theme, compact = false }: Props) {
  const imageLayers = asset.layers.filter((layer) => layer.render === "image" && layer.src);
  return (
    <div style={{ aspectRatio: `${asset.baseSize.width} / ${asset.baseSize.height}` }}>
      {imageLayers.map((layer) => <img key={layer.id} src={layer.src} alt="" />)}
      <HardwareRegionOverlays asset={asset} activity={activity} accent={accent} theme={theme} compact={compact} />
    </div>
  );
}
```

**Step 5: Run frontend tests**

Run:

```bash
cd other/figma-ui
npm test
npm run build
```

Expected: PASS and successful Vite build.

**Step 6: Commit**

```bash
git add other/figma-ui/src/app/App.tsx other/figma-ui/src/app/hardware-assets.mjs other/figma-ui/src/app/hardware-assets.test.mjs
git commit -m "Render hardware rigs from asset manifests"
```

---

## Task 7: Add Frontend Selection, Import, And Export UI

**Files:**
- Modify: `other/figma-ui/src/app/App.tsx`
- Modify: `other/figma-ui/src/app/hardware-assets.mjs`
- Modify: `other/figma-ui/src/app/hardware-assets.test.mjs`

**Step 1: Add failing state helper tests**

Add helper tests for selected ids:

```js
import { resolveSelectedHardwareAsset } from "./hardware-assets.mjs";

test("resolveSelectedHardwareAsset falls back to first matching kind", () => {
  const assets = [
    { id: "builtin.keyboard.office", kind: "keyboard", name: "Office" },
    { id: "builtin.keyboard.gaming", kind: "keyboard", name: "Gaming" },
  ];

  assert.equal(resolveSelectedHardwareAsset(assets, "keyboard", "missing").id, "builtin.keyboard.office");
});
```

**Step 2: Run test**

Run:

```bash
cd other/figma-ui
npm test -- src/app/hardware-assets.test.mjs
```

Expected: compile failure for missing helper.

**Step 3: Implement selection helper**

Add:

```js
export function resolveSelectedHardwareAsset(assets = [], kind, selectedId) {
  return (
    assets.find((asset) => asset.kind === kind && asset.id === selectedId) ??
    assets.find((asset) => asset.kind === kind) ??
    null
  );
}
```

**Step 4: Add UI state and controls**

In `App.tsx`:

- Replace `HARDWARE_RIG_VARIANT_STORAGE_KEY` with per-kind ids:
  - `rshare.hardwareAsset.keyboard`
  - `rshare.hardwareAsset.mouse`
- Load built-in manifests via `fetch(BUILTIN_HARDWARE_ASSET_MANIFESTS)`.
- Load imported assets via Tauri `list_hardware_assets`.
- Add settings controls for keyboard and mouse asset selection.
- Add file input for import:

```tsx
async function importHardwareAssetFile(file: File) {
  const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
  await invokeCommand("import_hardware_asset", { bytes });
  await refreshHardwareAssets();
}
```

- Add export button for selected non-built-in assets:

```tsx
const bytes = await invokeCommand<number[]>("export_hardware_asset", { assetId });
const blob = new Blob([new Uint8Array(bytes)], { type: "application/zip" });
```

Use a temporary object URL and `<a download>` for the browser download.

**Step 5: Run tests and build**

Run:

```bash
cd other/figma-ui
npm test
npm run build
```

Expected: PASS and successful build.

**Step 6: Commit**

```bash
git add other/figma-ui/src/app/App.tsx other/figma-ui/src/app/hardware-assets.mjs other/figma-ui/src/app/hardware-assets.test.mjs
git commit -m "Add hardware asset selection import export UI"
```

---

## Task 8: Final Workspace Verification

**Files:**
- Verify only.

**Step 1: Run Rust tests**

Run:

```bash
cargo test -p rshare-core hardware_asset_manifest_contract
cargo test -p rshare-desktop hardware_assets
cargo test --workspace
```

Expected: PASS.

**Step 2: Run frontend tests and build**

Run:

```bash
cd other/figma-ui
npm test
npm run build
```

Expected: PASS and successful build.

**Step 3: Run desktop app smoke check**

Run:

```bash
cargo run -p rshare-gui
```

Expected: desktop UI starts. Open the Devices and Settings pages, confirm:

- Built-in keyboard and mouse assets are listed.
- Changing selected assets updates visualizers.
- Pressed keys/buttons still highlight.
- Import rejects an invalid zip.
- Export downloads a zip for an imported asset.

**Step 4: Commit verification fixes if needed**

If verification required fixes:

```bash
git add <fixed-files>
git commit -m "Fix hardware asset pack verification issues"
```

**Step 5: Final status**

Run:

```bash
git status --short
```

Expected: only unrelated pre-existing user changes remain.
