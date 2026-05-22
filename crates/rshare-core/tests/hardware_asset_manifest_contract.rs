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
