use std::sync::LazyLock;

use serde::Serialize;
use serde_json::Value;

use crate::{
    CoreError, CoreResult, GenerationOptions, external_generation, generation_tuning,
    generation_views::{GenerationViewMode, MAX_CUSTOM_VIEWS},
};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ParameterId {
    Seed,
    Resolution,
    TextureSize,
    TargetFaces,
}

impl ParameterId {
    fn key(self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::Resolution => "resolution",
            Self::TextureSize => "texture_size",
            Self::TargetFaces => "target_faces",
        }
    }

    fn value(self, options: &GenerationOptions) -> u64 {
        match self {
            Self::Seed => options.seed,
            Self::Resolution => options.resolution.into(),
            Self::TextureSize => options.texture_size.into(),
            Self::TargetFaces => options.target_faces.into(),
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Constraint {
    Fixed,
    Choice {
        values: &'static [u64],
    },
    Integer {
        min: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<u64>,
    },
}

#[derive(Clone, Copy, Serialize)]
struct Parameter {
    id: ParameterId,
    label: &'static str,
    description: &'static str,
    default: u64,
    #[serde(flatten)]
    constraint: Constraint,
}

const SEED: Parameter = Parameter {
    id: ParameterId::Seed,
    label: "Seed",
    description: "Use the same seed to repeat sampling with the same model and settings.",
    default: 42,
    constraint: Constraint::Integer { min: 0, max: None },
};
const RESOLUTION: Parameter = Parameter {
    id: ParameterId::Resolution,
    label: "Geometry resolution",
    description: "Fixed by this generation profile.",
    default: 512,
    constraint: Constraint::Fixed,
};
const TEXTURE_SIZE: Parameter = Parameter {
    id: ParameterId::TextureSize,
    label: "Texture size",
    description: "Higher resolutions use more memory.",
    default: 1024,
    constraint: Constraint::Choice {
        values: &[512, 1024, 2048],
    },
};
const TARGET_FACES: Parameter = Parameter {
    id: ParameterId::TargetFaces,
    label: "Soft face target",
    description: "The generated mesh may contain more faces than requested.",
    default: 100_000,
    constraint: Constraint::Integer {
        min: 4,
        max: Some(u32::MAX as u64),
    },
};

const TEXTURED_PARAMETERS: &[Parameter] = &[RESOLUTION, TEXTURE_SIZE, TARGET_FACES, SEED];
const GEOMETRY_PARAMETERS: &[Parameter] = &[RESOLUTION, TARGET_FACES, SEED];

#[derive(Serialize)]
pub(crate) struct GenerationProfile {
    pub id: &'static str,
    label: &'static str,
    parameters: Vec<Value>,
    #[serde(skip)]
    core_parameters: &'static [Parameter],
    view_modes: &'static [GenerationViewMode],
    max_views: usize,
}

static PROFILES: LazyLock<Vec<GenerationProfile>> = LazyLock::new(|| {
    use GenerationViewMode::{Custom, Four, Single, Six};
    [
        ("pixal3d-mlx", "Apple MLX", &[Single][..], 1),
        (
            "pixal3d-cuda",
            "NVIDIA CUDA",
            &[Single, Four, Six, Custom][..],
            MAX_CUSTOM_VIEWS,
        ),
    ]
    .into_iter()
    .map(|(id, label, views, max_views)| profile(id, label, views, max_views, true))
    .chain(external_generation::PROFILES.iter().map(|backend| {
        profile(
            backend.id,
            backend.accelerator,
            &[Single],
            1,
            backend.textured,
        )
    }))
    .collect()
});

fn profile(
    id: &'static str,
    label: &'static str,
    view_modes: &'static [GenerationViewMode],
    max_views: usize,
    textured: bool,
) -> GenerationProfile {
    let core_parameters = if textured {
        TEXTURED_PARAMETERS
    } else {
        GEOMETRY_PARAMETERS
    };
    GenerationProfile {
        id,
        label,
        parameters: core_parameters
            .iter()
            .map(|parameter| serde_json::to_value(parameter).expect("bundled parameter"))
            .chain(generation_tuning::definitions(id))
            .collect(),
        core_parameters,
        view_modes,
        max_views,
    }
}

impl GenerationProfile {
    pub fn has_textures(&self) -> bool {
        self.core_parameters
            .iter()
            .any(|parameter| matches!(parameter.id, ParameterId::TextureSize))
    }
}

pub(crate) fn generation_profile(backend_id: &str) -> Option<&'static GenerationProfile> {
    PROFILES.iter().find(|profile| profile.id == backend_id)
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            backend_id: "auto".into(),
            seed: SEED.default,
            resolution: RESOLUTION.default as u16,
            texture_size: TEXTURE_SIZE.default as u16,
            target_faces: TARGET_FACES.default as u32,
            tuning: Default::default(),
            view_input: None,
        }
    }
}

impl GenerationOptions {
    pub fn validate(&self) -> CoreResult<()> {
        self.validated_profile().map(|_| ())
    }

