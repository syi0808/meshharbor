mod runtime_bundle {
    include!(concat!(env!("OUT_DIR"), "/pixal3d_runtime_bundle.rs"));
}

mod cuda_runtime;
mod generation;
mod glb;
mod operation;
mod parts;
mod quadriflow;
mod retopology;
mod runtime;
mod store;
mod uv_bootstrap;

pub use cuda_runtime::Pixal3dCudaRuntimeManager;
pub use generation::{GenerationOptions, GenerationResult, Pixal3dMlxRuntime, RuntimeReadiness};
pub use glb::{
    cleanup_glb, compress_mesh_glb, inspect_asset, optimize_render_glb, retopology_glb,
    simplify_glb, AssetDescriptor, CompressionReport, MeshCleanupReport, RenderOptimizationReport,
    SimplificationReport,
};
pub use meshharbor_atlas_baker::{bake_glb, AtlasBakeError, AtlasBakeOptions, AtlasBakeReport};
pub use operation::{
    MeshOperationResult, OperationCore, OperationDescriptor, RuntimeDoctor, SimplificationResult,
};
pub use parts::{separate_components, PartSeparationReport};
pub use quadriflow::{parse_quad_obj_output, quad_mesh_preview_glb, QuadMesh, QuadMeshReport};
pub use retopology::{
    normalize_remesh_positions, remesh_triangles_in_worker, restore_remesh_positions,
    validate_remesh_request, validate_remesh_response, RemeshRequest, RemeshResponse,
    RetopologyOptions, RetopologyReport, REMESH_PROTOCOL_VERSION,
};
pub use runtime::{Pixal3dRuntimeManager, RuntimeSetupReport};
pub use store::{Artifact, Asset, AssetKind, CoreError, CoreResult, Job, JobState, Store};
