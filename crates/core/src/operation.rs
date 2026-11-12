use std::{path::Path, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Asset, CoreError, CoreResult, SimplificationReport, Store, inspect_asset, simplify_glb,
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

    pub fn list_operations(&self) -> Vec<OperationDescriptor> {
        vec![
            OperationDescriptor { id: "asset.import".into(), title: "Import asset".into(), category: "asset".into(), summary: "Import PNG, JPEG, or GLB files while retaining an immutable content-addressed source artifact.".into(), implemented: true },
            OperationDescriptor { id: "asset.inspect".into(), title: "Inspect asset".into(), category: "asset".into(), summary: "Inspect supported GLB metadata and triangle counts.".into(), implemented: true },
            OperationDescriptor { id: "mesh.simplify".into(), title: "Simplify mesh".into(), category: "mesh".into(), summary: "Reduce indexed static GLB triangle primitives with meshoptimizer while retaining original vertex streams, materials, and textures.".into(), implemented: true },
            OperationDescriptor { id: "mesh.cleanup".into(), title: "Mesh cleanup".into(), category: "mesh".into(), summary: "Conservative cleanup with seam-, material-, and boundary-preservation checks.".into(), implemented: false },
            OperationDescriptor { id: "mesh.retopology".into(), title: "Retopology".into(), category: "mesh".into(), summary: "Reconstruct mesh topology; no implementation has passed dependency and quality gates.".into(), implemented: false },
            OperationDescriptor { id: "uv.unwrap".into(), title: "UV unwrap".into(), category: "uv".into(), summary: "Generate UV charts for supported meshes using a validated xatlas integration.".into(), implemented: false },
            OperationDescriptor { id: "texture.bake".into(), title: "Texture bake".into(), category: "texture".into(), summary: "Reproject supported PBR materials onto a new UV layout.".into(), implemented: false },
            OperationDescriptor { id: "render.optimize".into(), title: "Render optimization".into(), category: "mesh".into(), summary: "Optimize vertex-cache, overdraw, and vertex-fetch order without changing geometry.".into(), implemented: false },
            OperationDescriptor { id: "mesh.compress".into(), title: "Mesh compression".into(), category: "export".into(), summary: "Compress mesh payloads while retaining a validated editable source.".into(), implemented: false },
        ]
    }

    pub fn runtime_doctor(&self) -> RuntimeDoctor {
        let supported = self
            .list_operations()
            .into_iter()
            .filter(|item| item.implemented)
            .map(|item| item.id)
            .collect();
        RuntimeDoctor {
            status: "ready_for_mesh_operations".into(),
            platform: std::env::consts::OS.to_string(),
            operations: supported,
            model_backends: vec![
                json!({"id":"pixal3d-cuda", "status":"source_only_vendored", "path":"runtimes/pixal3d/cuda", "installable":false, "weights_included":false}),
                json!({"id":"pixal3d-mlx", "status":"source_only_vendored", "path":"runtimes/pixal3d/mlx", "installable":false, "weights_included":false}),
            ],
            notes: vec![
                "The Rust core and CPU mesh simplification are implemented; this does not yet constitute the full cross-platform Studio or one-file distribution.".into(),
                "Atlas UV rebaking remains experimental and is not part of the product simplification path; mesh vertex data, material references, and embedded texture payloads are not rewritten, while external image URIs are not fetched.".into(),
                "CUDA and MLX code are not installed or executed by this build, and no model weights are included.".into(),
            ],
        }
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
        assert!(!doctor.operations.contains(&"uv.unwrap".to_string()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
