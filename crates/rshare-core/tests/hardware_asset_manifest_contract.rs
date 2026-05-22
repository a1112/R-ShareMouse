use rshare_core::{
    HardwareAssetKind, HardwareAssetManifest, HardwareAssetValidationError, HardwareControlAction,
    HardwareRegionShape,
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
    manifest.mask = Some(
        serde_json::from_value(serde_json::json!({
            "src": "mask.png",
            "channels": [{ "value": 32, "region_id": "missing" }]
        }))
        .unwrap(),
    );

    assert_eq!(
        manifest.validate().unwrap_err(),
        HardwareAssetValidationError::UnknownMaskRegion("missing".to_string())
    );
}

#[test]
fn rejects_duplicate_mask_values() {
    let mut manifest = valid_keyboard_manifest();
    manifest.mask = Some(
        serde_json::from_value(serde_json::json!({
            "src": "mask.png",
            "channels": [
                { "value": 32, "region_id": "key.a" },
                { "value": 32, "region_id": "key.a" }
            ]
        }))
        .unwrap(),
    );

    assert_eq!(
        manifest.validate().unwrap_err(),
        HardwareAssetValidationError::DuplicateMaskValue(32)
    );
}
