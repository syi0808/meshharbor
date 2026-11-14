//! Optional Pixal3D MLX process integration and local readiness checks.
//!
//! Runtime setup and model downloads are explicit user-triggered actions in
//! `runtime.rs`; inference itself remains offline and never authenticates.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{Asset, AssetKind, CoreError, CoreResult, OperationCore, inspect_asset};
use crate::{cuda_runtime::Pixal3dCudaRuntimeManager, runtime::Pixal3dRuntimeManager};

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
    /// Backend selector: `auto`, `pixal3d-mlx`, or `pixal3d-cuda`.
    #[serde(default = "default_generation_backend")]
    pub backend_id: String,
    /// Reproducible sampler seed.
    pub seed: u64,
    /// Both vendored backends currently expose only the 512 geometry cascade.
    pub resolution: u16,
    /// Output PBR texture resolution.
    pub texture_size: u16,
    /// Soft triangle target. The model may return more faces.
    pub target_faces: u32,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            backend_id: default_generation_backend(),
            seed: 42,
            resolution: 512,
            texture_size: 1024,
            target_faces: 100_000,
        }
    }
}

fn default_generation_backend() -> String {
    "auto".into()
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
    pub environment_ready: bool,
    pub model_files_ready: bool,
    pub models_downloaded: bool,
    pub naf_conversion_required: bool,
    pub conversion_enabled: bool,
    pub downloads_enabled: bool,
    pub gated_model_access_required: bool,
}

