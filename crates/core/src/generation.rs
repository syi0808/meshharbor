//! Local model generation and Pixal3D MLX process integration.
//!
//! Runtime setup and model downloads are explicit user-triggered actions in
//! `runtime.rs`; inference itself remains offline and never authenticates.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    Asset, AssetKind, CoreError, CoreResult, MAX_IMAGE_BYTES, OperationCore,
    external_generation::{ExternalInput, ExternalProfile},
    generation_tuning,
    generation_views::PosedGenerationInput,
    huggingface_download::{PIXAL_CHECKPOINTS, PIXAL_REPOSITORY, PIXAL_REVISION},
    inspect_asset,
    job_execution::JobExecution,
    lato2::{self, DEFAULT_TARGET_VERTICES, Lato2Runtime},
};
use crate::{cuda_runtime::Pixal3dCudaRuntimeManager, runtime::Pixal3dRuntimeManager};

const DINO_REPOSITORY: &str = "models--facebook--dinov3-vitl16-pretrain-lvd1689m";
const DINO_REVISION: &str = "ea8dc2863c51be0a264bab82070e3e8836b02d51";
const MAX_GENERATED_GLB_BYTES: u64 = 512 * 1024 * 1024;
const MAX_GENERATION_PROMPT_BYTES: usize = 4096;

pub const MODEL_GENERATION_OPERATION: &str = "model.generate";
pub const IMAGE_GENERATION_OPERATION: &str = "image.generate";

