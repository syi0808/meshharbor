mod runtime_bundle {
    include!(concat!(env!("OUT_DIR"), "/pixal3d_runtime_bundle.rs"));
}

mod cuda_runtime;
mod generation;
mod glb;
mod operation;
mod parts;
mod quadriflow;
mod quadriflow_manifest;
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
pub use quadriflow::{
    QuadMesh, QuadMeshReport, QuadRetopologyOptions, QuadRetopologyReport, parse_quad_obj_output,
    quad_mesh_preview_glb, quadriflow_retopology_glb, quadriflow_sidecar_available,
};
pub use quadriflow_manifest::{
    QuadriFlowArchive, QuadriFlowArchiveFormat, QuadriFlowManifestError, QuadriFlowPayload,
    QuadriFlowPayloadFile, QuadriFlowPlatform, QuadriFlowReleaseArtifact,
    QuadriFlowReleaseManifest, QuadriFlowSource, QuadriFlowWorkflowEvidence,
    bundled_quadriflow_release_manifest, parse_quadriflow_release_manifest,
    validate_quadriflow_release_manifest, verify_quadriflow_payload_file,
};
pub use retopology::{
    REMESH_PROTOCOL_VERSION, RemeshRequest, RemeshResponse, RetopologyOptions, RetopologyReport,
    normalize_remesh_positions, remesh_triangles_in_worker, restore_remesh_positions,
    validate_remesh_request, validate_remesh_response,
};
pub use runtime::{Pixal3dRuntimeManager, RuntimeSetupReport};
pub use store::{Artifact, Asset, AssetKind, CoreError, CoreResult, Job, JobState, Store};
