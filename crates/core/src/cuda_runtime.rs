//! Managed Linux/NVIDIA runtime for the vendored Pixal3D CUDA backend.
//!
//! Setup only installs source dependencies into an app-private uv environment.
//! Model files are a separate, explicitly-confirmed action. Hugging Face login
//! and gated DINOv3 access remain under the account owner's control.

use std::{
    env,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::Value;
use uuid::Uuid;

use crate::{
    CoreError, CoreResult, HuggingFaceAccess, HuggingFaceConnection,
    generation::RuntimeReadiness,
    generation_tuning,
    huggingface_access::{DINO_REVISION, check_connection, check_model_access, require_access},
    huggingface_browser::access_python,
    huggingface_download::{
        self, DINO_REPO, PIXAL_CHECKPOINTS, PIXAL_MULTIVIEW_REVISION, PIXAL_REPOSITORY,
        PIXAL_REVISION,
    },
    job_execution::JobExecution,
    model_environment::ModelEnvironment,
    runtime::RuntimeSetupReport,
};

const DINO_REPOSITORY: &str = "models--facebook--dinov3-vitl16-pretrain-lvd1689m";
const MAX_GENERATED_GLB_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CUDA_PREFLIGHT_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone)]
pub struct Pixal3dCudaRuntimeManager {
    data_root: PathBuf,
    runtime_root: PathBuf,
    upstream_root: PathBuf,
    managed_root: PathBuf,
}

