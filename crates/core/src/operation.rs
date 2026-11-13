use std::{path::Path, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Asset, AtlasBakeError, AtlasBakeOptions, CoreError, CoreResult, GenerationOptions,
    GenerationResult, Pixal3dCudaRuntimeManager, Pixal3dMlxRuntime, Pixal3dRuntimeManager,
    RuntimeSetupReport, SimplificationReport, Store, bake_glb, cleanup_glb, compress_mesh_glb,
    inspect_asset, optimize_render_glb, separate_components, simplify_glb,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationDescriptor {
    pub id: String,
    pub title: String,
    pub category: String,
    pub summary: String,
    pub implemented: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeDoctor {
    pub status: String,
    pub platform: String,
    pub operations: Vec<String>,
    pub model_backends: Vec<Value>,
    pub notes: Vec<String>,
}

#[derive(Clone)]
pub struct OperationCore {
    store: Arc<Store>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimplificationResult {
    pub asset: Asset,
    pub report: SimplificationReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshOperationResult {
    pub asset: Asset,
    pub report: Value,
}

impl OperationCore {
    pub fn open(root: impl AsRef<Path>) -> CoreResult<Self> {
        Ok(Self {
            store: Arc::new(Store::open(root)?),
        })
    }

    pub fn with_store(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn import_asset(&self, name: &str, bytes: &[u8]) -> CoreResult<Asset> {
        self.store.import_asset(name, bytes)
    }

    pub fn inspect_bytes(&self, name: &str, bytes: &[u8]) -> CoreResult<Value> {
        let descriptor = inspect_asset(name, bytes)?;
        Ok(
            json!({ "kind": descriptor.kind, "mime_type": descriptor.mime_type, "metadata": descriptor.metadata }),
        )
    }

    pub fn simplify_asset(
        &self,
        asset_id: &str,
        target_triangles: u64,
        max_error: f32,
    ) -> CoreResult<SimplificationResult> {
        let source = self
            .store
            .get_asset(asset_id)?
            .ok_or_else(|| CoreError::NotFound(format!("Asset '{asset_id}' was not found.")))?;
        if source.kind != crate::AssetKind::Mesh {
            return Err(CoreError::InvalidInput(
                "Simplification requires a GLB mesh asset.".into(),
            ));
        }
        let source_artifact = source
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == "source")
            .or_else(|| source.artifacts.first())
            .ok_or_else(|| {
                CoreError::NotFound(format!("Asset '{asset_id}' has no stored artifact."))
            })?;
        let bytes = self.store.read_artifact(&source_artifact.id)?.bytes;
        let (output, report) = simplify_glb(&bytes, target_triangles, max_error)?;
        let name = derived_name(&source.name);
        let report_value = serde_json::to_value(&report)?;
        let asset = self.store.import_derived_asset(
            &name,
            &output,
            asset_id,
            json!({
                "operation": "mesh.simplify",
                "simplification": report_value,
            }),
        )?;
        Ok(SimplificationResult { asset, report })
    }

    /// Run a local mesh operation and persist its output as a separate derived asset.
    /// The immutable source artifact is never overwritten.
    pub fn process_mesh_asset(
        &self,
        asset_id: &str,
        operation: &str,
        parameters: Value,
    ) -> CoreResult<MeshOperationResult> {
        let source = self
            .store
            .get_asset(asset_id)?
            .ok_or_else(|| CoreError::NotFound(format!("Asset '{asset_id}' was not found.")))?;
        if source.kind != crate::AssetKind::Mesh {
            return Err(CoreError::InvalidInput(
                "Mesh processing requires a GLB mesh asset.".into(),
            ));
        }
        let source_artifact = source
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == "source")
            .or_else(|| source.artifacts.first())
            .ok_or_else(|| {
                CoreError::NotFound(format!("Asset '{asset_id}' has no stored artifact."))
            })?;
        if operation == "mesh.separate_parts" {
            crate::parts::validate_part_glb_size(source_artifact.byte_size)?;
        }
        if operation == "mesh.retopology" {
            if !crate::quadriflow_sidecar_available() {
                return Err(CoreError::OperationUnavailable(
                    "QuadriFlow is not packaged. Install the pinned geometry-only sidecar beside the MeshHarbor executable.".into(),
                ));
            }
            if source_artifact.byte_size > 64 * 1024 * 1024 {
                return Err(CoreError::OperationUnavailable(
                    "Retopology accepts GLB files up to 64 MiB.".into(),
                ));
            }
        }
        let bytes = self.store.read_artifact(&source_artifact.id)?.bytes;
        if operation == "mesh.retopology" {
            let options: crate::QuadRetopologyOptions = serde_json::from_value(parameters)
                .map_err(|error| {
                    CoreError::InvalidInput(format!("Invalid mesh.retopology parameters: {error}"))
                })?;
            let (preview, quad_obj, report) = crate::quadriflow_retopology_glb(&bytes, options)?;
            let report_value = serde_json::to_value(&report)?;
            let name = derived_mesh_name(&source.name, "quad-retopologized");
            let asset = self.store.import_derived_asset_with_artifacts(
                &name,
                &preview,
                asset_id,
                json!({
                    "operation": operation,
                    "report": report_value.clone(),
                    "quad_topology_artifact": true,
                }),
                &[("quad_topology", "model/obj", &quad_obj)],
            )?;
            return Ok(MeshOperationResult {
                asset,
                report: report_value,
            });
        }
        let (suffix, output, report) = match operation {
            "mesh.cleanup" => {
                reject_unused_parameters(&parameters, operation)?;
                let (output, report) = cleanup_glb(&bytes)?;
                ("cleaned", output, serde_json::to_value(report)?)
            }
            "render.optimize" => {
                let options: RenderOptimizationOptions = serde_json::from_value(parameters)
                    .map_err(|error| {
                        CoreError::InvalidInput(format!(
                            "Invalid render.optimize parameters: {error}"
                        ))
                    })?;
                let (output, report) = optimize_render_glb(&bytes, options.overdraw_threshold)?;
                ("render-optimized", output, serde_json::to_value(report)?)
            }
            "mesh.compress" => {
                reject_unused_parameters(&parameters, operation)?;
                let (output, report) = compress_mesh_glb(&bytes)?;
                ("compressed", output, serde_json::to_value(report)?)
            }
            "mesh.separate_parts" => {
                reject_unused_parameters(&parameters, operation)?;
                let (output, report) = separate_components(&bytes)?;
                ("separated-parts", output, serde_json::to_value(report)?)
            }
            "uv.unwrap" => {
                let options: AtlasBakeOptions =
                    serde_json::from_value(parameters).map_err(|error| {
                        CoreError::InvalidInput(format!("Invalid uv.unwrap parameters: {error}"))
                    })?;
                let (output, report) = bake_glb(&bytes, options).map_err(map_atlas_bake_error)?;
                ("atlas-unwrapped", output, serde_json::to_value(report)?)
            }
            other => {
                return Err(CoreError::OperationUnavailable(format!(
                    "Mesh operation '{other}' is not implemented by this build."
                )));
            }
        };
        let name = derived_mesh_name(&source.name, suffix);
        let asset = self.store.import_derived_asset(
            &name,
            &output,
            asset_id,
            json!({
                "operation": operation,
                "report": report.clone(),
            }),
        )?;
        Ok(MeshOperationResult { asset, report })
    }

    pub fn generate_pixal3d_image(
        &self,
        input_asset_id: &str,
        options: GenerationOptions,
    ) -> CoreResult<GenerationResult> {
        crate::generation::generate_pixal3d_image(self, input_asset_id, options)
    }

    pub fn setup_pixal3d_mlx_environment(&self) -> CoreResult<RuntimeSetupReport> {
        Pixal3dRuntimeManager::new(self.store.root()).setup_environment()
    }

    pub fn install_pixal3d_mlx_models(
        &self,
        confirm_large_download: bool,
        confirm_gated_access: bool,
    ) -> CoreResult<RuntimeSetupReport> {
        Pixal3dRuntimeManager::new(self.store.root())
            .install_models(confirm_large_download, confirm_gated_access)
    }

    pub fn convert_pixal3d_mlx_naf(&self) -> CoreResult<RuntimeSetupReport> {
        Pixal3dRuntimeManager::new(self.store.root()).convert_naf()
    }

    pub fn login_pixal3d_mlx_huggingface(&self) -> CoreResult<()> {
        Pixal3dRuntimeManager::new(self.store.root()).login_huggingface()
    }

    pub fn setup_pixal3d_cuda_environment(&self) -> CoreResult<RuntimeSetupReport> {
        Pixal3dCudaRuntimeManager::new(self.store.root()).setup_environment()
    }

    pub fn install_pixal3d_cuda_models(
        &self,
        confirm_large_download: bool,
        confirm_gated_access: bool,
    ) -> CoreResult<RuntimeSetupReport> {
        Pixal3dCudaRuntimeManager::new(self.store.root())
            .install_models(confirm_large_download, confirm_gated_access)
    }

    pub fn login_pixal3d_cuda_huggingface(&self) -> CoreResult<()> {
        Pixal3dCudaRuntimeManager::new(self.store.root()).login_huggingface()
    }

    pub fn list_operations(&self) -> Vec<OperationDescriptor> {
        let generation_ready = Pixal3dMlxRuntime::discover(self.store.root())
            .readiness()
            .ready
            || Pixal3dCudaRuntimeManager::new(self.store.root())
                .readiness()
                .ready;
        let quadriflow_ready = crate::quadriflow_sidecar_available();
        vec![
            OperationDescriptor { id: "asset.import".into(), title: "Import asset".into(), category: "asset".into(), summary: "Import PNG, JPEG, or GLB files while retaining an immutable content-addressed source artifact.".into(), implemented: true },
            OperationDescriptor { id: "asset.inspect".into(), title: "Inspect asset".into(), category: "asset".into(), summary: "Inspect supported GLB metadata and triangle counts.".into(), implemented: true },
            OperationDescriptor { id: "image.generate".into(), title: "Generate 3D from image".into(), category: "generation".into(), summary: "Run a local Pixal3D 512 profile using the ready platform backend (MLX on Apple Silicon or CUDA on Linux/NVIDIA). Environment setup and model downloads are explicit; gated terms remain user-controlled.".into(), implemented: generation_ready },
            OperationDescriptor { id: "mesh.simplify".into(), title: "Simplify mesh".into(), category: "mesh".into(), summary: "Reduce indexed static GLB triangle primitives with meshoptimizer while retaining original vertex streams, materials, and textures.".into(), implemented: true },
            OperationDescriptor { id: "mesh.cleanup".into(), title: "Mesh cleanup".into(), category: "mesh".into(), summary: "Remove repeated-index, exactly zero-area, and same-winding duplicate faces while preserving seams, vertex streams, materials, and the source asset.".into(), implemented: true },
            OperationDescriptor { id: "mesh.separate_parts".into(), title: "Separate connected parts".into(), category: "mesh".into(), summary: "Split one embedded static indexed TRIANGLES GLB (max 64 MiB total, 4 MiB JSON, 1,000,000 POSITION vertices, 3,000,000 indices, 256 parts) into connected primitives in the same mesh; shared-index connectivity, not scene nodes.".into(), implemented: true },
            OperationDescriptor { id: "mesh.retopology".into(), title: "Quad retopology".into(), category: "mesh".into(), summary: "Experimental QuadriFlow quad remeshing through a packaged sidecar. Accepts one closed static triangle GLB; textured inputs also require TEXCOORD_0, optional NORMAL, and the narrow supported embedded base-color/metallic-roughness PNG maps. Those maps are projected only into a triangulated GLB preview with distance, normal, coverage, and ambiguity gates; the editable quad OBJ remains geometry-only. Other attributes/maps, self-intersection freedom, and a measured surface-error tolerance are not supported or certified.".into(), implemented: quadriflow_ready },
            OperationDescriptor { id: "uv.unwrap".into(), title: "Atlas UV unwrap".into(), category: "uv".into(), summary: "Use xatlas to unwrap one supported static textured mesh and rebake its embedded base-color and metallic/roughness PNG maps from the same mesh's existing UVs; this is not high-to-low projection.".into(), implemented: true },
            OperationDescriptor { id: "texture.bake".into(), title: "Texture bake".into(), category: "texture".into(), summary: "Generic high-to-low and arbitrary texture baking is not implemented; uv.unwrap only rebakes supported same-mesh embedded base-color and metallic/roughness PNG maps.".into(), implemented: false },
            OperationDescriptor { id: "render.optimize".into(), title: "Render optimization".into(), category: "mesh".into(), summary: "Use meshoptimizer vertex-cache and overdraw reordering on opaque/masked indexed triangles; translucent order is preserved.".into(), implemented: true },
            OperationDescriptor { id: "mesh.compress".into(), title: "Mesh compression".into(), category: "export".into(), summary: "Encode supported geometry bufferViews with required KHR_meshopt_compression while retaining the original editable source asset.".into(), implemented: true },
        ]
    }

    pub fn runtime_doctor(&self) -> RuntimeDoctor {
        let supported = self
            .list_operations()
            .into_iter()
            .filter(|item| item.implemented)
            .map(|item| item.id)
            .collect();
        let cuda_manager = Pixal3dCudaRuntimeManager::new(self.store.root());
        let cuda = Some(cuda_manager.readiness());
        let mlx = Some(Pixal3dMlxRuntime::discover(self.store.root()).readiness());
        let mut model_backends = Vec::new();
        if let Some(readiness) = &cuda {
            model_backends.push(json!({
                "id": readiness.backend_id,
                "ready": readiness.ready,
                "status": if readiness.ready { "ready_paths_unverified" }
                    else if !cuda_manager.host_supported() { "unsupported_host_or_nvidia_gpu_missing" }
                    else if !readiness.environment_ready { "environment_setup_required" }
                    else if readiness.models_downloaded { "model_files_present_environment_unverified" }
                    else { "model_download_required" },
                "validation_level": readiness.validation_level,
                "installable": cuda_manager.host_supported(),
                "weights_included": false,
                "downloads_enabled": readiness.downloads_enabled,
                "environment_ready": readiness.environment_ready,
                "model_files_ready": readiness.model_files_ready,
                "models_downloaded": readiness.models_downloaded,
                "gated_model_access_required": readiness.gated_model_access_required,
                "runtime_root": readiness.runtime_root,
                "python": readiness.python,
                "missing": readiness.missing,
                "license_notice": "MeshHarbor is MIT; upstream model/checkpoint terms are separate and remain pending review. User confirmation and gated access are required before model downloads.",
            }));
        }
        if let Some(readiness) = &mlx {
            model_backends.push(json!({
                "id": readiness.backend_id,
                "ready": readiness.ready,
                "status": if readiness.ready {
                    "ready_paths_unverified"
                } else if readiness.environment_ready {
                    if readiness.naf_conversion_required {
                        "naf_conversion_required"
                    } else if readiness.models_downloaded {
                        "model_files_present_environment_unverified"
                    } else {
                        "model_download_required"
                    }
                } else {
                    "environment_setup_required"
                },
                "validation_level": readiness.validation_level,
                "installable": cfg!(all(target_os = "macos", target_arch = "aarch64")),
                "weights_included": false,
                "downloads_enabled": readiness.downloads_enabled,
                "environment_ready": readiness.environment_ready,
                "model_files_ready": readiness.model_files_ready,
                "models_downloaded": readiness.models_downloaded,
                "naf_conversion_required": readiness.naf_conversion_required,
                "conversion_enabled": readiness.conversion_enabled,
                "runtime_root": readiness.runtime_root,
                "python": readiness.python,
                "gated_model_access_required": readiness.gated_model_access_required,
                "missing": readiness.missing,
            }));
        }
        RuntimeDoctor {
            status: "ready_for_mesh_operations".into(),
            platform: std::env::consts::OS.to_string(),
            operations: supported,
            model_backends,
            notes: vec![
                "The Rust core includes CPU mesh simplification, conservative cleanup, shared-index connected-part splitting, xatlas UV atlas rebaking, render cache/overdraw optimization, and required KHR_meshopt_compression output; this does not yet constitute a full cross-platform Studio or one-file distribution.".into(),
                "Connected-part splitting handles one indexed static TRIANGLES primitive, groups by shared vertex indices, and emits primitives within the same GLB mesh; it does not split scene nodes, weld duplicate positions, or retopologize.".into(),
                "Quad retopology is available only when the pinned QuadriFlow sidecar is packaged beside MeshHarbor. It accepts one closed static triangle mesh; textured input needs POSITION, TEXCOORD_0, optional NORMAL, and the supported embedded base-color/metallic-roughness PNG maps. Supported maps are projected with distance, facing-normal, coverage, and opposing-shell ambiguity gates into the triangulated GLB preview only; the separate OBJ retains quad topology without UVs or appearance. Other maps/attributes are rejected, and self-intersection freedom or measured surface-error tolerance is not certified.".into(),
                "xatlas UV rebaking is limited to one static mesh/primitive/material with embedded PNG base-color and metallic/roughness maps; it repacks the same mesh's existing appearance and is not high-to-low projection. Unsupported tangents, extensions, external images, and other material maps are rejected.".into(),
                "Pixal3D CUDA setup is Linux x86_64/NVIDIA only, runs a PyTorch CUDA/SDPA preflight, and resolves upstream's unpinned ranges into an app-private per-install uv.lock; this is not a repository-wide reproducibility guarantee. Model weights are a separate explicit 25–30 GB action. Pixal3D MLX remains Apple Silicon only. Neither backend readiness proves output quality or general GPU memory fit; the latest CUDA source has not been revalidated end-to-end on a GPU.".into(),
                "The MIT license covers MeshHarbor code, not upstream model weights or separate DINOv3/NAF terms. Gated access is user-controlled, and generated user inputs stay local to the worker.".into(),
            ],
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderOptimizationOptions {
    #[serde(default = "default_overdraw_threshold")]
    overdraw_threshold: f32,
}

fn default_overdraw_threshold() -> f32 {
    1.05
}

fn map_atlas_bake_error(error: AtlasBakeError) -> CoreError {
    match error {
        AtlasBakeError::InvalidInput(message) => CoreError::InvalidInput(message),
        AtlasBakeError::Unsupported(message) => CoreError::OperationUnavailable(message),
        AtlasBakeError::Native(message) | AtlasBakeError::Output(message) => {
            CoreError::OperationUnavailable(format!("Atlas UV bake failed: {message}"))
        }
    }
}

fn reject_unused_parameters(parameters: &Value, operation: &str) -> CoreResult<()> {
    if parameters
        .as_object()
        .is_some_and(|parameters| !parameters.is_empty())
    {
        return Err(CoreError::InvalidInput(format!(
            "{operation} does not accept parameters."
        )));
    }
    Ok(())
}

fn derived_mesh_name(name: &str, suffix: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}-{suffix}.{extension}"),
        None => format!("{name}-{suffix}.glb"),
    }
}

fn derived_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}-simplified.{extension}"),
        None => format!("{name}-simplified.glb"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn core_reports_only_implemented_operations_as_available() {
        let root = std::env::temp_dir().join(format!(
            "meshharbor-core-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let core = OperationCore::open(&root).unwrap();
        let doctor = core.runtime_doctor();
        assert!(doctor.operations.contains(&"mesh.simplify".to_string()));
        assert!(doctor.operations.contains(&"mesh.cleanup".to_string()));
        assert!(doctor.operations.contains(&"render.optimize".to_string()));
        assert!(doctor.operations.contains(&"mesh.compress".to_string()));
        assert!(
            doctor
                .operations
                .contains(&"mesh.separate_parts".to_string())
        );
        assert!(doctor.operations.contains(&"uv.unwrap".to_string()));
        assert_eq!(
            doctor.operations.contains(&"mesh.retopology".to_string()),
            crate::quadriflow_sidecar_available()
        );
        assert_eq!(
            core.list_operations()
                .iter()
                .find(|operation| operation.id == "mesh.retopology")
                .unwrap()
                .implemented,
            crate::quadriflow_sidecar_available()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
