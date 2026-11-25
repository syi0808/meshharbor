use std::{path::Path, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Asset, AssetKind, AtlasBakeError, AtlasBakeOptions, CoreError, CoreResult, GenerationInput,
    GenerationInputMode, GenerationOptions, GenerationResult, IMAGE_GENERATION_OPERATION,
    MODEL_GENERATION_OPERATION, Pixal3dCudaRuntimeManager, Pixal3dMlxRuntime, Pixal3dProfile,
    Pixal3dRuntimeManager, QuadriFlowBackendStatus, QuadriFlowInstallResult, RuntimeSetupReport,
    SimplificationReport, Store, bake_glb, cleanup_glb, compress_mesh_glb, external_generation,
    inspect_asset, install_bundled_quadriflow_sidecar, job_execution::JobExecution,
    model_storage::model_path, optimize_render_glb, separate_components, simplify_glb,
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
    pub model_catalog: Vec<Value>,
    pub quadriflow_backend: QuadriFlowBackendStatus,
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

    pub fn quadriflow_backend_status(&self) -> QuadriFlowBackendStatus {
        crate::quadriflow_installer::quadriflow_backend_status(self.store.data_root())
    }

    pub fn install_quadriflow_backend(&self) -> CoreResult<QuadriFlowInstallResult> {
        let status = self.quadriflow_backend_status();
        if !status.install_available {
            return Err(CoreError::OperationUnavailable(format!(
                "{} {}",
                status.message, status.next_step
            )));
        }
        install_bundled_quadriflow_sidecar(self.store.data_root())
            .map_err(|error| CoreError::OperationUnavailable(error.to_string()))
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
        self.simplify_asset_with_execution(
            asset_id,
            target_triangles,
            max_error,
            JobExecution::default(),
        )
    }

    pub fn simplify_asset_for_job(
        &self,
        asset_id: &str,
        target_triangles: u64,
        max_error: f32,
        job_id: &str,
    ) -> CoreResult<SimplificationResult> {
        self.simplify_asset_with_execution(
            asset_id,
            target_triangles,
            max_error,
            JobExecution::for_job(&self.store, job_id),
        )
    }

    fn simplify_asset_with_execution(
        &self,
        asset_id: &str,
        target_triangles: u64,
        max_error: f32,
        execution: JobExecution<'_>,
    ) -> CoreResult<SimplificationResult> {
        execution.stage("load_mesh")?;
        let source = self
            .store
            .get_asset(asset_id)?
            .ok_or_else(|| CoreError::NotFound(format!("Asset '{asset_id}' was not found.")))?;
        if source.kind != crate::AssetKind::Mesh {
            return Err(CoreError::InvalidInput(
                "Simplification requires a GLB mesh asset.".into(),
            ));
        }
        let source_artifact = mesh_source_artifact(&source)?;
        let bytes = self.store.read_artifact(&source_artifact.id)?.bytes;
        execution.stage("process_mesh")?;
        let (output, report) = simplify_glb(&bytes, target_triangles, max_error)?;
        let name = derived_name(&source.name);
        let report_value = serde_json::to_value(&report)?;
        execution.begin_commit()?;
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

    fn mesh_processing_source(&self, asset_id: &str) -> CoreResult<Asset> {
        let source = self
            .store
            .get_asset(asset_id)?
            .ok_or_else(|| CoreError::NotFound(format!("Asset '{asset_id}' was not found.")))?;
        if source.kind != crate::AssetKind::Mesh {
            return Err(CoreError::InvalidInput(
                "Mesh processing requires a GLB mesh asset.".into(),
            ));
        }
        Ok(source)
    }

    /// Check source eligibility using the same preparation as retopology.
    /// Does not consult or execute a backend, create a job, or store an output.
    pub fn preflight_retopology_asset(
        &self,
        asset_id: &str,
        options: crate::QuadRetopologyOptions,
    ) -> CoreResult<crate::QuadRetopologyPreflight> {
        let source = self.mesh_processing_source(asset_id)?;
        let source_artifact = mesh_source_artifact(&source)?;
        validate_retopology_source_size(source_artifact.byte_size)?;
        let bytes = self.store.read_artifact(&source_artifact.id)?.bytes;
        crate::quadriflow_preflight_glb(&bytes, options)
    }

    /// Run a local mesh operation and persist its output as a separate derived asset.
    /// The immutable source artifact is never overwritten.
    pub fn process_mesh_asset(
        &self,
        asset_id: &str,
        operation: &str,
        parameters: Value,
    ) -> CoreResult<MeshOperationResult> {
        self.process_mesh_asset_with_execution(
            asset_id,
            operation,
            parameters,
            JobExecution::default(),
        )
    }

    pub fn process_mesh_asset_for_job(
        &self,
        asset_id: &str,
        operation: &str,
        parameters: Value,
        job_id: &str,
    ) -> CoreResult<MeshOperationResult> {
        self.process_mesh_asset_with_execution(
            asset_id,
            operation,
            parameters,
            JobExecution::for_job(&self.store, job_id),
        )
    }

    fn process_mesh_asset_with_execution(
        &self,
        asset_id: &str,
        operation: &str,
        parameters: Value,
        execution: JobExecution<'_>,
    ) -> CoreResult<MeshOperationResult> {
        execution.stage("load_mesh")?;
        let source = self.mesh_processing_source(asset_id)?;
        let source_artifact = mesh_source_artifact(&source)?;
        if operation == "mesh.separate_parts" {
            crate::parts::validate_part_glb_size(source_artifact.byte_size)?;
        }
        if operation == "mesh.retopology" {
            let backend = self.quadriflow_backend_status();
            if !backend.available {
                return Err(CoreError::OperationUnavailable(format!(
                    "{} {}",
                    backend.message, backend.next_step
                )));
            }
            validate_retopology_source_size(source_artifact.byte_size)?;
        }
        let bytes = self.store.read_artifact(&source_artifact.id)?.bytes;
        execution.stage("process_mesh")?;
        if operation == "mesh.retopology" {
            let options: crate::QuadRetopologyOptions = serde_json::from_value(parameters)
                .map_err(|error| {
                    CoreError::InvalidInput(format!("Invalid mesh.retopology parameters: {error}"))
                })?;
            let (preview, quad_obj, report) = crate::quadriflow::quadriflow_retopology_glb_at(
                &bytes,
                options,
                self.store.data_root(),
            )?;
            let report_value = serde_json::to_value(&report)?;
            let name = derived_mesh_name(&source.name, "quad-retopologized");
            execution.begin_commit()?;
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
        execution.begin_commit()?;
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

    pub fn check_image_generation(&self, options: &GenerationOptions) -> CoreResult<()> {
        let profile = options.validated_profile()?;
        if !profile.input_modes.contains(&GenerationInputMode::Image) {
            return Err(CoreError::OperationUnavailable(format!(
                "{} does not support image-to-3D input.",
                profile.id
            )));
        }
        if let Some(external) = external_generation::ExternalProfile::find(profile.id) {
            external.require_ready(self.store.root())?;
        }
        Ok(())
    }

    pub fn check_model_generation(
        &self,
        input: &GenerationInput,
        options: &GenerationOptions,
    ) -> CoreResult<()> {
        let profile = options.validate_input(input)?;
        if let Some(asset_id) = input.asset_id() {
            let references = options
                .view_input
                .iter()
                .flat_map(|views| views.references.iter().skip(1))
                .map(|reference| reference.asset_id.as_str());
            for reference in std::iter::once(asset_id).chain(references) {
                let asset = self.store.get_asset(reference)?.ok_or_else(|| {
                    CoreError::NotFound(format!("Asset '{reference}' was not found."))
                })?;
                if asset.kind != AssetKind::Image {
                    return Err(CoreError::InvalidInput(
                        "Image generation requires imported PNG or JPEG reference assets.".into(),
                    ));
                }
            }
        }
        if let Some(external) = external_generation::ExternalProfile::find(profile.id) {
            external.require_ready(self.store.root())?;
        }
        Ok(())
    }

    pub fn generate_model(
        &self,
        input: &GenerationInput,
        options: GenerationOptions,
    ) -> CoreResult<GenerationResult> {
        crate::generation::generate_model(
            self,
            input,
            options,
            JobExecution::default(),
            MODEL_GENERATION_OPERATION,
        )
    }

    pub fn generate_model_for_job(
        &self,
        input: &GenerationInput,
        options: GenerationOptions,
        job_id: &str,
    ) -> CoreResult<GenerationResult> {
        crate::generation::generate_model(
            self,
            input,
            options,
            JobExecution::for_job(&self.store, job_id),
            MODEL_GENERATION_OPERATION,
        )
    }

    pub fn generate_image(
        &self,
        input_asset_id: &str,
        options: GenerationOptions,
    ) -> CoreResult<GenerationResult> {
        crate::generation::generate_model(
            self,
            &GenerationInput::Image {
                asset_id: input_asset_id.to_owned(),
            },
            options,
            JobExecution::default(),
            IMAGE_GENERATION_OPERATION,
        )
    }

    pub fn generate_image_for_job(
        &self,
        input_asset_id: &str,
        options: GenerationOptions,
        job_id: &str,
    ) -> CoreResult<GenerationResult> {
        crate::generation::generate_model(
            self,
            &GenerationInput::Image {
                asset_id: input_asset_id.to_owned(),
            },
            options,
            JobExecution::for_job(&self.store, job_id),
            IMAGE_GENERATION_OPERATION,
        )
    }

    pub fn setup_pixal3d_mlx_environment(&self) -> CoreResult<RuntimeSetupReport> {
        Pixal3dRuntimeManager::new(self.store.root()).setup_environment()
    }

    pub fn run_runtime_job(&self, kind: crate::RuntimeJobKind, job_id: &str) -> CoreResult<Value> {
        use crate::RuntimeJobKind;
        let root = self.store.root();
        let execution = JobExecution::for_job(&self.store, job_id);
        let report = match kind {
            RuntimeJobKind::Setup => {
                Pixal3dRuntimeManager::new(root).setup_environment_tracked(execution)
            }
            RuntimeJobKind::InstallModels => {
                Pixal3dRuntimeManager::new(root).install_models_tracked(true, true, execution)
            }
            RuntimeJobKind::ConvertNaf => {
                Pixal3dRuntimeManager::new(root).convert_naf_tracked(execution)
            }
            RuntimeJobKind::CudaSetup => {
                Pixal3dCudaRuntimeManager::new(root).setup_environment_tracked(execution)
            }
            RuntimeJobKind::CudaInstallModels => {
                Pixal3dCudaRuntimeManager::new(root).install_models_tracked(true, true, execution)
            }
            RuntimeJobKind::CudaInstallMultiview => Pixal3dCudaRuntimeManager::new(root)
                .install_multiview_models_tracked(true, execution),
            RuntimeJobKind::InstallCatalogModel(backend) => {
                return external_generation::ExternalProfile::find(backend)
                    .ok_or_else(|| CoreError::InvalidInput("Unknown generation backend.".into()))?
                    .install(root, execution);
            }
        }?;
        Ok(serde_json::to_value(report)?)
    }

    pub fn catalog_install_job(
        &self,
        backend: &str,
        confirm_gated_access: bool,
    ) -> CoreResult<crate::RuntimeJobKind> {
        let profile = external_generation::ExternalProfile::find(backend)
            .ok_or_else(|| CoreError::InvalidInput("Unknown generation backend.".into()))?;
        if profile.needs_dinov3() && !confirm_gated_access {
            return Err(CoreError::InvalidInput(
                "Confirm access to DINOv3 before installing TRELLIS.2.".into(),
            ));
        }
        if !profile.host_supported()
            || (profile.id.ends_with("-cuda")
                && !Pixal3dCudaRuntimeManager::new(self.store.root()).host_supported())
        {
            return Err(CoreError::OperationUnavailable(format!(
                "{} is not supported on this host.",
                profile.accelerator
            )));
        }
        Ok(crate::RuntimeJobKind::InstallCatalogModel(profile.id))
    }

    pub fn remove_model_files(&self, backend: &str) -> CoreResult<()> {
        let root = self.store.root();
        if let Some(profile) = external_generation::ExternalProfile::find(backend) {
            return profile.remove_models(root);
        }
        Pixal3dProfile::from_backend(backend)?.remove_models(root)
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
        let generation_profiles: Vec<_> = [
            (
                "pixal3d-mlx",
                Pixal3dMlxRuntime::discover(self.store.root())
                    .readiness()
                    .ready,
            ),
            (
                "pixal3d-cuda",
                Pixal3dCudaRuntimeManager::new(self.store.root())
                    .readiness()
                    .ready,
            ),
        ]
        .into_iter()
        .chain(
            external_generation::PROFILES
                .iter()
                .map(|profile| (profile.id, profile.is_ready(self.store.root()))),
        )
        .filter_map(|(id, ready)| {
            ready
                .then(|| crate::generation_catalog::generation_profile(id))
                .flatten()
        })
        .collect();
        let image_generation_ready = generation_profiles
            .iter()
            .any(|profile| profile.input_modes.contains(&GenerationInputMode::Image));
        let quadriflow_ready =
            crate::quadriflow::quadriflow_sidecar_available_at(self.store.data_root());
        vec![
            OperationDescriptor { id: "asset.import".into(), title: "Import asset".into(), category: "asset".into(), summary: "Import PNG, JPEG, or GLB files while retaining an immutable content-addressed source artifact.".into(), implemented: true },
            OperationDescriptor { id: "asset.inspect".into(), title: "Inspect asset".into(), category: "asset".into(), summary: "Inspect supported GLB metadata and triangle counts.".into(), implemented: true },
            OperationDescriptor { id: MODEL_GENERATION_OPERATION.into(), title: "Generate 3D model".into(), category: "generation".into(), summary: "Generate a local GLB with a ready backend that supports the requested input. Inference does not install packages or download weights.".into(), implemented: !generation_profiles.is_empty() },
            OperationDescriptor { id: IMAGE_GENERATION_OPERATION.into(), title: "Generate 3D from image".into(), category: "generation".into(), summary: "Compatibility operation for generating a local GLB from an imported image.".into(), implemented: image_generation_ready },
            OperationDescriptor { id: "mesh.simplify".into(), title: "Simplify mesh".into(), category: "mesh".into(), summary: "Reduce indexed static GLB triangles to a hard upper limit with meshoptimizer while retaining original vertex streams, materials, and textures. No result is saved if the limit cannot be met.".into(), implemented: true },
            OperationDescriptor { id: "mesh.cleanup".into(), title: "Mesh cleanup".into(), category: "mesh".into(), summary: "Remove repeated-index, exactly zero-area, and same-winding duplicate faces, then simplify eligible primitives within a conservative error tolerance without a triangle target.".into(), implemented: true },
            OperationDescriptor { id: "mesh.separate_parts".into(), title: "Separate connected parts".into(), category: "mesh".into(), summary: "Find components in one static indexed TRIANGLES primitive by shared indices and exact unambiguous edges. Emit up to 256 primitives, grouping excess fragments without discarding triangles. No vertex welding or scene-node split.".into(), implemented: true },
            OperationDescriptor { id: "mesh.retopology".into(), title: "Quad retopology".into(), category: "mesh".into(), summary: "Experimental QuadriFlow quad remeshing through a signature-verified, manifest-matched managed sidecar; adjacent executables are ignored. Accepts one closed static triangle GLB; textured inputs also require TEXCOORD_0, optional NORMAL, and the narrow supported embedded base-color/metallic-roughness PNG maps. Those maps are projected only into a triangulated GLB preview with distance, normal, coverage, and ambiguity gates; the editable quad OBJ remains geometry-only. Other attributes/maps, self-intersection freedom, and a measured surface-error tolerance are not supported or certified.".into(), implemented: quadriflow_ready },
            OperationDescriptor { id: "uv.unwrap".into(), title: "Atlas UV unwrap".into(), category: "uv".into(), summary: "Use xatlas to unwrap one supported static textured mesh and rebake its embedded base-color and metallic/roughness PNG maps from its existing UVs. Zero-area faces are omitted from the result. This is not high-to-low projection.".into(), implemented: true },
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
        let cuda_supported = cuda_manager.host_supported();
        let cuda = Some(cuda_manager.readiness());
        let mlx_runtime = Pixal3dMlxRuntime::discover(self.store.root());
        let mlx = Some(mlx_runtime.readiness());
        let shared_cache = crate::huggingface_download::shared_hub_cache();
        let mut model_backends = Vec::new();
        if let Some(readiness) = &cuda {
            model_backends.push(json!({
                "id": readiness.backend_id,
                "generation": crate::generation_catalog::generation_profile(&readiness.backend_id),
                "local_files": Pixal3dProfile::Cuda.local_files(self.store.root()),
                "account_tools_ready": readiness.environment_ready || crate::huggingface_browser::access_python(cuda_manager.runtime_root()).is_some(),
                "pixal3d_cache": crate::huggingface_download::pixal_cache_status(&cuda_manager.home_path(), shared_cache.as_deref()),
                "multiview_ready": readiness.ready && crate::huggingface_download::multiview_files_present(&cuda_manager.home_path()),
                "ready": readiness.ready,
                "status": if readiness.ready { "ready_paths_unverified" }
                    else if !cuda_supported { "unsupported_host_or_nvidia_gpu_missing" }
                    else if !readiness.environment_ready { "environment_setup_required" }
                    else if readiness.models_downloaded { "model_files_present_environment_unverified" }
                    else { "model_download_required" },
                "validation_level": readiness.validation_level,
                "installable": cuda_supported,
                "weights_included": false,
                "downloads_enabled": readiness.downloads_enabled,
                "environment_ready": readiness.environment_ready,
                "model_files_ready": readiness.model_files_ready,
                "models_downloaded": readiness.models_downloaded,
                "gated_model_access_required": readiness.gated_model_access_required,
                "runtime_root": readiness.runtime_root,
                "model_paths": [model_path(cuda_manager.home_path().join(".cache/huggingface/hub"))],
                "python": readiness.python,
                "missing": readiness.missing,
                "license_notice": "MeshHarbor's MIT license covers its own code. Upstream runtime and checkpoint terms remain separate and pending review. User confirmation and gated access are required before model downloads.",
            }));
        }
        if let Some(readiness) = &mlx {
            model_backends.push(json!({
                "id": readiness.backend_id,
                "generation": crate::generation_catalog::generation_profile(&readiness.backend_id),
                "local_files": Pixal3dProfile::Mlx.local_files(self.store.root()),
                "account_tools_ready": readiness.environment_ready || crate::huggingface_browser::access_python(Pixal3dRuntimeManager::new(self.store.root()).runtime_root()).is_some(),
                "pixal3d_cache": crate::huggingface_download::pixal_cache_status(&mlx_runtime.home, shared_cache.as_deref()),
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
                "model_paths": [model_path(mlx_runtime.weights.clone()), model_path(mlx_runtime.home.join(".cache/huggingface/hub"))],
                "python": readiness.python,
                "gated_model_access_required": readiness.gated_model_access_required,
                "missing": readiness.missing,
            }));
        }
        model_backends.extend(
            external_generation::PROFILES
                .iter()
                .map(|profile| profile.status(self.store.root(), cuda_supported)),
        );
        let pixal3d: Value =
            serde_json::from_str(include_str!("../../../registry/models/pixal3d.json"))
                .expect("bundled Pixal3D registry must be valid JSON");
        let pixal3d_backends = pixal3d["backends"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|backend| backend["id"].as_str())
            .map(|profile| format!("pixal3d-{profile}"))
            .collect::<Vec<_>>();
        RuntimeDoctor {
            status: "ready_for_mesh_operations".into(),
            platform: std::env::consts::OS.to_string(),
            operations: supported,
            model_backends,
            model_catalog: vec![
                json!({
                    "id": pixal3d["id"],
                    "title": pixal3d["title"],
                    "status": pixal3d["status"],
                    "backend_ids": pixal3d_backends,
                }),
                json!({
                    "id": "trellis2", "title": "TRELLIS.2", "status": "experimental_opt_in",
                    "backend_ids": ["trellis2-mlx", "trellis2-cuda"], "output": "textured_glb",
                }),
                json!({
                    "id": "triposg", "title": "TripoSG", "status": "experimental_opt_in",
                    "backend_ids": ["triposg-mlx", "triposg-cuda"], "output": "geometry_glb",
                }),
                json!({
                    "id": "trellis-text", "title": "TRELLIS Text Large", "status": "experimental_opt_in",
                    "backend_ids": [external_generation::TRELLIS_TEXT_MLX_ID, external_generation::TRELLIS_TEXT_CUDA_ID],
                    "output": "geometry_glb",
                }),
            ],
            quadriflow_backend: self.quadriflow_backend_status(),
            notes: vec![
                "The Rust core includes CPU mesh simplification, conservative cleanup, shared-index connected-part splitting, xatlas UV atlas rebaking, render cache/overdraw optimization, and required KHR_meshopt_compression output; this does not yet constitute a full cross-platform Studio or one-file distribution.".into(),
                "Connected-part splitting handles one indexed static TRIANGLES primitive, groups by shared vertex indices, and emits primitives within the same GLB mesh; it does not split scene nodes, weld duplicate positions, or retopologize.".into(),
                "QuadriFlow retopology is unavailable in this build: adjacent executables are ignored, and the in-process signature/provenance verifier has no approved sidecar to accept because the trusted manifest is empty. Backend status and an install request are exposed, but readiness and installation stay fail-closed until a verified managed release is available. It accepts one closed static triangle mesh; textured input needs POSITION, TEXCOORD_0, optional NORMAL, and the supported embedded base-color/metallic-roughness PNG maps. Supported maps are projected with distance, facing-normal, coverage, and opposing-shell ambiguity gates into the triangulated GLB preview only; the separate OBJ retains quad topology without UVs or appearance. Other maps/attributes are rejected, and self-intersection freedom or measured surface-error tolerance is not certified.".into(),
                "xatlas UV rebaking is limited to one static mesh/primitive/material with embedded PNG base-color and metallic/roughness maps; it repacks the same mesh's existing appearance and is not high-to-low projection. Unsupported tangents, extensions, external images, and other material maps are rejected.".into(),
                "Pixal3D CUDA setup is Linux x86_64/NVIDIA only, runs a PyTorch CUDA/SDPA preflight, and resolves upstream's unpinned ranges into an app-private per-install uv.lock; this is not a repository-wide reproducibility guarantee. Model weights are a separate explicit 25–30 GB action. Pixal3D MLX remains Apple Silicon only. Neither backend readiness proves output quality or general GPU memory fit; the latest CUDA source has not been revalidated end-to-end on a GPU.".into(),
                "TRELLIS Text, TRELLIS.2, and TripoSG are opt-in installs in Studio. The engine provisions an isolated environment and downloads pinned snapshots after explicit consent. Existing operator-provisioned runtimes are kept separately; readiness does not prove license compliance, memory fit, or inference quality.".into(),
                "MeshHarbor's MIT license covers its own code. Third-party runtime and model terms, including DINOv3 and NAF, remain separate; gated access is user-controlled. See THIRD-PARTY-NOTICES.md.".into(),
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

fn mesh_source_artifact(source: &Asset) -> CoreResult<&crate::Artifact> {
    source
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == "source")
        .or_else(|| source.artifacts.first())
        .ok_or_else(|| {
            CoreError::NotFound(format!("Asset '{}' has no stored artifact.", source.id))
        })
}

fn validate_retopology_source_size(byte_size: u64) -> CoreResult<()> {
    if byte_size > crate::quadriflow::MAX_QUADRIFLOW_INPUT_GLB_BYTES as u64 {
        return Err(CoreError::OperationUnavailable(
            "Retopology accepts GLB files up to 64 MiB.".into(),
        ));
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
            crate::quadriflow::quadriflow_sidecar_available_at(core.store.data_root())
        );
        assert_eq!(
            core.list_operations()
                .iter()
                .find(|operation| operation.id == "mesh.retopology")
                .unwrap()
                .implemented,
            crate::quadriflow::quadriflow_sidecar_available_at(core.store.data_root())
        );
        assert_eq!(
            doctor.quadriflow_backend.available,
            crate::quadriflow::quadriflow_sidecar_available_at(core.store.data_root())
        );
        assert!(!doctor.quadriflow_backend.available);
        if !doctor.quadriflow_backend.install_available {
            assert!(matches!(
                core.install_quadriflow_backend(),
                Err(CoreError::OperationUnavailable(_))
            ));
            assert!(!root.join("backends").exists());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