/// Isolated MLX runtime profile managed under MeshHarbor's data directory.
#[derive(Debug, Clone)]
pub struct Pixal3dMlxRuntime {
    pub root: PathBuf,
    pub python: PathBuf,
    pub home: PathBuf,
    pub weights: PathBuf,
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
    pub fn discover(data_root: impl AsRef<Path>) -> Self {
        let runtime_manager = Pixal3dRuntimeManager::new(data_root);
        let root = runtime_manager.source_root().to_path_buf();
        let runtime_home = runtime_manager.runtime_root().to_path_buf();
        let python = std::env::var_os("MESHHARBOR_PIXAL3D_MLX_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    runtime_home.join(".venv/Scripts/python.exe")
                } else {
                    runtime_home.join(".venv/bin/python")
                }
            });
        let home = runtime_home.join("home");
        let weights = std::env::var_os("MESHHARBOR_PIXAL3D_WEIGHTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| runtime_home.join("weights"));
        Self {
            root,
            python,
            home,
            weights,
        }
    }

    pub fn readiness(&self) -> RuntimeReadiness {
        let mut missing = Vec::new();
        let environment_ready = self.python.is_file();
        if !environment_ready {
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

        let mut model_files_ready = self.home.is_dir();
        let mut models_downloaded = self.home.is_dir();
        if self.home.is_dir() {
            // The pinned MLX entry point hard-codes expanduser("~/.cache/huggingface/hub"),
            // so provision an isolated HOME rather than mutating the user's existing HF cache.
            let hub = self.home.join(".cache/huggingface/hub");
            let pixal_snapshot = hub
                .join(PIXAL_REPOSITORY)
                .join("snapshots")
                .join(PIXAL_REVISION);
            for filename in PIXAL_CHECKPOINTS {
                if !pixal_snapshot.join("ckpts").join(filename).is_file() {
                    model_files_ready = false;
                    models_downloaded = false;
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
                model_files_ready = false;
                models_downloaded = false;
                missing.push(format!(
                    "Missing the sole pinned gated DINOv3 checkpoint at {} (the user must request access, authenticate, and keep only this revision in the selected HOME cache because the pinned entrypoint chooses an arbitrary snapshot).",
                    dino_snapshot.display(),
                ));
            }
        } else {
            model_files_ready = false;
            models_downloaded = false;
            missing.push(format!(
                "The isolated runtime home is missing: {}. Run `meshharbor runtime setup` first.",
                self.home.display()
            ));
        }

        let naf_source = self.weights.join("naf_release.pth");
        let naf_target = self.weights.join("naf_release.safetensors");
        let naf_conversion_required = naf_source.is_file() && !naf_target.is_file();
        if !naf_target.is_file() {
            model_files_ready = false;
            if !naf_source.is_file() {
                models_downloaded = false;
            }
            missing.push(
                format!("Missing converted NAF checkpoint at {}. Run the documented NAF conversion step after model download.", naf_target.display()),
            );
        }

        RuntimeReadiness {
            backend_id: "pixal3d-mlx".into(),
            ready: environment_ready && model_files_ready && missing.is_empty(),
            validation_level: "required_paths_only_environment_unverified".into(),
            runtime_root: Some(
                self.home
                    .parent()
                    .unwrap_or(self.home.as_path())
                    .to_path_buf(),
            ),
            python: Some(self.python.clone()),
            missing,
            environment_ready,
            model_files_ready,
            models_downloaded,
            naf_conversion_required,
            conversion_enabled: models_downloaded
                && naf_conversion_required
                && cfg!(all(target_os = "macos", target_arch = "aarch64")),
            downloads_enabled: environment_ready
                && !models_downloaded
                && cfg!(all(target_os = "macos", target_arch = "aarch64")),
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

        let entrypoint = self.prepare_entrypoint()?;
        let python_bin = self.python.parent().ok_or_else(|| {
            CoreError::OperationUnavailable(
                "Configured Python path has no parent directory.".into(),
            )
        })?;
        let virtual_env = python_bin.parent().unwrap_or(python_bin);
        let mut path_entries = vec![python_bin.to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            path_entries.extend(std::env::split_paths(&path));
        }
        let path = std::env::join_paths(path_entries).unwrap_or_default();
        let started = Instant::now();
        let mut command = Command::new(&self.python);
        command
            .current_dir(&self.root)
            // Do not leak account tokens or arbitrary Python package paths to
            // the model worker. The cached checkpoints suffice in offline mode.
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("PYTHONPATH", &self.root)
            .env("MESHHARBOR_PIXAL3D_WEIGHTS_DIR", &self.weights)
            .env("VIRTUAL_ENV", virtual_env)
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
            .arg(entrypoint.path())
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

    fn prepare_entrypoint(&self) -> CoreResult<PreparedEntrypoint> {
        const UPSTREAM_NAF_PATH: &str = "    naf_path = os.path.join(os.path.dirname(__file__), \"weights\", \"naf_release.safetensors\")";
        const MANAGED_NAF_PATH: &str = concat!(
            "    naf_weights_dir = os.environ.get(\"MESHHARBOR_PIXAL3D_WEIGHTS_DIR\", ",
            "os.path.join(os.path.dirname(__file__), \"weights\"))\n",
            "    naf_path = os.path.join(naf_weights_dir, \"naf_release.safetensors\")"
        );
        let upstream = self.root.join("generate_pixal3d.py");
        let source = std::fs::read_to_string(&upstream)?;
        if source.matches(UPSTREAM_NAF_PATH).count() != 1 {
            return Err(CoreError::OperationUnavailable(
                "Pinned Pixal3D entrypoint does not match the reviewed NAF path contract; refusing to patch it at runtime.".into(),
            ));
        }
        let managed = source.replace(UPSTREAM_NAF_PATH, MANAGED_NAF_PATH);
        let runtime_root = self.home.parent().unwrap_or(self.home.as_path());
        let directory = runtime_root.join("entrypoint");
        std::fs::create_dir_all(&directory)?;

        // Each inference owns its own immutable script. Generation uses a shared
        // runtime lock, so a fixed temporary or final path lets concurrent
        // requests overwrite one another or race while renaming.
        let target = directory.join(format!("generate_pixal3d-{}.py", Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        if let Err(error) = file.write_all(managed.as_bytes()) {
            drop(file);
            let _ = std::fs::remove_file(&target);
            return Err(error.into());
        }
        drop(file);
        Ok(PreparedEntrypoint(target))
    }
}

struct PreparedEntrypoint(PathBuf);

impl PreparedEntrypoint {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PreparedEntrypoint {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
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

struct GenerationCommandReport {
    elapsed_ms: u128,
}

/// Generates a textured GLB from a previously imported local image using the
/// explicitly selected, already prepared Pixal3D backend.
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

    let selected_backend_id = match options.backend_id.as_str() {
        "auto" => {
            if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
                "pixal3d-cuda"
            } else {
                "pixal3d-mlx"
            }
        }
        "pixal3d-mlx" | "pixal3d-cuda" => options.backend_id.as_str(),
        other => {
            return Err(CoreError::InvalidInput(format!(
                "Unknown generation backend '{other}'. Choose auto, pixal3d-mlx, or pixal3d-cuda."
            )));
        }
    };

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
    let input_path = work_dir.join(format!("source-{input_filename}"));
    let output_path = work_dir.join("result.glb");
    std::fs::write(&input_path, input_bytes)?;

    let elapsed_ms = match selected_backend_id {
        "pixal3d-mlx" => {
            let runtime = Pixal3dMlxRuntime::discover(core.store().root());
            if !runtime.readiness().ready {
                return Err(CoreError::OperationUnavailable(format!(
                    "Pixal3D MLX runtime is not ready: {}",
                    runtime.readiness().missing.join("; ")
                )));
            }
            runtime
                .generate(&input_path, &output_path, &options)?
                .elapsed_ms
        }
        "pixal3d-cuda" => {
            let runtime = Pixal3dCudaRuntimeManager::new(core.store().root());
            runtime.generate(&input_path, &work_dir, &options)?
        }
        _ => unreachable!("backend selector was validated above"),
    };
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
            "backend": selected_backend_id,
            "seed": options.seed,
            "resolution": options.resolution,
            "texture_size": options.texture_size,
            "target_faces": options.target_faces,
            "elapsed_ms": elapsed_ms,
            "validation": descriptor.metadata,
        }),
    )?;
    drop(cleanup);
    Ok(GenerationResult {
        asset,
        backend_id: selected_backend_id.into(),
        seed: options.seed,
        resolution: options.resolution,
        texture_size: options.texture_size,
        target_faces: options.target_faces,
        elapsed_ms,
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

    fn expected_managed_source(source: &str) -> String {
        const UPSTREAM_NAF_PATH: &str = "    naf_path = os.path.join(os.path.dirname(__file__), \"weights\", \"naf_release.safetensors\")";
        const MANAGED_NAF_PATH: &str = concat!(
            "    naf_weights_dir = os.environ.get(\"MESHHARBOR_PIXAL3D_WEIGHTS_DIR\", ",
            "os.path.join(os.path.dirname(__file__), \"weights\"))\n",
            "    naf_path = os.path.join(naf_weights_dir, \"naf_release.safetensors\")"
        );
        source.replace(UPSTREAM_NAF_PATH, MANAGED_NAF_PATH)
    }

    #[test]
    fn options_constrain_to_reviewed_mlx_profile() {
        let root = std::env::temp_dir().join(format!("meshharbor-mlx-profile-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("pixalmlx")).unwrap();
        std::fs::write(root.join("generate_pixal3d.py"), "# test").unwrap();
        let runtime = Pixal3dMlxRuntime {
            python: root.join(".venv/bin/python"),
            root: root.clone(),
            home: root.join("home"),
            weights: root.join("weights"),
        };
        let status = runtime.readiness();
        assert_eq!(status.backend_id, "pixal3d-mlx");
        assert!(!status.ready);
        assert!(!status.downloads_enabled);
        assert!(status.gated_model_access_required);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_entrypoint_uses_app_data_weights_without_mutating_pinned_source() {
        let root = std::env::temp_dir().join(format!("meshharbor-mlx-overlay-{}", Uuid::new_v4()));
        let source_root = root.join("source");
        let runtime_root = root.join("runtime");
        std::fs::create_dir_all(source_root.join("pixalmlx")).unwrap();
        let upstream = source_root.join("generate_pixal3d.py");
        let source = "    naf_path = os.path.join(os.path.dirname(__file__), \"weights\", \"naf_release.safetensors\")";
        std::fs::write(&upstream, source).unwrap();
        let runtime = Pixal3dMlxRuntime {
            python: runtime_root.join(".venv/bin/python"),
            root: source_root,
            home: runtime_root.join("home"),
            weights: runtime_root.join("weights"),
        };

        let entrypoint = runtime.prepare_entrypoint().unwrap();
        let entrypoint_path = entrypoint.path().to_path_buf();
        let patched = std::fs::read_to_string(entrypoint.path()).unwrap();
        assert_eq!(patched, expected_managed_source(source));
        assert_eq!(std::fs::read_to_string(upstream).unwrap(), source);
        drop(entrypoint);
        assert!(!entrypoint_path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_entrypoints_are_isolated_per_generation_source() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-mlx-entrypoints-{}", Uuid::new_v4()));
        let runtime_root = root.join("runtime");
        let source_one = root.join("source-one");
        let source_two = root.join("source-two");
        std::fs::create_dir_all(&runtime_root).unwrap();
        std::fs::create_dir_all(&source_one).unwrap();
        std::fs::create_dir_all(&source_two).unwrap();

        let naf_path = "    naf_path = os.path.join(os.path.dirname(__file__), \"weights\", \"naf_release.safetensors\")";
        let upstream_one = source_one.join("generate_pixal3d.py");
        let upstream_two = source_two.join("generate_pixal3d.py");
        let source_one_text = format!("# source-one marker\n{naf_path}\n");
        let source_two_text = format!("# source-two marker\n{naf_path}\n");
        std::fs::write(&upstream_one, &source_one_text).unwrap();
        std::fs::write(&upstream_two, &source_two_text).unwrap();

        let runtime_one = Pixal3dMlxRuntime {
            python: runtime_root.join(".venv/bin/python"),
            root: source_one,
            home: runtime_root.join("home"),
            weights: runtime_root.join("weights"),
        };
        let runtime_two = Pixal3dMlxRuntime {
            root: source_two,
            ..runtime_one.clone()
        };

        let entrypoint_one = runtime_one.prepare_entrypoint().unwrap();
        let path_one = entrypoint_one.path().to_path_buf();
        let entrypoint_two = runtime_two.prepare_entrypoint().unwrap();
        let path_two = entrypoint_two.path().to_path_buf();

        assert_ne!(path_one, path_two);
        let prepared_one = std::fs::read_to_string(entrypoint_one.path()).unwrap();
        let prepared_two = std::fs::read_to_string(entrypoint_two.path()).unwrap();
        assert_eq!(prepared_one, expected_managed_source(&source_one_text));
        assert_eq!(prepared_two, expected_managed_source(&source_two_text));
        assert_eq!(
            std::fs::read_to_string(&upstream_one).unwrap(),
            source_one_text
        );
        assert_eq!(
            std::fs::read_to_string(&upstream_two).unwrap(),
            source_two_text
        );

        drop(entrypoint_one);
        assert!(!path_one.exists());
        assert!(path_two.exists());
        drop(entrypoint_two);
        assert!(!path_two.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_entrypoint_preparation_uses_independent_complete_files() {
        use std::collections::HashSet;
        use std::sync::{Arc, Barrier};

        const CONCURRENCY: usize = 8;
        let root =
            std::env::temp_dir().join(format!("meshharbor-mlx-entrypoint-race-{}", Uuid::new_v4()));
        let runtime_root = root.join("runtime");
        let source_root = root.join("source");
        std::fs::create_dir_all(&runtime_root).unwrap();
        std::fs::create_dir_all(&source_root).unwrap();
        let source = "# concurrent marker\n    naf_path = os.path.join(os.path.dirname(__file__), \"weights\", \"naf_release.safetensors\")\n";
        std::fs::write(source_root.join("generate_pixal3d.py"), source).unwrap();
        let runtime = Pixal3dMlxRuntime {
            python: runtime_root.join(".venv/bin/python"),
            root: source_root,
            home: runtime_root.join("home"),
            weights: runtime_root.join("weights"),
        };
        let barrier = Arc::new(Barrier::new(CONCURRENCY));
        let workers: Vec<_> = (0..CONCURRENCY)
            .map(|_| {
                let runtime = runtime.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    runtime.prepare_entrypoint().unwrap()
                })
            })
            .collect();
        let entries: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        let expected = expected_managed_source(source);
        let paths: HashSet<PathBuf> = entries
            .iter()
            .map(|entry| entry.path().to_path_buf())
            .collect();
        assert_eq!(paths.len(), CONCURRENCY);
        for entry in &entries {
            let prepared = std::fs::read_to_string(entry.path()).unwrap();
            assert_eq!(prepared, expected);
        }
        let paths: Vec<_> = entries
            .iter()
            .map(|entry| entry.path().to_path_buf())
            .collect();
        drop(entries);
        assert!(paths.iter().all(|path| !path.exists()));
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
