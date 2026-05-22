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
