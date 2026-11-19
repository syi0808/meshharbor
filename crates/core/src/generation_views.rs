use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{CoreError, CoreResult};

pub(crate) const MAX_CUSTOM_VIEWS: usize = 8;
const PRESET_DISTANCE: f64 = 3.1192049980163574;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationViewMode {
    Single,
    Four,
    Six,
    Custom,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PosedGenerationInput {
    pub mode: GenerationViewMode,
    pub references: Vec<GenerationReference>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationReference {
    pub asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<OrbitCamera>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrbitCamera {
    pub azimuth_degrees: f64,
    pub elevation_degrees: f64,
    pub distance: f64,
    pub fov_degrees: f64,
}

impl PosedGenerationInput {
    pub(crate) fn validate(&self) -> CoreResult<()> {
        let count = self.references.len();
        let valid_count = match self.mode {
            GenerationViewMode::Single => false,
            GenerationViewMode::Four => count == 4,
            GenerationViewMode::Six => count == 6,
            GenerationViewMode::Custom => (2..=MAX_CUSTOM_VIEWS).contains(&count),
        };
        if !valid_count {
            return Err(CoreError::InvalidInput(
                "Provide exactly four or six images for a preset, or two to eight for custom views.".into(),
            ));
        }
        let mut ids = HashSet::new();
        for reference in &self.references {
            if reference.asset_id.is_empty() || !ids.insert(&reference.asset_id) {
                return Err(CoreError::InvalidInput(
                    "Each view needs a distinct imported image.".into(),
                ));
            }
            match (&reference.camera, self.mode) {
                (Some(camera), GenerationViewMode::Custom)
                    if (-360.0..=360.0).contains(&camera.azimuth_degrees)
                        && (-90.0..=90.0).contains(&camera.elevation_degrees)
                        && (0.01..=1000.0).contains(&camera.distance)
                        && (1.0..=179.0).contains(&camera.fov_degrees) => {}
                (None, GenerationViewMode::Four | GenerationViewMode::Six) => {}
                _ => return Err(CoreError::InvalidInput(
                    "Custom views require azimuth (−360–360°), elevation (−90–90°), distance (0.01–1000), and FOV (1–179°); presets supply their own cameras.".into(),
                )),
            }
        }
        Ok(())
    }

    pub(crate) fn camera_metadata(&self) -> Value {
        let positions = [
            (0.0, 0.0),
            (90.0, 0.0),
            (180.0, 0.0),
            (270.0, 0.0),
            (0.0, 90.0),
            (0.0, -90.0),
        ];
        let frames = self
            .references
            .iter()
            .enumerate()
            .map(|(index, reference)| {
                let camera = match &reference.camera {
                    Some(camera) => camera.clone(),
                    None => OrbitCamera {
                        azimuth_degrees: positions[index].0,
                        elevation_degrees: positions[index].1,
                        distance: PRESET_DISTANCE,
                        fov_degrees: 20.0,
                    },
                };
                json!({
                    "image_index": index,
                    "azimuth_degrees": camera.azimuth_degrees,
                    "elevation_degrees": camera.elevation_degrees,
                    "distance": camera.distance,
                    "fov_degrees": camera.fov_degrees,
                })
            })
            .collect::<Vec<_>>();
        json!({ "frames": frames })
    }
}
