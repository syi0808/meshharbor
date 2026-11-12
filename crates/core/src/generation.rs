//! Optional, manually provisioned Pixal3D MLX process integration.
//!
//! This module deliberately never installs dependencies, downloads weights, or
//! performs Hugging Face authentication. The user provisions the pinned
//! upstream source, its Python environment, and model checkpoints themselves.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{Asset, AssetKind, CoreError, CoreResult, OperationCore, inspect_asset};

const PIXAL_REVISION: &str = "0b31f9160aa400719af409098bff7936a932f726";
const DINO_REPOSITORY: &str = "models--facebook--dinov3-vitl16-pretrain-lvd1689m";
const PIXAL_REPOSITORY: &str = "models--TencentARC--Pixal3D";
const DINO_REVISION: &str = "ea8dc2863c51be0a264bab82070e3e8836b02d51";

const PIXAL_CHECKPOINTS: &[&str] = &[
    "ss_flow_img_dit_1_3B_64_bf16.safetensors",
    "ss_dec_conv3d_16l8_fp16.safetensors",
    "slat_flow_img2shape_dit_1_3B_512_bf16.safetensors",
    "shape_dec_next_dc_f16c32_fp16.safetensors",
    "slat_flow_img2shape_dit_1_3B_1024_bf16.safetensors",
    "slat_flow_imgshape2tex_dit_1_3B_1024_bf16.safetensors",
    "tex_dec_next_dc_f16c32_fp16.safetensors",
];
const MAX_GENERATED_GLB_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GenerationOptions {
    /// Reproducible sampler seed.
    pub seed: u64,
    /// The vendored MLX backend currently supports the validated 512 cascade.
    pub resolution: u16,
    /// Output PBR texture resolution.
    pub texture_size: u16,
    /// Soft triangle target. The model may return more faces.
    pub target_faces: u32,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            seed: 42,
            resolution: 512,
            texture_size: 1024,
            target_faces: 100_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeReadiness {
    pub backend_id: String,
    pub ready: bool,
    /// Readiness checks required local paths only; no Python imports are run.
    pub validation_level: String,
    pub runtime_root: Option<PathBuf>,
    pub python: Option<PathBuf>,
    pub missing: Vec<String>,
    pub downloads_enabled: bool,
    pub gated_model_access_required: bool,
}

/// User-provisioned MLX runtime profile.
#[derive(Debug, Clone)]
pub struct Pixal3dMlxRuntime {
    pub root: PathBuf,
    pub python: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationResult {
    pub asset: Asset,
    pub backend_id: String,
    pub seed: u64,
    pub resolution: u16,
    pub texture_size: u16,
    pub target_faces: u32,
    pub elapsed_ms: u128,
}

impl Pixal3dMlxRuntime {
    /// Reads only non-secret path configuration. No tokens are inspected.
    pub fn discover() -> Option<Self> {
        let root = std::env::var_os("MESHHARBOR_PIXAL3D_MLX_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../runtimes/pixal3d/mlx/upstream")
            });
        let python = std::env::var_os("MESHHARBOR_PIXAL3D_MLX_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    root.join(".venv/Scripts/python.exe")
                } else {
                    root.join(".venv/bin/python")
                }
            });
        Some(Self { root, python })
    }

    pub fn readiness(&self) -> RuntimeReadiness {
        let mut missing = Vec::new();
        if !self.python.is_file() {
            missing.push(format!(
                "Python environment is missing: {}",
                self.python.display()
            ));
        }
        if !self.root.join("generate_pixal3d.py").is_file() || !self.root.join("pixalmlx").is_dir()
        {
            missing.push(format!(
                "Pixal3D MLX source is missing under {} (set MESHHARBOR_PIXAL3D_MLX_ROOT).",
                self.root.display()
            ));
        }

        let home = user_home();
        if let Some(home) = home {
            // The pinned MLX entry point hard-codes expanduser("~/.cache/huggingface/hub"),
            // so readiness must check this exact location rather than HF_HOME overrides.
            let hub = home.join(".cache/huggingface/hub");
            let pixal_snapshot = hub
                .join(PIXAL_REPOSITORY)
                .join("snapshots")
                .join(PIXAL_REVISION);
            for filename in PIXAL_CHECKPOINTS {
                if !pixal_snapshot.join("ckpts").join(filename).is_file() {
                    missing.push(format!("Missing Pixal3D checkpoint: {filename}"));
                }
            }

            let dino_snapshot = hub
                .join(DINO_REPOSITORY)
                .join("snapshots")
                .join(DINO_REVISION);
            let dino_snapshot_root = hub.join(DINO_REPOSITORY).join("snapshots");
            if !has_only_pinned_snapshot(&dino_snapshot_root, DINO_REVISION)
                || !has_safetensors(&dino_snapshot)
            {
                missing.push(format!(
                    "Missing the sole pinned gated DINOv3 checkpoint at {} (the user must request access, authenticate, and keep only this revision in the selected HOME cache because the pinned entrypoint chooses an arbitrary snapshot).",
                    dino_snapshot.display(),
                ));
            }
        } else {
            missing.push("HOME is unset; the upstream MLX runtime expects Hugging Face files under ~/.cache/huggingface/hub.".into());
        }

        if !self.root.join("weights/naf_release.safetensors").is_file() {
            missing.push(
                "Missing converted NAF checkpoint at weights/naf_release.safetensors.".into(),
            );
        }

        RuntimeReadiness {
            backend_id: "pixal3d-mlx".into(),
            ready: missing.is_empty(),
            validation_level: "required_paths_only_environment_unverified".into(),
            runtime_root: Some(self.root.clone()),
            python: Some(self.python.clone()),
            missing,
            downloads_enabled: false,
            gated_model_access_required: true,
        }
    }

    fn generate(
        &self,
        input: &Path,
        output: &Path,
        options: &GenerationOptions,
    ) -> CoreResult<GenerationCommandReport> {
        let readiness = self.readiness();
        if !readiness.ready {
            return Err(CoreError::OperationUnavailable(format!(
                "Pixal3D MLX runtime is not ready: {}. MeshHarbor will not install dependencies, accept gated terms, or download weights automatically.",
                readiness.missing.join("; ")
            )));
        }
        if options.resolution != 512 {
            return Err(CoreError::InvalidInput(
                "The current MLX integration exposes only the reviewed 512 generation profile."
                    .into(),
            ));
        }
        if !matches!(options.texture_size, 512 | 1024 | 2048) {
            return Err(CoreError::InvalidInput(
                "texture_size must be 512, 1024, or 2048.".into(),
            ));
        }
        if options.target_faces < 4 {
            return Err(CoreError::InvalidInput(
                "target_faces must be at least 4.".into(),
            ));
        }

        let started = Instant::now();
        let mut command = Command::new(&self.python);
        command
            .current_dir(&self.root)
            // Do not leak account tokens or arbitrary Python package paths to
            // the model worker. The cached checkpoints suffice in offline mode.
            .env_clear()
            .env("HOME", user_home().unwrap_or_else(|| self.root.clone()))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("PYTHONPATH", &self.root)
            .env("VIRTUAL_ENV", self.root.join(".venv"))
            .env("PYTHONNOUSERSITE", "1")
            // The vendored CLI loads pinned local checkpoints directly. Force
            // offline hub behavior so an inference request cannot surprise-
            // download files or send credentials to a model host.
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1");
        command.env("HF_HUB_DISABLE_TELEMETRY", "1");
        for temporary_key in ["TMPDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(temporary_key) {
                command.env(temporary_key, value);
            }
        }
        let status = command
            .arg(self.root.join("generate_pixal3d.py"))
            .arg("--image")
            .arg(input)
            .arg("--output")
            .arg(output)
            .arg("--seed")
            .arg(options.seed.to_string())
            .arg("--resolution")
            .arg(options.resolution.to_string())
            .arg("--texture-size")
            .arg(options.texture_size.to_string())
            .arg("--target-faces")
            .arg(options.target_faces.to_string())
            // Avoid model auto-download/telemetry: the user's prepared input
            // is passed through without an unpinned background-removal model;
            // camera uses the upstream fixed-FOV fallback instead of MoGe.
            .arg("--no-rembg")
            .arg("--no-moge")
            .arg("--hole-fill-backend")
            .arg("fan")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|error| {
                CoreError::OperationUnavailable(format!(
                    "Could not start the configured Pixal3D MLX Python runtime: {error}"
                ))
            })?;

        if !status.success() {
            let message = format!(
                "Pixal3D MLX exited with status {}. See the local runtime stderr for details.",
                status
                    .code()
                    .map_or_else(|| "signal".into(), |code| code.to_string())
            );
            return Err(CoreError::OperationUnavailable(message));
        }
        if !output.is_file() {
            return Err(CoreError::InvalidInput(
                "Pixal3D MLX exited successfully but did not produce its requested GLB output."
                    .into(),
            ));
        }
        let output_size = std::fs::metadata(output)?.len();
        if output_size == 0 || output_size > MAX_GENERATED_GLB_BYTES {
            return Err(CoreError::InvalidInput(format!(
                "Generated GLB size ({output_size} bytes) is outside the supported 1..={MAX_GENERATED_GLB_BYTES} byte range."
            )));
        }
        Ok(GenerationCommandReport {
            elapsed_ms: started.elapsed().as_millis(),
        })
    }
}

