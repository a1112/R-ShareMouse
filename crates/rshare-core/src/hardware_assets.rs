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
                return Err(HardwareAssetValidationError::InvalidGeometry(
                    region.id.clone(),
                ));
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
                points.len() >= 3
                    && points
                        .iter()
                        .all(|point| finite_unit(point.x) && finite_unit(point.y))
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