    pub(crate) fn validated_profile(&self) -> CoreResult<&'static GenerationProfile> {
        let backend_id = if self.backend_id != "auto" {
            &self.backend_id
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            "pixal3d-cuda"
        } else {
            "pixal3d-mlx"
        };
        self.validate_for_backend(backend_id)
    }

    pub(crate) fn validate_for_backend(
        &self,
        backend_id: &str,
    ) -> CoreResult<&'static GenerationProfile> {
        let profile = generation_profile(backend_id).ok_or_else(|| {
            CoreError::InvalidInput(format!("Unknown generation backend '{backend_id}'."))
        })?;
        if !profile.has_textures() && u64::from(self.texture_size) != TEXTURE_SIZE.default {
            return Err(CoreError::InvalidInput(format!(
                "{backend_id} generates geometry only and does not support texture_size."
            )));
        }
        for parameter in profile.core_parameters {
            let value = parameter.id.value(self);
            let key = parameter.id.key();
            let problem = match parameter.constraint {
                Constraint::Fixed if value != parameter.default => {
                    format!("{key} is fixed at {} for {backend_id}.", parameter.default)
                }
                Constraint::Choice { values } if !values.contains(&value) => {
                    format!("{key} must be one of {values:?} for {backend_id}.")
                }
                Constraint::Integer { min, .. } if value < min => {
                    format!("{key} must be at least {min} for {backend_id}.")
                }
                Constraint::Integer { max: Some(max), .. } if value > max => {
                    format!("{key} must be at most {max} for {backend_id}.")
                }
                _ => continue,
            };
            return Err(CoreError::InvalidInput(problem));
        }
        if let Some(input) = &self.view_input {
            if !profile.view_modes.contains(&input.mode) {
                return Err(CoreError::InvalidInput(format!(
                    "{backend_id} does not support the requested view mode."
                )));
            }
            input.validate()?;
        }
        generation_tuning::validate(self, backend_id)?;
        Ok(profile)
    }

    pub fn validate_primary_image(&self, asset_id: &str) -> CoreResult<()> {
        if let Some(input) = &self.view_input {
            if input
                .references
                .first()
                .is_none_or(|reference| reference.asset_id != asset_id)
            {
                return Err(CoreError::InvalidInput(
                    "The first view must match the job’s primary image.".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation_views::{GenerationReference, OrbitCamera, PosedGenerationInput};
    use serde_json::json;

    #[test]
    fn catalog_defaults_are_accepted_by_each_profile() {
        let defaults = GenerationOptions::default();
        for profile in PROFILES.iter() {
            assert!(defaults.validate_for_backend(profile.id).is_ok());
            let mut explicit = defaults.clone();
            for setting in profile
                .parameters
                .iter()
                .filter(|setting| setting["tuning"] == true)
            {
                explicit.tuning.insert(
                    setting["id"].as_str().unwrap().into(),
                    setting["default"].clone(),
                );
            }
            explicit.validate_for_backend(profile.id).unwrap();
            for parameter in profile.core_parameters {
                assert_eq!(parameter.id.value(&defaults), parameter.default);
            }
            for invalid in [
                GenerationOptions {
                    resolution: 1024,
                    ..defaults.clone()
                },
                GenerationOptions {
                    texture_size: 768,
                    ..defaults.clone()
                },
                GenerationOptions {
                    target_faces: 3,
                    ..defaults.clone()
                },
            ] {
                assert!(invalid.validate_for_backend(profile.id).is_err());
            }
        }
        assert!(
            serde_json::from_value::<GenerationOptions>(serde_json::json!({"unknown": 1})).is_err()
        );
    }

    #[test]
    fn posed_view_contract_rejects_unsupported_modes_and_ambiguous_camera_mapping() {
        let mut options = GenerationOptions {
            backend_id: "pixal3d-cuda".into(),
            view_input: Some(PosedGenerationInput {
                mode: GenerationViewMode::Six,
                references: (0..6)
                    .map(|index| GenerationReference {
                        asset_id: format!("asset-{index}"),
                        camera: None,
                    })
                    .collect(),
            }),
            ..GenerationOptions::default()
        };
        assert!(options.validate().is_ok());
        assert!(options.validate_for_backend("pixal3d-mlx").is_err());
        assert!(options.validate_primary_image("asset-1").is_err());
        let input = options.view_input.as_mut().unwrap();
        let frames = input.camera_metadata();
        assert_eq!(frames["frames"][1]["azimuth_degrees"], json!(90.0));
        assert_eq!(frames["frames"][4]["elevation_degrees"], json!(90.0));
        assert_eq!(frames["frames"][5]["elevation_degrees"], json!(-90.0));
        input.references.truncate(2);
        assert!(input.validate().is_err());
        input.mode = GenerationViewMode::Custom;
        assert!(input.validate().is_err());
        for (index, view) in input.references.iter_mut().enumerate() {
            view.camera = Some(OrbitCamera {
                azimuth_degrees: index as f64 * 30.0,
                elevation_degrees: 10.0,
                distance: 3.0,
                fov_degrees: 20.0,
            });
        }
        assert_eq!(
            input.camera_metadata()["frames"][1]["azimuth_degrees"],
            json!(30.0)
        );
        assert!(options.validate().is_ok());
        options.tuning.insert("fov_degrees".into(), json!(49.13));
        assert!(options.validate().is_err());
    }
}
