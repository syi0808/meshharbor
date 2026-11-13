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
    AssetDescriptor, CompressionReport, MeshCleanupReport, RenderOptimizationReport,
    SimplificationReport, cleanup_glb, compress_mesh_glb, inspect_asset, optimize_render_glb,
    retopology_glb, simplify_glb,
};
pub use meshharbor_atlas_baker::{AtlasBakeError, AtlasBakeOptions, AtlasBakeReport, bake_glb};
pub use operation::{
    MeshOperationResult, OperationCore, OperationDescriptor, RuntimeDoctor, SimplificationResult,
};
pub use parts::{PartSeparationReport, separate_components};
pub use quadriflow::{QuadMesh, QuadMeshReport, parse_quad_obj_output, quad_mesh_preview_glb};
pub use retopology::{
    REMESH_PROTOCOL_VERSION, RemeshRequest, RemeshResponse, RetopologyOptions, RetopologyReport,
    normalize_remesh_positions, remesh_triangles_in_worker, restore_remesh_positions,
    validate_remesh_request, validate_remesh_response,
};
pub use runtime::{Pixal3dRuntimeManager, RuntimeSetupReport};
pub use store::{Artifact, Asset, AssetKind, CoreError, CoreResult, Job, JobState, Store};