fn has_safetensors(directory: &Path) -> bool {
    std::fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("safetensors")
        })
}

fn has_only_pinned_snapshot(directory: &Path, revision: &str) -> bool {
    let snapshots = std::fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| !name.starts_with('.'))
        })
        .map(|entry| entry.file_name());
    let mut snapshots = snapshots;
    matches!(snapshots.next().and_then(|name| name.into_string().ok()).as_deref(), Some(name) if name == revision)
        && snapshots.next().is_none()
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

struct GenerationCommandReport {
    elapsed_ms: u128,
}

/// Generates a textured GLB from a previously imported local image using
/// an explicitly prepared Pixal3D MLX runtime.
pub(crate) fn generate_pixal3d_image(
    core: &OperationCore,
    input_asset_id: &str,
    options: GenerationOptions,
) -> CoreResult<GenerationResult> {
    let source = core
        .store()
        .get_asset(input_asset_id)?
        .ok_or_else(|| CoreError::NotFound(format!("Asset '{input_asset_id}' was not found.")))?;
    if source.kind != AssetKind::Image {
        return Err(CoreError::InvalidInput(
            "Image-to-3D generation requires an imported PNG or JPEG asset.".into(),
        ));
    }
    let source_artifact = source
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == "source")
        .ok_or_else(|| CoreError::NotFound("Image source artifact is missing.".into()))?;
    let input_bytes = core.store().read_artifact(&source_artifact.id)?.bytes;

    let runtime = Pixal3dMlxRuntime::discover().ok_or_else(|| {
        CoreError::OperationUnavailable("No Pixal3D MLX runtime profile is configured.".into())
    })?;
    if !runtime.readiness().ready {
        return Err(CoreError::OperationUnavailable(format!(
            "Pixal3D MLX runtime is not ready: {}",
            runtime.readiness().missing.join("; ")
        )));
    }

    let work_dir = core
        .store()
        .staging_root()
        .join(format!("generate-{}", Uuid::new_v4()));
    std::fs::create_dir(&work_dir)?;
    let cleanup = WorkDir(work_dir.clone());
    let input_filename = match source.mime_type.as_str() {
        "image/png" => "input.png",
        "image/jpeg" => "input.jpg",
        _ => {
            return Err(CoreError::InvalidInput(
                "Pixal3D accepts PNG or JPEG source images.".into(),
            ));
        }
    };
    let input_path = work_dir.join(input_filename);
    let output_path = work_dir.join("result.glb");
    std::fs::write(&input_path, input_bytes)?;

    let report = runtime.generate(&input_path, &output_path, &options)?;
    let output = std::fs::read(&output_path)?;
    let descriptor = inspect_asset("result.glb", &output)?;
    if descriptor.kind != AssetKind::Mesh {
        return Err(CoreError::InvalidInput(
            "Pixal3D output did not validate as a GLB mesh.".into(),
        ));
    }
    let output_name = source
        .name
        .rsplit_once('.')
        .map(|(stem, _)| format!("{stem}-generated.glb"))
        .unwrap_or_else(|| "generated.glb".into());
    let asset = core.store().import_derived_asset(
        &output_name,
        &output,
        input_asset_id,
        json!({
            "operation": "image.generate",
            "backend": "pixal3d-mlx",
            "seed": options.seed,
            "resolution": options.resolution,
            "texture_size": options.texture_size,
            "target_faces": options.target_faces,
            "elapsed_ms": report.elapsed_ms,
            "validation": descriptor.metadata,
        }),
    )?;
    drop(cleanup);
    Ok(GenerationResult {
        asset,
        backend_id: "pixal3d-mlx".into(),
        seed: options.seed,
        resolution: options.resolution,
        texture_size: options.texture_size,
        target_faces: options.target_faces,
        elapsed_ms: report.elapsed_ms,
    })
}

