//! Managed Linux/NVIDIA runtime for the vendored Pixal3D CUDA backend.
//!
//! Setup only installs source dependencies into an app-private uv environment.
//! Model files are a separate, explicitly-confirmed action. Hugging Face login
//! and gated DINOv3 access remain under the account owner's control.

use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::Value;

use crate::{
    CoreError, CoreResult, generation::RuntimeReadiness, runtime::RuntimeSetupReport,
    uv_bootstrap::ensure_uv,
};

const PYTHON_REQUEST: &str = "3.12";
const PIXAL_REVISION: &str = "0b31f9160aa400719af409098bff7936a932f726";
const DINO_REVISION: &str = "ea8dc2863c51be0a264bab82070e3e8836b02d51";
const DINO_REPOSITORY: &str = "models--facebook--dinov3-vitl16-pretrain-lvd1689m";
const PIXAL_REPOSITORY: &str = "models--TencentARC--Pixal3D";
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

#[derive(Debug, Clone)]
pub struct Pixal3dCudaRuntimeManager {
    runtime_root: PathBuf,
    upstream_root: PathBuf,
    managed_root: PathBuf,
}

impl Pixal3dCudaRuntimeManager {
    pub fn new(data_root: impl AsRef<Path>) -> Self {
        let runtime_root = env::var_os("MESHHARBOR_PIXAL3D_CUDA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_root.as_ref().join("runtimes/pixal3d-cuda"));
        let source_root = runtime_root.join("source");
        Self {
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
        self.ensure_supported_host()?;
        self.ensure_source()?;
        let device = self.detect_gpu()?;
        let uv = ensure_uv(&self.runtime_root)?;
        let python_dir = self.runtime_root.join("python");
        let venv_dir = self.runtime_root.join(".venv");
        let home = self.home_path();
        std::fs::create_dir_all(&python_dir)?;
        std::fs::create_dir_all(&venv_dir)?;
        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(self.cache_path())?;

        eprintln!("NVIDIA GPU detected: {device}");
        eprintln!("[1/4] Installing isolated managed Python {PYTHON_REQUEST}…");
        let mut install = self.uv_command(&uv);
        install
            .args(["python", "install", PYTHON_REQUEST, "--install-dir"])
            .arg(&python_dir);
        run_visible(install, "uv could not install managed Python 3.12")?;

        let managed_python = self.find_managed_python(&uv)?;
        let version = run_capture(
            Command::new(&managed_python).arg("--version"),
            "Could not verify managed Python",
        )?;
        if !version.starts_with("Python 3.12.") {
            return Err(CoreError::OperationUnavailable(format!(
                "uv returned unexpected interpreter {version}; the CUDA profile requires Python 3.12."
            )));
        }

        eprintln!("[2/4] Creating an isolated CUDA Python environment…");
        let mut venv = self.uv_command(&uv);
        venv.args(["venv", "--no-project", "--allow-existing", "--python"])
            .arg(&managed_python)
            .arg(&venv_dir);
        run_visible(venv, "uv could not create the isolated CUDA environment")?;

        let python = self.python_path();
        let lockfile = self.managed_root.join("uv.lock");
        if !lockfile.is_file() {
            eprintln!(
                "[3/4] Resolving the vendored dependency ranges into a private per-install uv.lock…"
            );
            let mut lock = self.uv_command(&uv);
            lock.current_dir(&self.managed_root)
                .arg("lock")
                .arg("--python")
                .arg(&python);
            run_visible(lock, "uv could not resolve the CUDA dependency lock")?;
        } else {
            eprintln!("[3/4] Reusing the existing managed CUDA uv.lock…");
        }

        eprintln!("[4/4] Installing the pinned Python 3.12 / PyTorch 2.6.0 + CUDA 12.4 profile…");
        let mut sync = self.uv_command(&uv);
        sync.current_dir(&self.managed_root)
            .env("UV_PROJECT_ENVIRONMENT", &venv_dir)
            .args(["sync", "--project"])
            .arg(&self.managed_root)
            .arg("--python")
            .arg(&python)
            .args(["--locked", "--no-dev", "--no-editable"]);
        run_visible(sync, "uv could not install the locked CUDA dependencies")?;

        // Upstream's doctor emits non-sensitive device/runtime diagnostics and
        // executes its FP16/FP32 SDPA finite-value smoke test. Reject CPU-only
        // or incompatible driver environments before making the backend ready.
        let report = self.verify_cuda_environment(&python)?;
        let marker = self.runtime_root.join("cuda-preflight.json");
        std::fs::write(&marker, serde_json::to_vec_pretty(&report)?)?;

        Ok(RuntimeSetupReport {
            backend_id: "pixal3d-cuda".into(),
            status: "python_environment_ready_models_not_installed".into(),
            runtime_root: self.runtime_root.clone(),
            python,
            lockfile,
            message: "The isolated Python environment and CUDA preflight succeeded. Model weights remain a separate, explicitly confirmed 25–30 GB download. Upstream has no committed lock; this install resolved one locally. Gated DINOv3 access and upstream model terms remain separate from MeshHarbor's MIT license.".into(),
        })
    }

    /// Sign in through Hugging Face's own terminal prompt inside the isolated
    /// MeshHarbor cache; no token enters MeshHarbor's API or logs.
    pub fn login_huggingface(&self) -> CoreResult<()> {
        self.ensure_supported_host()?;
        let python = self.python_path();
        if !python.is_file() {
            return Err(CoreError::OperationUnavailable(format!(
                "Managed CUDA environment missing at {}; run `meshharbor runtime cuda-setup` first.",
                python.display()
            )));
        }
        let executable = self.runtime_root.join(".venv/bin/hf");
        if !executable.is_file() {
            return Err(CoreError::OperationUnavailable(
                "The isolated Hugging Face CLI is missing; rerun CUDA setup.".into(),
            ));
        }
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
        run_visible(
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
        if !confirm_large_download || !confirm_gated_access {
            return Err(CoreError::InvalidInput(
                "CUDA model installation requires explicit confirmation of the 25–30 GB download and prior DINOv3 gated access.".into(),
            ));
        }
        self.ensure_supported_host()?;
        self.ensure_source()?;
        let python = self.python_path();
        if !python.is_file() {
            return Err(CoreError::OperationUnavailable(
                "Run `meshharbor runtime cuda-setup` before installing CUDA models.".into(),
            ));
        }
        let home = self.home_path();
        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(self.hf_home())?;

        // Query model metadata only. This verifies that the user's private HF
        // login can access the gated revision before the large checkpoint set
        // is fetched. It does not accept terms or download any weights.
        eprintln!("[1/2] Checking the user's existing DINOv3 access before large model downloads…");
        let access_check = format!(
            "from huggingface_hub import HfApi; HfApi().model_info(repo_id='facebook/dinov3-vitl16-pretrain-lvd1689m', revision='{DINO_REVISION}'); print('DINOv3 gated access verified')"
        );
        let mut preflight = self.python_command(&python);
        preflight
            .args(["-c", access_check.as_str()])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        run_visible(
            preflight,
            "DINOv3 access check failed; accept the model terms in Hugging Face and run `meshharbor runtime cuda-hf-login` first",
        )?;

        eprintln!(
            "[2/2] Downloading pinned Pixal3D, DINOv3 and NAF checkpoints (roughly 25–30 GB)…"
        );
        let mut download = self.python_command(&python);
        download
            .current_dir(&self.upstream_root)
            .args(["-m", "pixalcuda.cli", "download", "--skip-camera"])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        run_visible(
            download,
            "CUDA model download failed; retrying is safe and uses the isolated HF cache",
        )?;

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
            python,
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
        let environment_ready = self.python_path().is_file()
            && self.managed_root.join("uv.lock").is_file()
            && self.upstream_root.join("pixalcuda/cli.py").is_file()
            && self.runtime_root.join("cuda-preflight.json").is_file();
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

    pub fn generate(
        &self,
        input: &Path,
        output_dir: &Path,
        options: &crate::GenerationOptions,
    ) -> CoreResult<u128> {
        let readiness = self.readiness();
        if !readiness.ready {
            return Err(CoreError::OperationUnavailable(format!(
                "Pixal3D CUDA runtime is not ready: {}. Setup and model downloads are explicit; no weights are fetched during generation.",
                readiness.missing.join("; ")
            )));
        }
        validate_generation_options(options)?;
        let python = self.python_path();
        self.verify_cuda_environment(&python)?;
        std::fs::create_dir_all(output_dir)?;
        let snapshot = self
            .hf_home()
            .join("hub")
            .join(PIXAL_REPOSITORY)
            .join("snapshots")
            .join(PIXAL_REVISION);
        let started = std::time::Instant::now();
        let mut command = self.python_command(&python);
        command
            .current_dir(&self.upstream_root)
            // No ambient account token or user Python modules are exposed to
            // the model worker. The already-installed local files are enough.
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .arg("-m")
            .arg("pixalcuda.cli")
            .arg("generate")
            .arg("--device")
            .arg("cuda")
            .arg("--image")
            .arg(input)
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
            .args(["--camera", "fixed", "--fov-degrees", "49.13", "--no-rembg"])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        run_visible(command, "Pixal3D CUDA inference failed")?;

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
        if !is_supported_target(env::consts::OS, env::consts::ARCH) {
            return Err(CoreError::OperationUnavailable(
                "CUDA is available only on Linux x86_64 in this managed profile.".into(),
            ));
        }
        let output = Command::new("nvidia-smi")
            .args(["--query-gpu=name", "--format=csv,noheader"])
            .output()
            .map_err(|error| {
                CoreError::OperationUnavailable(format!(
                    "NVIDIA driver utility nvidia-smi is unavailable: {error}"
                ))
            })?;
        if !output.status.success() {
            return Err(CoreError::OperationUnavailable(format!(
                "nvidia-smi could not query an NVIDIA GPU: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
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

    fn verify_cuda_environment(&self, python: &Path) -> CoreResult<Value> {
        let mut command = self.python_command(python);
        command.args(["-m", "pixalcuda.cli", "doctor"]);
        let output = run_capture(&mut command, "PyTorch CUDA preflight failed")?;
        let report: Value = serde_json::from_str(&output).map_err(|error| {
            CoreError::OperationUnavailable(format!(
                "Pixal3D CUDA doctor returned invalid JSON: {error}"
            ))
        })?;
        let torch = report.get("torch").and_then(Value::as_str).unwrap_or("");
        let cuda_runtime = report
            .get("cuda_runtime")
            .and_then(Value::as_str)
            .unwrap_or("");
        let sdpa_fp16 = report.get("sdpa_torch.float16").and_then(Value::as_bool) == Some(true);
        let sdpa_fp32 = report.get("sdpa_torch.float32").and_then(Value::as_bool) == Some(true);
        if report.get("cuda_available").and_then(Value::as_bool) != Some(true)
            || !torch.starts_with("2.6.0")
            || !cuda_runtime.starts_with("12.4")
            || !sdpa_fp16
            || !sdpa_fp32
        {
            return Err(CoreError::OperationUnavailable(format!(
                "PyTorch/CUDA preflight did not meet the reviewed profile (requires torch 2.6.0, CUDA 12.4, CUDA available, and finite FP16/FP32 SDPA): {}",
                output.trim()
            )));
        }
        let versions = "import torch, torchvision; assert torch.__version__.startswith('2.6.0'); assert torchvision.__version__.startswith('0.21.0')";
        let mut verify_packages = self.python_command(python);
        verify_packages.args(["-c", versions]);
        run_visible(
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
            .env("UV_PYTHON_DOWNLOADS", "automatic")
            .env("UV_DEFAULT_INDEX", "https://pypi.org/simple");
        command
    }

    fn find_managed_python(&self, uv: &Path) -> CoreResult<PathBuf> {
        let output = self
            .uv_command(uv)
            .args([
                "python",
                "find",
                "--no-project",
                "--managed-python",
                PYTHON_REQUEST,
            ])
            .output()?;
        if !output.status.success() {
            return Err(CoreError::OperationUnavailable(format!(
                "uv could not locate its managed Python {PYTHON_REQUEST}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if path.is_empty() {
            return Err(CoreError::OperationUnavailable(
                "uv returned an empty path for its managed Python.".into(),
            ));
        }
        Ok(PathBuf::from(path))
    }
}

fn validate_generation_options(options: &crate::GenerationOptions) -> CoreResult<()> {
    if options.resolution != 512 {
        return Err(CoreError::InvalidInput(
            "Pixal3D CUDA exposes only the reviewed 512 geometry profile.".into(),
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
    Ok(())
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

fn run_visible(mut command: Command, failure_prefix: &str) -> CoreResult<()> {
    let status = command
        .status()
        .map_err(|error| CoreError::OperationUnavailable(format!("{failure_prefix}: {error}")))?;
    if !status.success() {
        return Err(CoreError::OperationUnavailable(format!(
            "{failure_prefix} (exit status {}). See the command output above.",
            status
                .code()
                .map_or_else(|| "signal".into(), |code| code.to_string())
        )));
    }
    Ok(())
}

fn run_capture(command: &mut Command, failure_prefix: &str) -> CoreResult<String> {
    let output = command
        .output()
        .map_err(|error| CoreError::OperationUnavailable(format!("{failure_prefix}: {error}")))?;
    if !output.status.success() {
        return Err(CoreError::OperationUnavailable(format!(
            "{failure_prefix}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
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
}
