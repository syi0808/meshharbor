mod runtime_bundle {
    include!(concat!(env!("OUT_DIR"), "/pixal3d_runtime_bundle.rs"));
}

mod generation;
mod cuda_runtime;
mod glb;
mod operation;
mod parts;
mod runtime;
mod uv_bootstrap;
mod store;

pub use cuda_runtime::Pixal3dCudaRuntimeManager;
pub use generation::{GenerationOptions, GenerationResult, Pixal3dMlxRuntime, RuntimeReadiness};
pub use glb::{
    AssetDescriptor, CompressionReport, MeshCleanupReport, RenderOptimizationReport,
    SimplificationReport, cleanup_glb, compress_mesh_glb, inspect_asset, optimize_render_glb,
    simplify_glb,
};
pub use meshharbor_atlas_baker::{AtlasBakeError, AtlasBakeOptions, AtlasBakeReport, bake_glb};
pub use parts::{PartSeparationReport, separate_components};
pub use operation::{MeshOperationResult, OperationCore, OperationDescriptor, RuntimeDoctor, SimplificationResult};
pub use runtime::{Pixal3dRuntimeManager, RuntimeSetupReport};
pub use store::{Artifact, Asset, AssetKind, CoreError, CoreResult, Job, JobState, Store};

