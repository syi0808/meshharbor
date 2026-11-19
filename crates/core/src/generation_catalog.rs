use serde::Serialize;

use crate::{CoreError, CoreResult, GenerationOptions};

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

const PIXAL3D_PARAMETERS: &[Parameter] = &[RESOLUTION, TEXTURE_SIZE, TARGET_FACES, SEED];

#[derive(Serialize)]
pub(crate) struct GenerationProfile {
    pub id: &'static str,
    label: &'static str,
    parameters: &'static [Parameter],
}

const PROFILES: &[GenerationProfile] = &[
    GenerationProfile {
        id: "pixal3d-mlx",
        label: "Apple MLX",
        parameters: PIXAL3D_PARAMETERS,
    },
    GenerationProfile {
        id: "pixal3d-cuda",
        label: "NVIDIA CUDA",
        parameters: PIXAL3D_PARAMETERS,
    },
];

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
        for parameter in profile.parameters {
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
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_defaults_are_accepted_by_each_profile() {
        let defaults = GenerationOptions::default();
        for profile in PROFILES {
            assert!(defaults.validate_for_backend(profile.id).is_ok());
            for parameter in profile.parameters {
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
}