impl Pixal3dCudaRuntimeManager {
    pub fn new(data_root: impl AsRef<Path>) -> Self {
        let data_root = data_root.as_ref().to_path_buf();
        let runtime_root = env::var_os("MESHHARBOR_PIXAL3D_CUDA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_root.join("runtimes/pixal3d-cuda"));
        let source_root = runtime_root.join("source");
        Self {
            data_root,
            upstream_root: source_root.join("upstream"),
            managed_root: source_root.join("managed"),
            runtime_root,
        }
    }

    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    pub fn python_path(&self) -> PathBuf {
        env::var_os("MESHHARBOR_PIXAL3D_CUDA_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.runtime_root.join(".venv/bin/python"))
    }

    pub fn home_path(&self) -> PathBuf {
        self.runtime_root.join("home")
    }

    pub fn huggingface_connection(&self, token: Option<&str>) -> CoreResult<HuggingFaceConnection> {
        check_connection(self.huggingface_command()?, token)
    }

    pub fn huggingface_model_access(&self) -> CoreResult<HuggingFaceAccess> {
        check_model_access(self.huggingface_command()?)
    }

    fn huggingface_command(&self) -> CoreResult<Command> {
        self.ensure_supported_host()?;
        let python = ModelEnvironment::load(&self.data_root)
            .map(|environment| environment.hf_python())
            .or_else(|| access_python(&self.runtime_root))
            .unwrap_or_else(|| self.python_path());
        if !python.is_file() {
            return Err(CoreError::OperationUnavailable(
                "Prepare browser sign-in or set up the CUDA environment before connecting a token."
                    .into(),
            ));
        }
        Ok(self.python_command(&python))
    }

    fn hf_home(&self) -> PathBuf {
        self.home_path().join(".cache/huggingface")
    }

    fn cache_path(&self) -> PathBuf {
        self.runtime_root.join("uv-cache")
    }

    /// Populate a private source copy from the pinned bytes embedded in the
    /// binary. No checkout path is consulted at runtime.
    pub fn ensure_source(&self) -> CoreResult<()> {
        for (relative, contents) in crate::runtime_bundle::PIXAL3D_CUDA_FILES.iter().copied() {
            let target = self.runtime_root.join("source").join(relative);
            if target.is_file() && std::fs::read(&target)?.as_slice() == contents {
                continue;
            }
            let parent = target.parent().ok_or_else(|| {
                CoreError::OperationUnavailable(
                    "Embedded CUDA source path has no parent directory.".into(),
                )
            })?;
            std::fs::create_dir_all(parent)?;
            std::fs::write(target, contents)?;
        }
        self.validate_source()
    }

    fn validate_source(&self) -> CoreResult<()> {
        for path in [
            self.upstream_root.join("pyproject.toml"),
            self.upstream_root.join("pixalcuda/cli.py"),
            self.upstream_root.join("pixalcuda/pipeline.py"),
            self.upstream_root.join("pixalcuda/weights.py"),
            self.managed_root.join("pyproject.toml"),
        ] {
            if !path.is_file() {
                return Err(CoreError::OperationUnavailable(format!(
                    "Embedded Pixal3D CUDA source is incomplete; missing {}.",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    /// Set up a private Python 3.12/CUDA 12.4-compatible environment. This
    /// requires Linux x86_64 and a visible NVIDIA device, and never downloads
    /// model weights. Since upstream does not ship a lock file, the first
    /// explicit setup resolves and stores a per-install uv.lock in app data.
    pub fn setup_environment(&self) -> CoreResult<RuntimeSetupReport> {
        self.setup_environment_tracked(JobExecution::default())
    }

    pub(crate) fn setup_environment_tracked(
        &self,
        execution: JobExecution<'_>,
    ) -> CoreResult<RuntimeSetupReport> {
        execution.stage("prepare_runtime")?;
        let environment = ModelEnvironment::require(&self.data_root)?;
        let device = self.detect_gpu_tracked(execution)?;
        self.invalidate_preflight_marker()?;
        self.ensure_source()?;
        self.setup_environment_with_tools(environment.uv(), &device, execution)
    }

    fn setup_environment_with_tools(
        &self,
        uv: &Path,
        device: &str,
        execution: JobExecution<'_>,
    ) -> CoreResult<RuntimeSetupReport> {
        self.invalidate_preflight_marker()?;
        let python_dir = self.runtime_root.join("python");
        let venv_dir = self.runtime_root.join(".venv");
        let home = self.home_path();
        std::fs::create_dir_all(&python_dir)?;
        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(self.cache_path())?;

        eprintln!("NVIDIA GPU detected: {device}");
        let python = crate::runtime::prepare_managed_venv(
            execution,
            &python_dir,
            &venv_dir,
            ModelEnvironment::load(&self.data_root)
                .as_ref()
                .map(|environment| environment.python("3.12")),
            || self.uv_command(uv),
            |path| self.python_command(path),
        )?;
        let lockfile = self.managed_root.join("uv.lock");
        execution.stage("resolve_dependencies")?;
        if !lockfile.is_file() {
            eprintln!(
                "[3/4] Resolving the vendored dependency ranges into a private per-install uv.lock…"
            );
            let mut lock = self.uv_command(uv);
            lock.current_dir(&self.managed_root)
                .arg("lock")
                .arg("--python")
                .arg(&python);
            execution.visible(lock, "uv could not resolve the CUDA dependency lock")?;
        } else {
            execution.note("Reusing the existing managed CUDA uv.lock.")?;
        }

        execution.stage("sync_dependencies")?;
        let mut sync = self.uv_command(uv);
        sync.current_dir(&self.managed_root)
            .env("UV_PROJECT_ENVIRONMENT", &venv_dir)
            .args(["sync", "--project"])
            .arg(&self.managed_root)
            .arg("--python")
            .arg(&python)
            .args(["--locked", "--no-dev", "--no-editable"]);
        execution.visible(sync, "uv could not install the locked CUDA dependencies")?;

        // Upstream's doctor emits non-sensitive device/runtime diagnostics and
        // executes its FP16/FP32 SDPA finite-value smoke test. Reject CPU-only
        // or incompatible driver environments before making the backend ready.
        execution.stage("verify_environment")?;
        let report = self.verify_cuda_environment(&python, execution)?;
        self.publish_preflight_marker(&report)?;

        Ok(RuntimeSetupReport {
            backend_id: "pixal3d-cuda".into(),
            status: "python_environment_ready_models_not_installed".into(),
            runtime_root: self.runtime_root.clone(),
            python,
            lockfile,
            message: "The isolated Python environment and CUDA preflight succeeded. Model weights remain a separate, explicitly confirmed 25–30 GB download. Upstream has no committed lock; this install resolved one locally. Gated DINOv3 access and upstream model terms remain separate from MeshHarbor's MIT license.".into(),
        })
    }

    fn invalidate_preflight_marker(&self) -> CoreResult<()> {
        let marker = self.runtime_root.join("cuda-preflight.json");
        match std::fs::remove_file(marker) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn publish_preflight_marker(&self, report: &Value) -> CoreResult<()> {
        if !cuda_preflight_report_matches_profile(report) {
            return Err(CoreError::OperationUnavailable(
                "CUDA preflight returned a report outside the reviewed runtime profile.".into(),
            ));
        }
        let contents = serde_json::to_vec_pretty(report)?;
        if contents.is_empty() || contents.len() as u64 > MAX_CUDA_PREFLIGHT_BYTES {
            return Err(CoreError::OperationUnavailable(
                "CUDA preflight report exceeds the supported marker size.".into(),
            ));
        }

        let marker = self.runtime_root.join("cuda-preflight.json");
        let temporary = self
            .runtime_root
            .join(format!(".cuda-preflight-{}.tmp", Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let write_result = file.write_all(&contents).and_then(|()| file.sync_all());
        if let Err(error) = write_result {
            drop(file);
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        drop(file);

        if let Err(error) = std::fs::rename(&temporary, &marker) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        Ok(())
    }

    /// Sign in through Hugging Face's own terminal prompt inside the isolated
    /// MeshHarbor cache; no token enters MeshHarbor's API or logs.
    pub fn login_huggingface(&self) -> CoreResult<()> {
        self.ensure_supported_host()?;
        let executable = ModelEnvironment::require(&self.data_root)?.hf_cli();
        std::fs::create_dir_all(self.home_path())?;
        eprintln!(
            "Hugging Face will prompt for the token in this terminal. It is saved only in MeshHarbor's isolated cache."
        );
        let mut command = Command::new(executable);
        command
            .args(["auth", "login"])
            .env_clear()
            .env("HOME", self.home_path())
            .env("HF_HOME", self.hf_home())
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("PATH", managed_path(&self.runtime_root));
        JobExecution::default().visible(
            command,
            "Hugging Face login failed; MeshHarbor did not receive the token",
        )
    }

    /// Download only the single-view CUDA model profile (no MoGe and no
    /// rembg). Confirmation booleans must be supplied by the caller immediately
    /// before the 25–30 GB transfer and after gated DINOv3 access was granted.
    pub fn install_models(
        &self,
        confirm_large_download: bool,
        confirm_gated_access: bool,
    ) -> CoreResult<RuntimeSetupReport> {
        self.install_models_tracked(
            confirm_large_download,
            confirm_gated_access,
            JobExecution::default(),
        )
    }

    pub(crate) fn install_models_tracked(
        &self,
        confirm_large_download: bool,
        confirm_gated_access: bool,
        execution: JobExecution<'_>,
    ) -> CoreResult<RuntimeSetupReport> {
        execution.stage("prepare_runtime")?;
        if !confirm_large_download || !confirm_gated_access {
            return Err(CoreError::InvalidInput(
                "CUDA model installation requires explicit confirmation of the 25–30 GB download and prior DINOv3 gated access.".into(),
            ));
        }
        self.ensure_supported_host()?;
        ModelEnvironment::require(&self.data_root)?;
        self.ensure_source()?;
        let python = self.python_path();
        if !python.is_file() {
            return Err(CoreError::OperationUnavailable(
                "Run `meshharbor runtime cuda-setup` before installing CUDA models.".into(),
            ));
        }
        self.install_models_with_python(
            &python,
            huggingface_download::shared_hub_cache().as_deref(),
            execution,
        )
    }

    fn install_models_with_python(
        &self,
        python: &Path,
        shared_cache: Option<&Path>,
        execution: JobExecution<'_>,
    ) -> CoreResult<RuntimeSetupReport> {
        let home = self.home_path();
        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(self.hf_home())?;

        execution.stage("check_model_access")?;
        let access_python =
            access_python(&self.runtime_root).unwrap_or_else(|| python.to_path_buf());
        require_access(
            self.python_command(&access_python),
            "meshharbor runtime cuda-hf-login",
        )?;

        execution.stage("download_weights")?;
        huggingface_download::download(
            &self.runtime_root,
            &home,
            DINO_REPO,
            DINO_REVISION,
            ["model.safetensors"],
            execution,
        )?;
        huggingface_download::download_pixal3d(
            &self.runtime_root,
            &home,
            shared_cache,
            false,
            execution,
        )?;

        execution.stage("verify_weights")?;
        let mut naf = self.python_command(python);
        naf.current_dir(&self.upstream_root)
            .args([
                "-c",
                "from pixalcuda.weights import ModelStore; ModelStore().naf()",
            ])
            .stdout(Stdio::from(std::io::stderr()))
            .stderr(Stdio::inherit());
        execution.visible(naf, "Could not download or verify the CUDA NAF checkpoint")?;

        let readiness = self.readiness();
        if !readiness.models_downloaded {
            return Err(CoreError::OperationUnavailable(format!(
                "Model downloader finished but the pinned CUDA file set is incomplete: {}",
                readiness.missing.join("; ")
            )));
        }
        Ok(RuntimeSetupReport {
            backend_id: "pixal3d-cuda".into(),
            status: "model_files_ready".into(),
            runtime_root: self.runtime_root.clone(),
            python: python.to_path_buf(),
            lockfile: self.managed_root.join("uv.lock"),
            message: "Pinned Pixal3D and DINOv3 snapshots and SHA-256-verified NAF weights are present in the isolated HF cache. Inference remains offline; model-specific terms are separate from MeshHarbor's MIT license.".into(),
        })
    }

    pub fn readiness(&self) -> RuntimeReadiness {
        let mut missing = Vec::new();
        let supported = is_supported_target(env::consts::OS, env::consts::ARCH);
        if !supported {
            missing.push(
                "Pixal3D CUDA is supported only on Linux x86_64 with an NVIDIA CUDA device.".into(),
            );
        }
        if let Err(error) = self.detect_gpu() {
            missing.push(error.to_string());
        }
        let environment_ready = cuda_environment_is_ready(
            &self.python_path(),
            &self.managed_root.join("uv.lock"),
            &self.upstream_root.join("pixalcuda/cli.py"),
            &self.runtime_root.join("cuda-preflight.json"),
        );
        if !environment_ready {
            missing.push(format!(
                "Managed CUDA Python environment is missing or incomplete at {}; run `meshharbor runtime cuda-setup`.",
                self.runtime_root.display()
            ));
        }

        let hub = self.hf_home().join("hub");
        let pixal_snapshot = hub
            .join(PIXAL_REPOSITORY)
            .join("snapshots")
            .join(PIXAL_REVISION);
        let dino_snapshot = hub
            .join(DINO_REPOSITORY)
            .join("snapshots")
            .join(DINO_REVISION);
        let mut model_files_ready = true;
        let mut models_downloaded = true;
        for filename in PIXAL_CHECKPOINTS {
            if !pixal_snapshot.join("ckpts").join(filename).is_file() {
                model_files_ready = false;
                models_downloaded = false;
                missing.push(format!("Missing pinned Pixal3D checkpoint: {filename}"));
            }
        }
        for path in [
            pixal_snapshot.join("pipeline.json"),
            dino_snapshot.join("model.safetensors"),
            hub.join("pixal3d-cuda/naf_release.pth"),
        ] {
            if !path.is_file() {
                model_files_ready = false;
                models_downloaded = false;
                missing.push(format!(
                    "Missing pinned CUDA model file: {}",
                    path.display()
                ));
            }
        }

        RuntimeReadiness {
            backend_id: "pixal3d-cuda".into(),
            ready: supported && environment_ready && model_files_ready && self.detect_gpu().is_ok(),
            validation_level:
                "required_paths_nvidia_smi_and_last_cuda_smoke_test_current_driver_not_rechecked"
                    .into(),
            runtime_root: Some(self.runtime_root.clone()),
            python: Some(self.python_path()),
            missing,
            environment_ready,
            model_files_ready,
            models_downloaded,
            naf_conversion_required: false,
            conversion_enabled: false,
            downloads_enabled: supported && environment_ready && !models_downloaded,
            gated_model_access_required: true,
        }
    }

    pub fn host_supported(&self) -> bool {
        is_supported_target(env::consts::OS, env::consts::ARCH) && self.detect_gpu().is_ok()
    }

    pub fn install_multiview_models(&self, confirmed: bool) -> CoreResult<RuntimeSetupReport> {
        self.install_multiview_models_tracked(confirmed, JobExecution::default())
    }

    pub(crate) fn install_multiview_models_tracked(
        &self,
        confirmed: bool,
        execution: JobExecution<'_>,
    ) -> CoreResult<RuntimeSetupReport> {
        execution.stage("prepare_runtime")?;
        if !confirmed {
            return Err(CoreError::InvalidInput(
                "Confirm the additional model files before downloading multiview weights.".into(),
            ));
        }
        ModelEnvironment::require(&self.data_root)?;
        if !self.readiness().ready {
            return Err(CoreError::OperationUnavailable(
                "Set up the base Pixal3D model before adding multiview weights.".into(),
            ));
        }
        execution.stage("download_weights")?;
        huggingface_download::download_pixal3d_multiview(
            &self.runtime_root,
            &self.home_path(),
            execution,
        )?;
        execution.stage("verify_weights")?;
        if !huggingface_download::multiview_files_present(&self.home_path()) {
            return Err(CoreError::OperationUnavailable(
                "The pinned multiview snapshot is incomplete.".into(),
            ));
        }
        Ok(RuntimeSetupReport {
            backend_id: "pixal3d-cuda".into(),
            status: "multiview_files_ready".into(),
            runtime_root: self.runtime_root.clone(),
            python: self.python_path(),
            lockfile: self.managed_root.join("uv.lock"),
            message: "Pinned Pixal3D multiview weights are ready for offline generation.".into(),
        })
    }

    pub fn generate(
        &self,
        input: &Path,
        output_dir: &Path,
        options: &crate::GenerationOptions,
    ) -> CoreResult<u128> {
        self.generate_from_references(
            &[input.to_path_buf()],
            output_dir,
            options,
            Default::default(),
        )
    }

    pub(crate) fn generate_from_references(
        &self,
        inputs: &[PathBuf],
        output_dir: &Path,
        options: &crate::GenerationOptions,
        execution: crate::job_execution::JobExecution<'_>,
    ) -> CoreResult<u128> {
        let readiness = self.readiness();
        if !readiness.ready {
            return Err(CoreError::OperationUnavailable(format!(
                "Pixal3D CUDA runtime is not ready: {}. Setup and model downloads are explicit; no weights are fetched during generation.",
                readiness.missing.join("; ")
            )));
        }
        options.validate_for_backend("pixal3d-cuda")?;
        let expected_inputs = options
            .view_input
            .as_ref()
            .map_or(1, |input| input.references.len());
        if inputs.len() != expected_inputs {
            return Err(CoreError::InvalidInput(
                "The staged images do not match the selected view mode.".into(),
            ));
        }
        let multiview = options.view_input.is_some();
        if multiview && !huggingface_download::multiview_files_present(&self.home_path()) {
            return Err(CoreError::OperationUnavailable("Install the optional Pixal3D multiview weights before generating from posed views.".into()));
        }
        let python = self.python_path();
        self.verify_cuda_environment(&python, execution)?;
        std::fs::create_dir_all(output_dir)?;
        let snapshot = self
            .hf_home()
            .join("hub")
            .join(PIXAL_REPOSITORY)
            .join("snapshots")
            .join(if multiview {
                PIXAL_MULTIVIEW_REVISION
            } else {
                PIXAL_REVISION
            });
        let started = std::time::Instant::now();
        let mut command = self.python_command(&python);
        command
            .current_dir(&self.upstream_root)
            // No ambient account token or user Python modules are exposed to
            // the model worker. The already-installed local files are enough.
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .arg("-c")
            .arg(concat!(
                include_str!("sampler_overrides.py"),
                "\n",
                include_str!("cuda_generation.py")
            ))
            .arg("generate")
            .arg("--device")
            .arg("cuda")
            .arg("--output-dir")
            .arg(output_dir)
            .arg("--model-dir")
            .arg(snapshot)
            .arg("--seed")
            .arg(options.seed.to_string())
            .arg("--texture-size")
            .arg(options.texture_size.to_string())
            .arg("--target-faces")
            .arg(options.target_faces.to_string())
            .arg("--no-rembg");
        if let Some(input) = &options.view_input {
            let transforms = output_dir.join("transforms.json");
            std::fs::write(&transforms, serde_json::to_vec(&input.camera_metadata())?)?;
            command
                .arg("--images")
                .args(inputs)
                .arg("--transforms")
                .arg(transforms);
        } else {
            command
                .arg("--image")
                .arg(&inputs[0])
                .args(["--camera", "fixed"]);
        }
        generation_tuning::add_arguments(&mut command, options, "pixal3d-cuda");
        let status = crate::generation::run_generation_command(&mut command, execution)?;
        if !status.success() {
            return Err(CoreError::OperationUnavailable(format!(
                "Pixal3D CUDA inference failed ({status}). {}",
                execution.output_hint()
            )));
        }

        let output = output_dir.join("result.glb");
        if !output.is_file() {
            return Err(CoreError::InvalidInput(
                "Pixal3D CUDA exited successfully but did not produce result.glb.".into(),
            ));
        }
        let bytes = std::fs::metadata(output)?.len();
        if bytes == 0 || bytes > MAX_GENERATED_GLB_BYTES {
            return Err(CoreError::InvalidInput(format!(
                "Generated GLB size ({bytes} bytes) is outside the supported 1..={MAX_GENERATED_GLB_BYTES} byte range."
            )));
        }
        Ok(started.elapsed().as_millis())
    }

    fn ensure_supported_host(&self) -> CoreResult<()> {
        if !is_supported_target(env::consts::OS, env::consts::ARCH) {
            return Err(CoreError::OperationUnavailable(
                "The managed Pixal3D CUDA runtime currently requires Linux x86_64 with an NVIDIA CUDA GPU; macOS MLX remains a separate backend.".into(),
            ));
        }
        self.detect_gpu().map(|_| ())
    }

    fn detect_gpu(&self) -> CoreResult<String> {
        self.detect_gpu_tracked(JobExecution::default())
    }

    fn detect_gpu_tracked(&self, execution: JobExecution<'_>) -> CoreResult<String> {
        if !is_supported_target(env::consts::OS, env::consts::ARCH) {
            return Err(CoreError::OperationUnavailable(
                "CUDA is available only on Linux x86_64 in this managed profile.".into(),
            ));
        }
        let stdout = execution.capture(
            Command::new("nvidia-smi").args(["--query-gpu=name", "--format=csv,noheader"]),
            "nvidia-smi could not query an NVIDIA GPU",
        )?;
        let name = stdout
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
            .unwrap_or("");
        if name.is_empty() {
            return Err(CoreError::OperationUnavailable(
                "nvidia-smi found no NVIDIA GPU.".into(),
            ));
        }
        Ok(name.to_owned())
    }

    fn python_command(&self, python: &Path) -> Command {
        let mut command = Command::new(python);
        command
            .env_clear()
            .env("HOME", self.home_path())
            .env("HF_HOME", self.hf_home())
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("PYTHONNOUSERSITE", "1")
            .env("VIRTUAL_ENV", self.runtime_root.join(".venv"))
            .env("PATH", managed_path(&self.runtime_root));
        for temporary_key in ["TMPDIR", "TEMP", "TMP"] {
            if let Some(value) = env::var_os(temporary_key) {
                command.env(temporary_key, value);
            }
        }
        command
    }

    fn verify_cuda_environment(
        &self,
        python: &Path,
        execution: JobExecution<'_>,
    ) -> CoreResult<Value> {
        let mut command = self.python_command(python);
        command.args(["-m", "pixalcuda.cli", "doctor"]);
        let output = execution.capture(&mut command, "PyTorch CUDA preflight failed")?;
        let report: Value = serde_json::from_str(&output).map_err(|error| {
            CoreError::OperationUnavailable(format!(
                "Pixal3D CUDA doctor returned invalid JSON: {error}"
            ))
        })?;
        if !cuda_preflight_report_matches_profile(&report) {
            return Err(CoreError::OperationUnavailable(format!(
                "PyTorch/CUDA preflight did not meet the reviewed profile (requires torch 2.6.0, CUDA 12.4, CUDA available, and finite FP16/FP32 SDPA): {}",
                output.trim()
            )));
        }
        let versions = "import torch, torchvision; assert torch.__version__.startswith('2.6.0'); assert torchvision.__version__.startswith('0.21.0')";
        let mut verify_packages = self.python_command(python);
        verify_packages.args(["-c", versions]);
        execution.visible(
            verify_packages,
            "Installed CUDA package versions do not match the reviewed PyTorch profile",
        )?;
        Ok(report)
    }

    fn uv_command(&self, uv: &Path) -> Command {
        let mut command = Command::new(uv);
        command
            .env_clear()
            .env("HOME", self.home_path())
            .env("PATH", managed_path(&self.runtime_root))
            .env("UV_PYTHON_INSTALL_DIR", self.runtime_root.join("python"))
            .env("UV_CACHE_DIR", self.cache_path())
            .env("UV_PROJECT_ENVIRONMENT", self.runtime_root.join(".venv"))
            .env("UV_NO_CONFIG", "1")
            .env("UV_NO_PROGRESS", "0")
            .env("UV_PYTHON_DOWNLOADS", "never")
            .env("UV_DEFAULT_INDEX", "https://pypi.org/simple");
        command
    }
}

fn cuda_preflight_report_matches_profile(report: &Value) -> bool {
    report.get("cuda_available").and_then(Value::as_bool) == Some(true)
        && report
            .get("torch")
            .and_then(Value::as_str)
            .is_some_and(|version| version.starts_with("2.6.0"))
        && report
            .get("cuda_runtime")
            .and_then(Value::as_str)
            .is_some_and(|version| version.starts_with("12.4"))
        && report.get("sdpa_torch.float16").and_then(Value::as_bool) == Some(true)
        && report.get("sdpa_torch.float32").and_then(Value::as_bool) == Some(true)
}

fn read_valid_cuda_preflight_marker(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if metadata.len() > MAX_CUDA_PREFLIGHT_BYTES {
        return false;
    }

    let mut contents = Vec::new();
    let mut bounded_file = file.take(MAX_CUDA_PREFLIGHT_BYTES + 1);
    if bounded_file.read_to_end(&mut contents).is_err()
        || contents.is_empty()
        || contents.len() as u64 > MAX_CUDA_PREFLIGHT_BYTES
    {
        return false;
    }
    serde_json::from_slice::<Value>(&contents)
        .is_ok_and(|report| cuda_preflight_report_matches_profile(&report))
}

fn cuda_environment_is_ready(
    python: &Path,
    lockfile: &Path,
    entrypoint: &Path,
    marker: &Path,
) -> bool {
    python.is_file()
        && lockfile.is_file()
        && entrypoint.is_file()
        && read_valid_cuda_preflight_marker(marker)
}

fn is_supported_target(os: &str, arch: &str) -> bool {
    os == "linux" && arch == "x86_64"
}

fn managed_path(runtime_root: &Path) -> String {
    let mut paths = vec![runtime_root.join(".venv/bin"), runtime_root.join("python")];
    if let Some(path) = env::var_os("PATH") {
        paths.extend(env::split_paths(&path));
    }
    env::join_paths(paths)
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn cuda_profile_is_linux_x86_64_only() {
        assert!(is_supported_target("linux", "x86_64"));
        assert!(!is_supported_target("macos", "aarch64"));
        assert!(!is_supported_target("windows", "x86_64"));
        assert!(!is_supported_target("linux", "aarch64"));
    }

    #[test]
    fn unconfirmed_large_or_gated_download_is_rejected() {
        let root = env::temp_dir().join(format!("meshharbor-cuda-{}", Uuid::new_v4()));
        let manager = Pixal3dCudaRuntimeManager::new(&root);
        assert!(manager.install_models(false, false).is_err());
        assert!(manager.install_models(true, false).is_err());
        assert!(manager.install_models(false, true).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_weights_never_report_cuda_ready() {
        let root = env::temp_dir().join(format!("meshharbor-cuda-ready-{}", Uuid::new_v4()));
        let manager = Pixal3dCudaRuntimeManager::new(&root);
        let readiness = manager.readiness();
        assert_eq!(readiness.backend_id, "pixal3d-cuda");
        assert!(!readiness.ready);
        assert!(!readiness.models_downloaded);
        assert!(readiness.gated_model_access_required);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    fn write_test_script(path: &Path, contents: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, contents).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(unix)]
    fn isolated_manager(root: &Path) -> (Pixal3dCudaRuntimeManager, PathBuf) {
        let data_root = root.join("app-data");
        let runtime_root = data_root.join("runtimes/pixal3d-cuda");
        std::fs::create_dir_all(&data_root).unwrap();
        (
            Pixal3dCudaRuntimeManager {
                data_root: data_root.clone(),
                upstream_root: runtime_root.join("source/upstream"),
                managed_root: runtime_root.join("source/managed"),
                runtime_root: runtime_root.clone(),
            },
            data_root,
        )
    }

    #[test]
    fn cuda_environment_readiness_requires_bounded_profile_matched_marker() {
        let root = env::temp_dir().join(format!("meshharbor-cuda-marker-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let python = root.join("python");
        let lockfile = root.join("uv.lock");
        let entrypoint = root.join("pixalcuda/cli.py");
        let marker = root.join("cuda-preflight.json");
        std::fs::create_dir_all(entrypoint.parent().unwrap()).unwrap();
        std::fs::write(&python, "test interpreter").unwrap();
        std::fs::write(&lockfile, "test lock").unwrap();
        std::fs::write(&entrypoint, "test entrypoint").unwrap();

        assert!(!cuda_environment_is_ready(
            &python,
            &lockfile,
            &entrypoint,
            &marker
        ));
        std::fs::write(&marker, "{").unwrap();
        assert!(!cuda_environment_is_ready(
            &python,
            &lockfile,
            &entrypoint,
            &marker
        ));

        let valid = r#"{"cuda_available":true,"torch":"2.6.0+cu124","cuda_runtime":"12.4","sdpa_torch.float16":true,"sdpa_torch.float32":true}"#;
        let wrong_profile = valid.replace(r#""cuda_runtime":"12.4""#, r#""cuda_runtime":"12.5""#);
        std::fs::write(&marker, wrong_profile).unwrap();
        assert!(!cuda_environment_is_ready(
            &python,
            &lockfile,
            &entrypoint,
            &marker
        ));

        std::fs::write(&marker, valid).unwrap();
        assert!(cuda_environment_is_ready(
            &python,
            &lockfile,
            &entrypoint,
            &marker
        ));
        std::fs::write(&marker, vec![b' '; (MAX_CUDA_PREFLIGHT_BYTES + 1) as usize]).unwrap();
        assert!(!cuda_environment_is_ready(
            &python,
            &lockfile,
            &entrypoint,
            &marker
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn managed_cuda_setup_uses_app_private_tools_and_clears_credentials() {
        let root = env::temp_dir().join(format!("meshharbor-cuda-setup-{}", Uuid::new_v4()));
        let (manager, data_root) = isolated_manager(&root);
        let runtime_root = manager.runtime_root.clone();
        std::fs::write(data_root.join("owner-sentinel"), "preserve me").unwrap();
        let cached_model = manager.hf_home().join("hub/sentinel/model.safetensors");
        std::fs::create_dir_all(cached_model.parent().unwrap()).unwrap();
        std::fs::write(&cached_model, "preserve cached model").unwrap();
        manager.ensure_source().unwrap();

        let fake_python = root.join("fake-python");
        write_test_script(
            &fake_python,
            r#"#!/bin/sh
printf '%s|%s|%s|%s\n' "$HF_TOKEN" "$HUGGING_FACE_HUB_TOKEN" "$HOME" "$HF_HOME" >> "$HOME/python-env.log"
case "$1" in
  --version) echo "Python 3.12.9" ;;
  -m)
    if [ "$2" = "pixalcuda.cli" ] && [ "$3" = "doctor" ]; then
      if [ -f "$HOME/fail-doctor" ]; then
        echo '{"cuda_available":false,"torch":"2.6.0+cu124","cuda_runtime":"12.4","sdpa_torch.float16":true,"sdpa_torch.float32":true}'
      else
        echo '{"cuda_available":true,"torch":"2.6.0+cu124","cuda_runtime":"12.4","sdpa_torch.float16":true,"sdpa_torch.float32":true}'
      fi
    fi
    ;;
  -c) exit 0 ;;
  *) exit 0 ;;
esac
"#,
        );
        let fake_uv = root.join("fake-uv");
        let mut uv_script = String::from(
            r#"#!/bin/sh
printf '%s:%s|%s|%s|%s\n' "$1" "$2" "$HF_TOKEN" "$HUGGING_FACE_HUB_TOKEN" "$HOME" >> "$UV_CACHE_DIR/uv.log"
FAKE_PYTHON_PLACEHOLDER
case "$1:$2" in
  python:install)
    interpreter="$UV_PYTHON_INSTALL_DIR/cpython-3.12.9/bin/python"
    mkdir -p "$(dirname "$interpreter")"
    cp "$FAKE_PYTHON" "$interpreter"
    chmod +x "$interpreter"
    ;;
  python:find)
    echo "$UV_PYTHON_INSTALL_DIR/cpython-3.12.9/bin/python"
    ;;
  venv:*)
    destination=""
    for argument in "$@"; do destination="$argument"; done
    mkdir -p "$destination/bin"
    cp "$FAKE_PYTHON" "$destination/bin/python"
    chmod +x "$destination/bin/python"
    printf '%s\n' 'include-system-site-packages = false' > "$destination/pyvenv.cfg"
    ;;
  lock:*)
    : > uv.lock
    ;;
  sync:*)
    if [ -f "$HOME/fail-sync" ]; then exit 23; fi
    ;;
  *)
    echo "unexpected fake uv arguments: $*" >&2
    exit 81
    ;;
esac
"#,
        );
        let fake_python_assignment = format!(
            "FAKE_PYTHON='{}'",
            fake_python.to_string_lossy().replace('\'', "'\\''")
        );
        uv_script = uv_script.replace("FAKE_PYTHON_PLACEHOLDER", &fake_python_assignment);
        write_test_script(&fake_uv, &uv_script);

        let report = manager
            .setup_environment_with_tools(&fake_uv, "fake NVIDIA GPU", Default::default())
            .unwrap();
        assert_eq!(report.backend_id, "pixal3d-cuda");
        assert_eq!(
            report.status,
            "python_environment_ready_models_not_installed"
        );
        assert!(report.runtime_root.starts_with(&data_root));
        assert!(report.python.starts_with(&runtime_root));
        assert!(report.lockfile.is_file());
        let preflight_marker = runtime_root.join("cuda-preflight.json");
        let environment_ready = || {
            cuda_environment_is_ready(
                &manager.python_path(),
                &manager.managed_root.join("uv.lock"),
                &manager.upstream_root.join("pixalcuda/cli.py"),
                &preflight_marker,
            )
        };
        assert!(read_valid_cuda_preflight_marker(&preflight_marker));
        assert!(environment_ready());
        assert!(!runtime_root.join("weights").exists());

        let fail_sync = manager.home_path().join("fail-sync");
        std::fs::write(&fail_sync, "fail this retry").unwrap();
        assert!(
            manager
                .setup_environment_with_tools(&fake_uv, "fake NVIDIA GPU", Default::default())
                .is_err()
        );
        assert!(!preflight_marker.exists());
        assert!(!environment_ready());
        std::fs::remove_file(&fail_sync).unwrap();

        let fail_doctor = manager.home_path().join("fail-doctor");
        std::fs::write(&fail_doctor, "fail the CUDA doctor").unwrap();
        assert!(
            manager
                .setup_environment_with_tools(&fake_uv, "fake NVIDIA GPU", Default::default())
                .is_err()
        );
        assert!(!preflight_marker.exists());
        assert!(!environment_ready());
        std::fs::remove_file(&fail_doctor).unwrap();

        let retry = manager
            .setup_environment_with_tools(&fake_uv, "fake NVIDIA GPU", Default::default())
            .unwrap();
        assert_eq!(
            retry.status,
            "python_environment_ready_models_not_installed"
        );
        assert!(read_valid_cuda_preflight_marker(&preflight_marker));
        assert!(environment_ready());
        assert_eq!(
            std::fs::read_to_string(cached_model).unwrap(),
            "preserve cached model"
        );
        assert!(std::fs::read_dir(&runtime_root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".cuda-preflight-")
        }));

        let uv_log = std::fs::read_to_string(manager.cache_path().join("uv.log")).unwrap();
        assert!(uv_log.lines().all(|line| {
            let fields: Vec<_> = line.split('|').collect();
            fields.len() == 4
                && fields[1].is_empty()
                && fields[2].is_empty()
                && fields[3].starts_with(&manager.home_path().to_string_lossy().to_string())
        }));
        let python_log =
            std::fs::read_to_string(manager.home_path().join("python-env.log")).unwrap();
        assert!(python_log.lines().all(|line| {
            let fields: Vec<_> = line.split('|').collect();
            fields.len() == 4
                && fields[0].is_empty()
                && fields[1].is_empty()
                && fields[2].starts_with(&manager.home_path().to_string_lossy().to_string())
                && fields[3].starts_with(&manager.hf_home().to_string_lossy().to_string())
        }));
        assert_eq!(
            std::fs::read_to_string(data_root.join("owner-sentinel")).unwrap(),
            "preserve me"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn model_download_retry_keeps_hf_partial_then_completes_pinned_cache_files() {
        let root = env::temp_dir().join(format!("meshharbor-cuda-download-{}", Uuid::new_v4()));
        let (manager, data_root) = isolated_manager(&root);
        let runtime_root = manager.runtime_root.clone();
        std::fs::write(data_root.join("owner-sentinel"), "preserve me").unwrap();
        manager.ensure_source().unwrap();

        let fake_python = root.join("fake-python");
        write_test_script(
            &fake_python,
            r#"#!/bin/sh
printf '%s|%s|%s|%s\n' "$HF_TOKEN" "$HUGGING_FACE_HUB_TOKEN" "$HOME" "$HF_HOME" >> "$HOME/python-env.log"
if [ "$1" = "-I" ]; then echo verified; exit 0; fi
[ "$1" = "-c" ] && [ "$2" = "from pixalcuda.weights import ModelStore; ModelStore().naf()" ] || exit 1
mkdir -p "$HF_HOME/hub/pixal3d-cuda"
printf 'fixture' > "$HF_HOME/hub/pixal3d-cuda/naf_release.pth"
"#,
        );
        let fake_hf = runtime_root.join(".venv/bin/hf");
        std::fs::create_dir_all(fake_hf.parent().unwrap()).unwrap();
        let hf_script = r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/hf.log"
[ -z "${HF_TOKEN+x}${HUGGING_FACE_HUB_TOKEN+x}${HF_HUB_CACHE+x}" ] || exit 1
[ "$HF_HOME" = "$HOME/.cache/huggingface" ] || exit 1
if [ "$2" = "facebook/dinov3-vitl16-pretrain-lvd1689m" ]; then
  DINO_PAYLOAD
  exit 0
fi
[ "$2" = "TencentARC/Pixal3D" ] || exit 1
partial="$HF_HOME/hub/.cache/download/partial"
if [ ! -f "$partial" ]; then
  mkdir -p "${partial%/*}"
  printf 'partial checkpoint bytes' > "$partial"
  exit 23
fi
PIXAL_PAYLOAD
"#;
        let mut payload = String::new();
        for filename in PIXAL_CHECKPOINTS {
            payload.push_str(&format!(
                "mkdir -p \"$HF_HOME/hub/{PIXAL_REPOSITORY}/snapshots/{PIXAL_REVISION}/ckpts\"\nprintf 'fixture' > \"$HF_HOME/hub/{PIXAL_REPOSITORY}/snapshots/{PIXAL_REVISION}/ckpts/{filename}\"\n"
            ));
        }
        payload.push_str(&format!(
            "mkdir -p \"$HF_HOME/hub/{PIXAL_REPOSITORY}/snapshots/{PIXAL_REVISION}\"\nprintf 'fixture' > \"$HF_HOME/hub/{PIXAL_REPOSITORY}/snapshots/{PIXAL_REVISION}/pipeline.json\"\n"
        ));
        let dino_payload = format!(
            "mkdir -p \"$HF_HOME/hub/{DINO_REPOSITORY}/snapshots/{DINO_REVISION}\"\nprintf 'fixture' > \"$HF_HOME/hub/{DINO_REPOSITORY}/snapshots/{DINO_REVISION}/model.safetensors\"\n"
        );
        write_test_script(
            &fake_hf,
            &hf_script
                .replace("DINO_PAYLOAD", &dino_payload)
                .replace("PIXAL_PAYLOAD", &payload),
        );

        let partial = manager.hf_home().join("hub/.cache/download/partial");
        assert!(
            manager
                .install_models_with_python(&fake_python, None, Default::default())
                .is_err()
        );
        assert!(
            !manager
                .hf_home()
                .join("hub/pixal3d-cuda/naf_release.pth")
                .exists()
        );
        assert_eq!(
            std::fs::read_to_string(&partial).unwrap(),
            "partial checkpoint bytes"
        );

        let report = manager
            .install_models_with_python(&fake_python, None, Default::default())
            .unwrap();
        assert_eq!(report.status, "model_files_ready");
        assert!(manager.readiness().models_downloaded);
        let dino = format!("download {DINO_REPO} --revision {DINO_REVISION} model.safetensors");
        let pixal = format!(
            "download {} --revision {PIXAL_REVISION} pipeline.json {} --max-workers 2",
            huggingface_download::PIXAL_REPO,
            PIXAL_CHECKPOINTS
                .iter()
                .map(|filename| format!("ckpts/{filename}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        let steps = std::fs::read_to_string(manager.home_path().join("hf.log")).unwrap();
        assert_eq!(
            steps.lines().collect::<Vec<_>>(),
            [dino.as_str(), pixal.as_str(), dino.as_str(), pixal.as_str()]
        );
        assert_eq!(
            std::fs::read_to_string(&partial).unwrap(),
            "partial checkpoint bytes"
        );
        let python_log =
            std::fs::read_to_string(manager.home_path().join("python-env.log")).unwrap();
        assert!(python_log.lines().all(|line| {
            let fields: Vec<_> = line.split('|').collect();
            fields.len() == 4
                && fields[0].is_empty()
                && fields[1].is_empty()
                && fields[2].starts_with(&manager.home_path().to_string_lossy().to_string())
                && fields[3].starts_with(&manager.hf_home().to_string_lossy().to_string())
        }));
        assert_eq!(
            std::fs::read_to_string(data_root.join("owner-sentinel")).unwrap(),
            "preserve me"
        );
        assert!(runtime_root.starts_with(&data_root));
        std::fs::remove_dir_all(root).unwrap();
    }
}