struct WorkDir(PathBuf);

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_constrain_to_reviewed_mlx_profile() {
        let root = std::env::temp_dir().join(format!("meshharbor-mlx-profile-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("pixalmlx")).unwrap();
        std::fs::write(root.join("generate_pixal3d.py"), "# test").unwrap();
        let runtime = Pixal3dMlxRuntime {
            python: root.join(".venv/bin/python"),
            root: root.clone(),
        };
        let status = runtime.readiness();
        assert_eq!(status.backend_id, "pixal3d-mlx");
        assert!(!status.ready);
        assert!(!status.downloads_enabled);
        assert!(status.gated_model_access_required);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn safetensors_probe_ignores_unrelated_files() {
        let root = std::env::temp_dir().join(format!("meshharbor-dino-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.json"), "{}").unwrap();
        assert!(!has_safetensors(&root));
        std::fs::write(root.join("model.safetensors"), b"test").unwrap();
        assert!(has_safetensors(&root));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dino_probe_requires_only_the_pinned_snapshot() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-dino-revisions-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join(DINO_REVISION)).unwrap();
        assert!(has_only_pinned_snapshot(&root, DINO_REVISION));
        std::fs::create_dir_all(root.join("another-revision")).unwrap();
        assert!(!has_only_pinned_snapshot(&root, DINO_REVISION));
        std::fs::remove_dir_all(root).unwrap();
    }
}
