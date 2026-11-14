mod runtime_bundle {
    include!(concat!(env!("OUT_DIR"), "/pixal3d_runtime_bundle.rs"));
}

mod cuda_runtime;
mod generation;
mod glb;
mod operation;
mod parts;
mod quadriflow;
mod quadriflow_attestation;
mod quadriflow_installer;
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
pub use quadriflow_installer::{
    QuadriFlowBackendStatus, QuadriFlowInstallError, QuadriFlowInstallResult,
    install_bundled_quadriflow_sidecar, installed_quadriflow_sidecar, quadriflow_backend_status,
    rollback_quadriflow_sidecar,
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

#[cfg(test)]
#[path = "../build_support/runtime_bundle.rs"]
mod runtime_bundle_build_tests;

#[cfg(test)]
mod runtime_bundle_integrity_tests {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn compiled_runtime_assets_match_the_pinned_allowlist() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../runtime_bundle_manifest.json")).unwrap();
        let files = manifest["files"].as_array().unwrap();
        let mut expected = BTreeMap::new();
        for file in files {
            let backend = file["backend"].as_str().unwrap();
            let path = file["path"].as_str().unwrap();
            let key = format!("{backend}/{path}");
            let hash = file["sha256"].as_str().unwrap().to_owned();
            let size = file["size_bytes"].as_u64().unwrap();
            assert!(
                expected.insert(key.clone(), (hash, size)).is_none(),
                "duplicate {key}"
            );
        }

        let mut compiled = BTreeMap::new();
        for (backend, assets) in [
            ("cuda", crate::runtime_bundle::PIXAL3D_CUDA_FILES),
            ("mlx", crate::runtime_bundle::PIXAL3D_MLX_FILES),
        ] {
            for (path, bytes) in assets {
                let key = format!("{backend}/{path}");
                let actual = (sha256_hex(bytes), bytes.len() as u64);
                assert!(
                    compiled.insert(key.clone(), actual).is_none(),
                    "duplicate {key}"
                );
            }
        }

        assert_eq!(compiled, expected);
    }
}