pub fn is_generation_operation(operation: &str) -> bool {
    matches!(
        operation,
        MODEL_GENERATION_OPERATION | IMAGE_GENERATION_OPERATION
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationInputMode {
    Image,
    Text,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GenerationInput {
    Image { asset_id: String },
    Text { prompt: String },
}

impl GenerationInput {
    pub fn asset_id(&self) -> Option<&str> {
        match self {
            Self::Image { asset_id } => Some(asset_id),
            Self::Text { .. } => None,
        }
    }

    pub fn mode(&self) -> GenerationInputMode {
        match self {
            Self::Image { .. } => GenerationInputMode::Image,
            Self::Text { .. } => GenerationInputMode::Text,
        }
    }

    pub(crate) fn validate(&self) -> CoreResult<()> {
        if let Self::Text { prompt } = self
            && (prompt.trim().is_empty()
                || prompt.contains('\0')
                || prompt.len() > MAX_GENERATION_PROMPT_BYTES)
        {
            return Err(CoreError::InvalidInput(format!(
                "A text prompt must be nonempty, contain no NUL, and use at most {MAX_GENERATION_PROMPT_BYTES} UTF-8 bytes."
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct GenerationOptions {
    /// Backend selector from the runtime catalog, or `auto` for platform Pixal3D.
    pub backend_id: String,
    /// Reproducible sampler seed.
    pub seed: u64,
    /// The initial profiles use a fixed geometry resolution of 512.
    pub resolution: u16,
    /// Output PBR texture resolution.
    pub texture_size: u16,
    /// Soft triangle target. The model may return more faces.
    pub target_faces: u32,
    /// Approximate vertex target for profiles that generate a new topology.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_vertices: Option<u32>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tuning: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_input: Option<PosedGenerationInput>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture_size: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_faces: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_vertices: Option<u32>,
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
        execution: JobExecution<'_>,
    ) -> CoreResult<GenerationCommandReport> {
        let readiness = self.readiness();
        if !readiness.ready {
            return Err(CoreError::OperationUnavailable(format!(
                "Pixal3D MLX runtime is not ready: {}. MeshHarbor will not install dependencies, accept gated terms, or download weights automatically.",
                readiness.missing.join("; ")
            )));
        }
        options.validate_for_backend("pixal3d-mlx")?;

        let entrypoint = self.prepare_entrypoint(generation_tuning::has_sampler_overrides(
            options,
            "pixal3d-mlx",
        ))?;
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
        command
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
            .arg("fan");
        generation_tuning::add_arguments(&mut command, options, "pixal3d-mlx");
        let status = run_generation_command(&mut command, execution)?;

        if !status.success() {
            let message = format!("Pixal3D MLX failed ({status}). {}", execution.output_hint());
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

    fn prepare_entrypoint(&self, tune_samplers: bool) -> CoreResult<PreparedEntrypoint> {
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
        let managed = patch_mlx_postprocess(&source.replace(UPSTREAM_NAF_PATH, MANAGED_NAF_PATH))?;
        let managed = if tune_samplers {
            patch_mlx_samplers(&managed)?
        } else {
            managed
        };
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

fn patch_mlx_postprocess(source: &str) -> CoreResult<String> {
    const UPSTREAM_IMPORT: &str = "\n    from generate import _cleanup_and_simplify_mesh\n";
    if source.matches(UPSTREAM_IMPORT).count() != 1 {
        return Err(CoreError::OperationUnavailable(
            "Pinned Pixal3D entrypoint does not match the reviewed mesh postprocessing contract."
                .into(),
        ));
    }
    let managed = include_str!("pixal3d_mlx_postprocess.py").trim_end();
    let inline = format!("\n    {}\n", managed.replace('\n', "\n    "));
    Ok(source.replace(UPSTREAM_IMPORT, &inline))
}

fn patch_mlx_samplers(source: &str) -> CoreResult<String> {
    let mismatch = || {
        CoreError::OperationUnavailable(
            "Pinned Pixal3D entrypoint does not match the reviewed sampler configuration.".into(),
        )
    };
    let main = "def main(argv=None, runtime=None):";
    if source.matches(main).count() != 1 {
        return Err(mismatch());
    }
    let mut patched = source.replace(
        main,
        &format!("{}\n\n{main}", include_str!("sampler_overrides.py")),
    );
    for (variable, stage) in [
        ("SS_SAMPLER", "sparse_structure_sampler"),
        ("SHAPE_SAMPLER", "shape_slat_sampler"),
        ("TEX_SAMPLER", "tex_slat_sampler"),
    ] {
        let marker = format!("    {variable} = dict(");
        if patched.matches(&marker).count() != 1 {
            return Err(mismatch());
        }
        let start = patched.find(&marker).ok_or_else(mismatch)?;
        let end = start + patched[start..].find(")\n\n").ok_or_else(mismatch)? + 2;
        if patched[start..end].lines().count() != 2 {
            return Err(mismatch());
        }
        patched.insert_str(
            end,
            &format!("    {variable} = _meshharbor_tune_sampler(\"{stage}\", {variable})\n"),
        );
    }
    Ok(patched)
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

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_stored_source_image(
    filename: &str,
    recorded_mime_type: &str,
    recorded_asset_sha256: &str,
    recorded_artifact_sha256: &str,
    bytes: &[u8],
) -> CoreResult<()> {
    let actual_sha256 = sha256_hex(bytes);
    if actual_sha256 != recorded_asset_sha256 || actual_sha256 != recorded_artifact_sha256 {
        return Err(CoreError::Conflict(
            "Stored image content no longer matches its recorded SHA-256; re-import it.".into(),
        ));
    }

    let descriptor = inspect_asset(filename, bytes)?;
    if descriptor.kind != AssetKind::Image || descriptor.mime_type != recorded_mime_type {
        return Err(CoreError::InvalidInput(
            "Stored image no longer matches its recorded PNG/JPEG format; re-import it.".into(),
        ));
    }
    Ok(())
}

fn read_bounded_source_image(path: &Path) -> CoreResult<Vec<u8>> {
    let read_limit = u64::try_from(MAX_IMAGE_BYTES)
        .ok()
        .and_then(|limit| limit.checked_add(1))
        .ok_or_else(|| CoreError::InvalidInput("Image size limit is too large.".into()))?;
    let mut file = std::fs::File::open(path)?.take(read_limit);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "Stored image exceeds the {} MiB generation limit; re-import a smaller PNG or JPEG.",
            MAX_IMAGE_BYTES / (1024 * 1024)
        )));
    }
    Ok(bytes)
}

fn stage_reference(
    core: &OperationCore,
    source: &Asset,
    work_dir: &Path,
    index: usize,
) -> CoreResult<PathBuf> {
    let extension = match (source.kind, source.mime_type.as_str()) {
        (AssetKind::Image, "image/png") => "png",
        (AssetKind::Image, "image/jpeg") => "jpg",
        _ => {
            return Err(CoreError::InvalidInput(
                "Generation references must be imported PNG or JPEG images.".into(),
            ));
        }
    };
    let artifact = source
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == "source")
        .ok_or_else(|| CoreError::NotFound("Image source artifact is missing.".into()))?;
    let bytes = read_bounded_source_image(&core.store().artifact_path(&artifact.id)?)?;
    validate_stored_source_image(
        &source.name,
        &source.mime_type,
        &source.sha256,
        &artifact.sha256,
        &bytes,
    )?;
    let input_path = work_dir.join(format!("source-{index:02}.{extension}"));
    std::fs::write(&input_path, bytes)?;
    Ok(input_path)
}

pub(crate) fn run_generation_command(
    command: &mut Command,
    execution: JobExecution<'_>,
) -> CoreResult<ExitStatus> {
    run_generation_command_with_input(command, execution, None)
}

pub(crate) fn run_generation_command_with_input(
    command: &mut Command,
    execution: JobExecution<'_>,
    input: Option<&[u8]>,
) -> CoreResult<ExitStatus> {
    let tracked = execution.is_tracked();
    if !tracked && input.is_none() {
        return Ok(command
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()?);
    }
    execution.checkpoint()?;
    #[cfg(unix)]
    if tracked {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else if tracked {
        command.stdin(Stdio::null());
    }
    let trace = execution.trace_command(command)?;
    let result = (|| {
        let mut child = command
            .stdout(if tracked {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stderr(if tracked {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .spawn()?;
        if let Some(bytes) = input {
            let mut stdin = child
                .stdin
                .take()
                .expect("generation worker stdin is piped");
            let written = stdin.write_all(bytes);
            drop(stdin);
            if let Err(error) = written {
                if tracked {
                    stop_generation_worker(&mut child)?;
                } else {
                    let _ = child.kill();
                    child.wait()?;
                }
                return Err(error.into());
            }
        }
        if !tracked {
            return Ok(child.wait()?);
        }
        let trace = trace.expect("tracked generation command");
        let stdout = child
            .stdout
            .take()
            .expect("generation worker stdout is piped");
        let stderr = child
            .stderr
            .take()
            .expect("generation worker stderr is piped");
        thread::scope(|scope| {
            let stderr = scope.spawn(|| trace.drain(stderr, true, |_| {}));
            let stdout = scope.spawn(|| {
                let mut line = Vec::new();
                let mut oversized = false;
                let mut stage_result = Ok(());
                trace.drain(stdout, false, |bytes| {
                    for chunk in bytes.split_inclusive(|byte| *byte == b'\n') {
                        if !oversized && line.len() + chunk.len() <= 512 {
                            line.extend_from_slice(chunk);
                        } else {
                            oversized = true;
                        }
                        if chunk.ends_with(b"\n") {
                            if !oversized
                                && stage_result.is_ok()
                                && let Ok(text) = std::str::from_utf8(&line)
                                && let Some(stage) = generation_worker_stage(text.trim())
                            {
                                stage_result = execution.stage(stage);
                            }
                            line.clear();
                            oversized = false;
                        }
                    }
                })?;
                stage_result
            });
            let status = loop {
                if let Err(error) = execution.checkpoint() {
                    stop_generation_worker(&mut child)?;
                    break Err(error);
                }
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Ok(None) => thread::sleep(Duration::from_millis(100)),
                    Err(error) => {
                        stop_generation_worker(&mut child)?;
                        break Err(error.into());
                    }
                }
            };
            let stdout_result = stdout.join().expect("generation stdout reader");
            let stderr_result = stderr.join().expect("generation stderr reader");
            let status = status?;
            stdout_result?;
            stderr_result?;
            Ok(status)
        })
    })();
    if let Some(trace) = trace {
        trace.finish(&result.as_ref().copied());
    }
    result
}

fn stop_generation_worker(child: &mut Child) -> CoreResult<()> {
    #[cfg(unix)]
    if let Some(group) = rustix::process::Pid::from_raw(child.id() as i32) {
        let stopped = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        if matches!(stopped, Ok(()) | Err(rustix::io::Errno::SRCH)) {
            child.wait()?;
            return Ok(());
        }
    }
    child.kill()?;
    child.wait()?;
    Ok(())
}

fn generation_worker_stage(line: &str) -> Option<&'static str> {
    let marker = line.strip_prefix("=== ")?.strip_suffix(" ===")?;
    Some(match marker {
        "preprocess_camera" => "preprocess_camera",
        "dino_features" => "dino_features",
        "Stage 1: Sparse Structure (proj)" | "sparse_structure" => "sparse_structure",
        "Stage 2a: LR Shape Latent (proj, 512)" | "shape_lr" => "shape_lr",
        "Stage 2b: Upsample → HR coordinates" | "coordinate_subdivision" => {
            "coordinate_subdivision"
        }
        "hr_conditioning" => "hr_conditioning",
        "texture_conditioning" => "texture_conditioning",
        "Stage 2c: HR Shape Latent (proj, 1024)" | "shape_hr" => "shape_hr",
        "Stage 3: Decode Shape" | "shape_decode" => "shape_decode",
        "Stage 4: Texture SLat (proj, 1024)" | "texture_flow" => "texture_flow",
        "Stage 5: Texture Decode" | "texture_decode" => "texture_decode",
        "text_conditioning" => "text_conditioning",
        "generate_structure" => "sparse_structure",
        "generate_latents" => "generate_latents",
        "generate_mesh" => "generate_mesh",
        "mesh_export" => "mesh_export",
        "Stage 6: Texture Bake + GLB" | "pbr_export" => "pbr_export",
        _ => return None,
    })
}

pub(crate) fn generate_model(
    core: &OperationCore,
    input: &GenerationInput,
    options: GenerationOptions,
    execution: JobExecution<'_>,
    operation: &'static str,
) -> CoreResult<GenerationResult> {
    let profile = options.validate_input(input)?;
    let selected_backend_id = profile.id;
    let work_dir = core
        .store()
        .staging_root()
        .join(format!("generate-{}", Uuid::new_v4()));
    std::fs::create_dir(&work_dir)?;
    let cleanup = WorkDir(work_dir.clone());
    let output_path = work_dir.join("result.glb");
    let (output_name, elapsed_ms) = match input {
        GenerationInput::Image { asset_id } => generate_from_images(
            core,
            asset_id,
            &options,
            selected_backend_id,
            &work_dir,
            &output_path,
            execution,
        )?,
        GenerationInput::Text { prompt } => {
            let backend = ExternalProfile::find(selected_backend_id).ok_or_else(|| {
                CoreError::OperationUnavailable(
                    "No text-conditioned worker for this backend.".into(),
                )
            })?;
            execution.stage("initialize_model")?;
            let elapsed_ms = backend.generate(
                core.store().root(),
                ExternalInput::Text(prompt),
                &output_path,
                &options,
                execution,
            )?;
            ("text-generated.glb".into(), elapsed_ms)
        }
    };
    execution.stage("validate_output")?;
    let (output, descriptor) = read_generated_glb(&output_path, selected_backend_id)?;
    execution.stage("save_model")?;
    execution.begin_commit()?;
    let texture_size = profile.has_textures().then_some(options.texture_size);
    let resolution = profile.has_resolution().then_some(options.resolution);
    let target_faces = profile.has_face_target().then_some(options.target_faces);
    let target_vertices = lato2::is_backend(selected_backend_id)
        .then_some(options.target_vertices.unwrap_or(DEFAULT_TARGET_VERTICES));
    let mut metadata = json!({
        "operation": operation,
        "input_mode": input.mode(),
        "backend": selected_backend_id,
        "seed": options.seed,
        "resolution": resolution,
        "texture_size": texture_size,
        "elapsed_ms": elapsed_ms,
        "validation": descriptor.metadata,
    });
    if let Some(target) = target_faces {
        metadata["target_faces"] = json!(target);
    }
    if let Some(target) = target_vertices {
        metadata["target_vertices"] = json!(target);
    }
    let asset =
        core.store()
            .import_generated_asset(&output_name, &output, input.asset_id(), metadata)?;
    drop(cleanup);
    Ok(GenerationResult {
        asset,
        backend_id: selected_backend_id.into(),
        seed: options.seed,
        resolution,
        texture_size,
        target_faces,
        target_vertices,
        elapsed_ms,
    })
}

fn generate_from_images(
    core: &OperationCore,
    input_asset_id: &str,
    options: &GenerationOptions,
    selected_backend_id: &str,
    work_dir: &Path,
    output_path: &Path,
    execution: JobExecution<'_>,
) -> CoreResult<(String, u128)> {
    execution.stage("prepare_references")?;
    let source = core
        .store()
        .get_asset(input_asset_id)?
        .ok_or_else(|| CoreError::NotFound(format!("Asset '{input_asset_id}' was not found.")))?;
    let mut input_paths = vec![stage_reference(core, &source, work_dir, 0)?];
    if let Some(input) = &options.view_input {
        for (index, reference) in input.references.iter().enumerate().skip(1) {
            execution.checkpoint()?;
            let image = core
                .store()
                .get_asset(&reference.asset_id)?
                .ok_or_else(|| {
                    CoreError::NotFound(format!("View {} image was not found.", index + 1))
                })?;
            input_paths.push(stage_reference(core, &image, work_dir, index)?);
        }
    }
    execution.stage("initialize_model")?;

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
                .generate(&input_paths[0], output_path, options, execution)?
                .elapsed_ms
        }
        "pixal3d-cuda" => {
            let runtime = Pixal3dCudaRuntimeManager::new(core.store().root());
            runtime.generate_from_references(&input_paths, work_dir, options, execution)?
        }
        backend_id if lato2::is_backend(backend_id) => {
            Lato2Runtime::require_image(core.store().root(), backend_id)?.generate_image(
                &input_paths[0],
                output_path,
                options.target_vertices.unwrap_or(DEFAULT_TARGET_VERTICES),
                options.seed,
                execution,
            )?
        }
        backend_id => ExternalProfile::find(backend_id)
            .expect("generation profile has a local runtime")
            .generate(
                core.store().root(),
                ExternalInput::Image(&input_paths[0]),
                output_path,
                options,
                execution,
            )?,
    };
    let output_name = source
        .name
        .rsplit_once('.')
        .map(|(stem, _)| format!("{stem}-generated.glb"))
        .unwrap_or_else(|| "generated.glb".into());
    Ok((output_name, elapsed_ms))
}

fn read_generated_glb(
    output_path: &Path,
    selected_backend_id: &str,
) -> CoreResult<(Vec<u8>, crate::AssetDescriptor)> {
    let metadata = output_path.symlink_metadata().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CoreError::InvalidInput(format!("{selected_backend_id} did not produce a GLB."))
        } else {
            error.into()
        }
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_GENERATED_GLB_BYTES {
        return Err(CoreError::InvalidInput(
            "Generation must produce a regular, nonempty GLB up to 512 MiB.".into(),
        ));
    }
    let mut output = Vec::new();
    std::fs::File::open(output_path)?
        .take(MAX_GENERATED_GLB_BYTES + 1)
        .read_to_end(&mut output)?;
    if output.len() as u64 > MAX_GENERATED_GLB_BYTES {
        return Err(CoreError::InvalidInput(
            "Generated GLB exceeds 512 MiB.".into(),
        ));
    }
    let descriptor = inspect_asset("result.glb", &output)?;
    if descriptor.kind != AssetKind::Mesh
        || descriptor.metadata["triangles"].as_u64().unwrap_or(0) == 0
    {
        return Err(CoreError::InvalidInput(
            "Generation output did not validate as a triangle GLB mesh.".into(),
        ));
    }
    Ok((output, descriptor))
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

    const MLX_ENTRYPOINT_FIXTURE: &str = concat!(
        "    naf_path = os.path.join(os.path.dirname(__file__), \"weights\", \"naf_release.safetensors\")\n",
        "    from generate import _cleanup_and_simplify_mesh\n",
    );

    fn imported_image_with_source_path(core: &OperationCore) -> (Asset, PathBuf) {
        let png = crate::glb::test_png_with_dimensions(1, 1);
        let asset = core.store().import_asset("legacy.png", &png).unwrap();
        let source = asset
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == "source")
            .unwrap();
        let path = core.store().artifact_path(&source.id).unwrap();
        (asset, path)
    }

    fn expected_managed_source(source: &str) -> String {
        const UPSTREAM_NAF_PATH: &str = "    naf_path = os.path.join(os.path.dirname(__file__), \"weights\", \"naf_release.safetensors\")";
        const MANAGED_NAF_PATH: &str = concat!(
            "    naf_weights_dir = os.environ.get(\"MESHHARBOR_PIXAL3D_WEIGHTS_DIR\", ",
            "os.path.join(os.path.dirname(__file__), \"weights\"))\n",
            "    naf_path = os.path.join(naf_weights_dir, \"naf_release.safetensors\")"
        );
        patch_mlx_postprocess(&source.replace(UPSTREAM_NAF_PATH, MANAGED_NAF_PATH)).unwrap()
    }

    #[test]
    fn recognizes_only_known_stage_markers_from_both_workers() {
        assert_eq!(
            generation_worker_stage("=== Stage 2b: Upsample → HR coordinates ==="),
            Some("coordinate_subdivision")
        );
        assert_eq!(
            generation_worker_stage("=== texture_flow ==="),
            Some("texture_flow")
        );
        assert_eq!(generation_worker_stage("=== model log ==="), None);
        assert_eq!(generation_worker_stage("printed === pbr_export ==="), None);
    }

    #[cfg(unix)]
    #[test]
    fn writes_text_input_to_worker_stdin_and_closes_the_pipe() {
        let mut command = Command::new("sh");
        command.args(["-c", "test \"$(cat)\" = '도자기 컵'"]);
        let status = run_generation_command_with_input(
            &mut command,
            JobExecution::default(),
            Some("도자기 컵".as_bytes()),
        )
        .unwrap();
        assert!(status.success());
    }

    #[cfg(unix)]
    #[test]
    fn tracked_worker_persists_fast_stages_and_bounded_redacted_output() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-generation-trace-{}", Uuid::new_v4()));
        let store = crate::Store::open(&root).unwrap();
        let job = store
            .create_running_job("model.generate", None, &json!({}))
            .unwrap();
        let execution = JobExecution::for_job(&store, &job.id);
        execution.stage("initialize_model").unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "cat >/dev/null; printf '%32768s\\n' ''; printf '=== shape_hr ===\\n=== mesh_export ===\\n'; printf '%s\\n' \"$WORKER_MESSAGE\" >&2; exit 17"])
            .env("WORKER_MESSAGE", "Invalid URL https://user:secret@example.invalid/file?sig=temporary");
        let status =
            run_generation_command_with_input(&mut command, execution, Some(b"private prompt"))
                .unwrap();
        assert_eq!(status.code(), Some(17));
        let steps = store.job_steps(&job.id).unwrap();
        assert_eq!(steps[2].stage.as_deref(), Some("shape_hr"));
        assert_eq!(steps[3].stage.as_deref(), Some("mesh_export"));
        let worker = &steps[1].commands[0];
        assert!(worker.truncated);
        assert!(worker.stdout.len() <= 16 * 1024);
        assert!(
            worker
                .stderr
                .contains("https://example.invalid/file?[redacted]")
        );
        let serialized = serde_json::to_string(&steps).unwrap();
        assert!(!serialized.contains("temporary"));
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("private prompt"));
        assert!(worker.outcome.as_deref().unwrap().contains("17"));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generation_rejects_changed_stored_images_before_either_backend() {
        let root = std::env::temp_dir().join(format!(
            "meshharbor-legacy-image-validation-{}",
            Uuid::new_v4()
        ));
        let core = OperationCore::open(&root).unwrap();
        let (asset, path) = imported_image_with_source_path(&core);
        let changed = crate::glb::test_png_with_dimensions(2, 1);
        std::fs::write(path, changed).unwrap();

        for backend_id in ["pixal3d-mlx", "pixal3d-cuda"] {
            let result = core.generate_image(
                &asset.id,
                GenerationOptions {
                    backend_id: backend_id.into(),
                    ..GenerationOptions::default()
                },
            );
            assert!(
                matches!(result, Err(CoreError::Conflict(_))),
                "{backend_id} must reject changed stored image bytes before backend startup"
            );
        }
        assert!(
            std::fs::read_dir(core.store().staging_root())
                .unwrap()
                .next()
                .is_none()
        );
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recorded_hashes_and_valid_image_format_are_accepted() {
        let png = crate::glb::test_png_with_dimensions(1, 1);
        let sha256 = sha256_hex(&png);
        assert!(
            validate_stored_source_image("source.png", "image/png", &sha256, &sha256, &png).is_ok()
        );
    }

    #[test]
    fn asset_and_artifact_hashes_are_checked_independently() {
        let png = crate::glb::test_png_with_dimensions(1, 1);
        let sha256 = sha256_hex(&png);

        assert!(matches!(
            validate_stored_source_image(
                "source.png",
                "image/png",
                "different-asset-digest",
                &sha256,
                &png
            ),
            Err(CoreError::Conflict(_))
        ));
        assert!(matches!(
            validate_stored_source_image(
                "source.png",
                "image/png",
                &sha256,
                "different-artifact-digest",
                &png
            ),
            Err(CoreError::Conflict(_))
        ));
    }

    #[test]
    fn matching_hash_does_not_make_malformed_legacy_image_valid() {
        let mut corrupt = crate::glb::test_png_with_dimensions(1, 1);
        corrupt[29] ^= 1;
        let sha256 = sha256_hex(&corrupt);
        let error =
            validate_stored_source_image("legacy.png", "image/png", &sha256, &sha256, &corrupt)
                .unwrap_err();
        assert!(matches!(error, CoreError::InvalidInput(_)));
    }

    #[test]
    fn generation_bounds_oversized_stored_images_before_either_backend() {
        let root =
            std::env::temp_dir().join(format!("meshharbor-legacy-image-size-{}", Uuid::new_v4()));
        let core = OperationCore::open(&root).unwrap();
        let (asset, path) = imported_image_with_source_path(&core);
        std::fs::File::create(path)
            .unwrap()
            .set_len((MAX_IMAGE_BYTES + 1) as u64)
            .unwrap();

        for backend_id in ["pixal3d-mlx", "pixal3d-cuda"] {
            let result = core.generate_image(
                &asset.id,
                GenerationOptions {
                    backend_id: backend_id.into(),
                    ..GenerationOptions::default()
                },
            );
            assert!(
                matches!(result, Err(CoreError::InvalidInput(_))),
                "{backend_id} must reject over-limit stored images before backend startup"
            );
        }
        assert!(
            std::fs::read_dir(core.store().staging_root())
                .unwrap()
                .next()
                .is_none()
        );
        assert_eq!(core.store().list_assets(10).unwrap().len(), 1);
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
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
    fn managed_mlx_sampling_overlays_only_the_reviewed_pinned_sampler_assignments() {
        let upstream = include_str!("../../../runtimes/pixal3d/mlx/upstream/generate_pixal3d.py");
        let patched = patch_mlx_samplers(upstream).unwrap();
        assert!(
            patched.contains("_meshharbor_tune_sampler(\"sparse_structure_sampler\", SS_SAMPLER)")
        );
        assert!(
            patched.contains("_meshharbor_tune_sampler(\"shape_slat_sampler\", SHAPE_SAMPLER)")
        );
        assert!(patched.contains("_meshharbor_tune_sampler(\"tex_slat_sampler\", TEX_SAMPLER)"));
        assert!(
            patch_mlx_samplers(&upstream.replace("SS_SAMPLER = dict(", "SS_SAMPLER = something("))
                .is_err()
        );
    }

    #[test]
    fn managed_mlx_postprocess_requires_the_unmodified_upstream_import() {
        let upstream = include_str!("../../../runtimes/pixal3d/mlx/upstream/generate_pixal3d.py");
        let import = "    from generate import _cleanup_and_simplify_mesh";
        let patched = patch_mlx_postprocess(upstream).unwrap();
        assert!(!patched.lines().any(|line| line == import));
        assert!(patched.contains("    def _cleanup_and_simplify_mesh(vertices, faces, *"));
        assert!(patched.contains("    vertices, faces = _cleanup_and_simplify_mesh("));
        assert!(patch_mlx_postprocess(&upstream.replace(import, "")).is_err());
        assert!(patch_mlx_postprocess(&format!("{upstream}\n{import}\n")).is_err());
    }

    #[test]
    fn managed_entrypoint_uses_app_data_weights_without_mutating_pinned_source() {
        let root = std::env::temp_dir().join(format!("meshharbor-mlx-overlay-{}", Uuid::new_v4()));
        let source_root = root.join("source");
        let runtime_root = root.join("runtime");
        std::fs::create_dir_all(source_root.join("pixalmlx")).unwrap();
        let upstream = source_root.join("generate_pixal3d.py");
        let source = MLX_ENTRYPOINT_FIXTURE;
        std::fs::write(&upstream, source).unwrap();
        let runtime = Pixal3dMlxRuntime {
            python: runtime_root.join(".venv/bin/python"),
            root: source_root,
            home: runtime_root.join("home"),
            weights: runtime_root.join("weights"),
        };

        let entrypoint = runtime.prepare_entrypoint(false).unwrap();
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

        let upstream_one = source_one.join("generate_pixal3d.py");
        let upstream_two = source_two.join("generate_pixal3d.py");
        let source_one_text = format!("# source-one marker\n{MLX_ENTRYPOINT_FIXTURE}");
        let source_two_text = format!("# source-two marker\n{MLX_ENTRYPOINT_FIXTURE}");
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

        let entrypoint_one = runtime_one.prepare_entrypoint(false).unwrap();
        let path_one = entrypoint_one.path().to_path_buf();
        let entrypoint_two = runtime_two.prepare_entrypoint(false).unwrap();
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
        let source = MLX_ENTRYPOINT_FIXTURE;
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
                    runtime.prepare_entrypoint(false).unwrap()
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
