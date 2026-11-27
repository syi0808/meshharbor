use meshharbor_core::{GenerationInput, GenerationOptions, Pixal3dProfile};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GenerateModel {
    pub input: GenerationInput,
    #[serde(default)]
    pub parameters: GenerationOptions,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, clap::Args)]
#[serde(deny_unknown_fields)]
pub(crate) struct SimplifyMesh {
    #[arg(long)]
    pub target_triangles: u64,
    #[arg(long, default_value_t = crate::default_max_error())]
    #[serde(default = "crate::default_max_error")]
    pub max_error: f32,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema, clap::Args)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CleanupMesh {
    #[arg(long)]
    pub for_retopology: bool,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema, clap::Args)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct VoxelRemesh {
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<u32>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema, clap::Args)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct UnwrapUv {
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<u32>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub padding: Option<u32>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema, clap::Args)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct OptimizeRendering {
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overdraw_threshold: Option<f32>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema, clap::Args)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RetopologizeLato2 {
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_id: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_triangles: Option<usize>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atlas_resolution: Option<u32>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompressMesh {}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuntimePackage {
    #[default]
    Base,
    Multiview,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstallBackend {
    pub confirmed: bool,
    #[serde(default)]
    pub confirm_gated_access: bool,
    #[serde(default)]
    pub package: RuntimePackage,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RetryJob {
    #[serde(default)]
    pub confirm_download: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountScope {
    Mlx,
    Cuda,
}

impl AccountScope {
    pub(crate) fn segment(self) -> &'static str {
        match self {
            Self::Mlx => "mlx",
            Self::Cuda => "cuda",
        }
    }
}

impl From<AccountScope> for Pixal3dProfile {
    fn from(scope: AccountScope) -> Self {
        match scope {
            AccountScope::Mlx => Self::Mlx,
            AccountScope::Cuda => Self::Cuda,
        }
    }
}
