//! Managed setup for the optional Pixal3D MLX runtime.
//!
//! This installer only provisions an isolated uv-managed Python environment
//! and explicitly requested model files. Gated Hugging Face terms and login
//! remain under the account owner's control.

use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CoreError, CoreResult, HuggingFaceAccess, HuggingFaceConnection,
    huggingface_access::{DINO_REVISION, check_connection, check_model_access, require_access},
    huggingface_browser::access_python,
    huggingface_download::{self, DINO_REPO},
    job_execution::JobExecution,
};

const PYTHON_REQUEST: &str = "3.12";
const NAF_SHA256: &str = "c096c1ab2217a5c3ac136365f721685e2201379cb69d509cfb0261183847c98f";
const CONVERTER_PACKAGES: &[&str] = &[
    "torch==2.5.1",
    "safetensors==0.7.0",
    "numpy==1.26.4",
    "packaging==26.2",
];

#[derive(Clone, Copy)]
pub enum RuntimeJobKind {
    Setup,
    InstallModels,
    ConvertNaf,
    CudaSetup,
    CudaInstallModels,
    CudaInstallMultiview,
    InstallCatalogModel(&'static str),
    GeoSam2Backend(&'static str),
    GeoSam2Weights(&'static str),
}

impl RuntimeJobKind {
    pub fn operation(self) -> &'static str {
        match self {
            Self::Setup => "runtime.pixal3d.setup",
            Self::InstallModels => "runtime.pixal3d.install_models",
            Self::ConvertNaf => "runtime.pixal3d.convert_naf",
            Self::CudaSetup => "runtime.pixal3d.cuda_setup",
            Self::CudaInstallModels => "runtime.pixal3d.cuda_install_models",
            Self::CudaInstallMultiview => "runtime.pixal3d.cuda_install_multiview",
            Self::InstallCatalogModel(_) => "runtime.models.install",
            Self::GeoSam2Backend(_) => "runtime.geosam2.install_backend",
            Self::GeoSam2Weights(_) => "runtime.geosam2.download_weights",
        }
    }

    pub fn backend_id(self) -> &'static str {
        match self {
            Self::Setup | Self::InstallModels | Self::ConvertNaf => "pixal3d-mlx",
            Self::CudaSetup | Self::CudaInstallModels | Self::CudaInstallMultiview => {
                "pixal3d-cuda"
            }
            Self::InstallCatalogModel(backend)
            | Self::GeoSam2Backend(backend)
            | Self::GeoSam2Weights(backend) => backend,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSetupReport {
    pub backend_id: String,
    pub status: String,
    pub runtime_root: PathBuf,
    pub python: PathBuf,
    pub lockfile: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Pixal3dRuntimeManager {
    source_root: PathBuf,
    runtime_root: PathBuf,
}

impl Pixal3dRuntimeManager {
    pub fn new(data_root: impl AsRef<Path>) -> Self {
        let data_root = data_root.as_ref().to_path_buf();
        let runtime_root = env::var_os("MESHHARBOR_PIXAL3D_MLX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_root.join("runtimes/pixal3d-mlx"));
        let source_root = env::var_os("MESHHARBOR_PIXAL3D_MLX_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| runtime_root.join("source/upstream"));
        Self {
            source_root,
            runtime_root,
        }
    }

    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    /// Materialize the first-party source snapshot embedded in the binary
    /// beside the user-managed runtime. This makes installed binaries
    /// independent of the original checkout's `CARGO_MANIFEST_DIR`.
    pub fn ensure_source(&self) -> CoreResult<()> {
        materialize_embedded_source(
            &self.runtime_root,
            env::var_os("MESHHARBOR_PIXAL3D_MLX_ROOT").is_none(),
        )
    }

    pub fn python_path(&self) -> PathBuf {
        if let Some(path) = env::var_os("MESHHARBOR_PIXAL3D_MLX_PYTHON") {
            return PathBuf::from(path);
        }
        if cfg!(windows) {
            self.runtime_root.join(".venv/Scripts/python.exe")
        } else {
            self.runtime_root.join(".venv/bin/python")
        }
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
        self.ensure_apple_silicon()?;
        let python = access_python(&self.runtime_root).unwrap_or_else(|| self.python_path());
        if !python.is_file() {
            return Err(CoreError::OperationUnavailable(
                "Prepare browser sign-in or set up the MLX environment before connecting a token."
                    .into(),
            ));
        }
        Ok(self.python_command(&python))
    }

    /// Provision a managed Python 3.12 and the source-pinned, locked MLX
    /// dependencies. If needed, `uv` is bootstrapped from a pinned official
    /// release after the user explicitly starts runtime setup.
    pub fn setup_environment(&self) -> CoreResult<RuntimeSetupReport> {
        self.setup_environment_tracked(JobExecution::default())
    }

    pub(crate) fn setup_environment_tracked(
        &self,
        execution: JobExecution<'_>,
    ) -> CoreResult<RuntimeSetupReport> {
        execution.stage("prepare_runtime")?;
        self.ensure_apple_silicon()?;
        self.ensure_source()?;
        self.validate_source()?;
        let uv = crate::uv_bootstrap::ensure_uv_tracked(&self.runtime_root, execution)?;
        self.setup_environment_with_uv(&uv, execution)
    }

    fn setup_environment_with_uv(
        &self,
        uv: &Path,
        execution: JobExecution<'_>,
    ) -> CoreResult<RuntimeSetupReport> {
        let python_dir = self.runtime_root.join("python");
        let cache_dir = self.runtime_root.join("uv-cache");
        let home_dir = self.home_path();
        let venv_dir = self.runtime_root.join(".venv");
        std::fs::create_dir_all(&python_dir)?;
        std::fs::create_dir_all(&cache_dir)?;
        std::fs::create_dir_all(&home_dir)?;

        let venv_python = prepare_managed_venv(
            execution,
            &python_dir,
            &venv_dir,
            || self.uv_command(uv, &home_dir, &cache_dir),
            |path| self.python_command(path),
        )?;

        execution.stage("sync_dependencies")?;
        let mut sync = self.uv_command(uv, &home_dir, &cache_dir);
        sync.env("UV_PROJECT_ENVIRONMENT", &venv_dir)
            .arg("sync")
            .arg("--project")
            .arg(&self.source_root)
            .arg("--python")
            .arg(&venv_python)
            .args(["--locked", "--no-dev", "--no-editable"]);
        execution.visible(sync, "uv could not install the pinned MLX dependencies")?;

        execution.stage("verify_environment")?;
        let mut version_check = self.python_command(&venv_python);
        version_check.arg("--version");
        let verified_version = execution.capture(
            &mut version_check,
            "Could not verify the created MLX environment",
        )?;
        if !verified_version.starts_with("Python 3.12.") {
            return Err(CoreError::OperationUnavailable(format!(
                "The managed environment uses unexpected interpreter {verified_version}."
            )));
        }
        let mut import_check = self.python_command(&venv_python);
        import_check.args([
            "-c",
            "import mlx, huggingface_hub, safetensors; print('MLX runtime imports are available')",
        ]);
        execution.capture(
            &mut import_check,
            "The locked Python environment was created, but MLX imports failed",
        )?;
        Ok(RuntimeSetupReport {
            backend_id: "pixal3d-mlx".into(),
            status: "python_environment_ready_models_not_installed".into(),
            runtime_root: self.runtime_root.clone(),
            python: venv_python,
            lockfile: self.source_root.join("uv.lock"),
            message: "Pinned Python dependencies are installed. Model weights remain a separate, explicitly requested download; gated DINOv3 access must be approved and authenticated by the user first.".into(),
        })
    }

    /// Download the pinned model files after the user has accepted any gated
    /// DINOv3 terms in Hugging Face and signed in using this isolated HOME.
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
                "MLX model installation requires explicit confirmation of the large download and prior DINOv3 gated access.".into(),
            ));
        }
        self.ensure_apple_silicon()?;
        self.ensure_source()?;
        self.validate_source()?;
        let python = self.python_path();
        if !python.is_file() {
            return Err(CoreError::OperationUnavailable(format!(
                "Managed Python environment not found at {}. Run `meshharbor runtime setup` first.",
                python.display()
            )));
        }
        let uv = crate::uv_bootstrap::ensure_uv_tracked(&self.runtime_root, execution)?;
        self.install_models_with_tools(
            &uv,
            &python,
            huggingface_download::shared_hub_cache().as_deref(),
            execution,
        )
    }

    fn install_models_with_tools(
        &self,
        uv: &Path,
        python: &Path,
        shared_cache: Option<&Path>,
        execution: JobExecution<'_>,
    ) -> CoreResult<RuntimeSetupReport> {
        let home = self.home_path();
        std::fs::create_dir_all(&home)?;
        let weights = self.runtime_root.join("weights");
        std::fs::create_dir_all(&weights)?;
        let scripts = self.managed_scripts_root();
        execution.stage("check_model_access")?;
        let access_python =
            access_python(&self.runtime_root).unwrap_or_else(|| python.to_path_buf());
        require_access(
            self.python_command(&access_python),
            "meshharbor runtime hf-login",
        )?;

        execution.stage("download_weights")?;
        huggingface_download::download(
            &self.runtime_root,
            &home,
            DINO_REPO,
            DINO_REVISION,
            ["model.safetensors", "config.json", "--max-workers", "1"],
            execution,
        )?;
        huggingface_download::download_pixal3d(
            &self.runtime_root,
            &home,
            shared_cache,
            true,
            execution,
        )?;
        execution.stage("verify_weights")?;
        let mut verify = self.python_command(python);
        verify
            .current_dir(&self.source_root)
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .arg(scripts.join("download_models.py"))
            .arg("--source-root")
            .arg(&self.source_root)
            .arg("--weights-dir")
            .arg(&weights)
            .stdout(Stdio::from(std::io::stderr()))
            .stderr(Stdio::inherit());
        execution.visible(
            verify,
            "Could not verify the pinned Hugging Face snapshots or prepare the NAF checkpoint",
        )?;
        self.convert_naf_checkpoint(
            uv,
            &home,
            &self.runtime_root.join("uv-cache"),
            &weights,
            execution,
        )?;
        Ok(RuntimeSetupReport {
            backend_id: "pixal3d-mlx".into(),
            status: "model_files_ready".into(),
            runtime_root: self.runtime_root.clone(),
            python: python.to_path_buf(),
            lockfile: self.source_root.join("uv.lock"),
            message: "Pinned Pixal3D and gated DINOv3 snapshots are ready; Pixal3D/NAF hashes were verified, and NAF was converted in an isolated CPU-only environment.".into(),
        })
    }

    /// Retry the offline NAF conversion after model files are already present.
    /// Only PyPI conversion packages are downloaded; no HF login is used.
    pub fn convert_naf(&self) -> CoreResult<RuntimeSetupReport> {
        self.convert_naf_tracked(JobExecution::default())
    }

    pub(crate) fn convert_naf_tracked(
        &self,
        execution: JobExecution<'_>,
    ) -> CoreResult<RuntimeSetupReport> {
        execution.stage("prepare_runtime")?;
        self.ensure_apple_silicon()?;
        self.ensure_source()?;
        self.validate_source()?;
        let python = self.python_path();
        if !python.is_file() {
            return Err(CoreError::OperationUnavailable(format!(
                "Managed Python environment not found at {}. Run `meshharbor runtime setup` first.",
                python.display()
            )));
        }
        let uv = crate::uv_bootstrap::ensure_uv_tracked(&self.runtime_root, execution)?;
        let home = self.home_path();
        std::fs::create_dir_all(&home)?;
        let cache = self.runtime_root.join("uv-cache");
        std::fs::create_dir_all(&cache)?;
        let weights = self.runtime_root.join("weights");
        self.convert_naf_checkpoint(&uv, &home, &cache, &weights, execution)?;
        Ok(RuntimeSetupReport {
            backend_id: "pixal3d-mlx".into(),
            status: "model_files_ready".into(),
            runtime_root: self.runtime_root.clone(),
            python,
            lockfile: self.source_root.join("uv.lock"),
            message:
                "The verified NAF checkpoint was converted in an isolated CPU-only environment."
                    .into(),
        })
    }

    /// Let the user authenticate to Hugging Face in the dedicated MeshHarbor
    /// cache. The token is read by the official interactive CLI and never
    /// enters MeshHarbor's API, logs, or the user's normal HF cache.
    pub fn login_huggingface(&self) -> CoreResult<()> {
        self.ensure_apple_silicon()?;
        let python = self.python_path();
        if !python.is_file() {
            return Err(CoreError::OperationUnavailable(format!(
                "Managed Python environment not found at {}. Run `meshharbor runtime setup` first.",
                python.display()
            )));
        }
        let executable = if cfg!(windows) {
            self.runtime_root.join(".venv/Scripts/hf.exe")
        } else {
            self.runtime_root.join(".venv/bin/hf")
        };
        if !executable.is_file() {
            return Err(CoreError::OperationUnavailable(format!(
                "The isolated Hugging Face CLI is missing at {}. Rerun `meshharbor runtime setup`.",
                executable.display()
            )));
        }
        let home = self.home_path();
        std::fs::create_dir_all(&home)?;
        eprintln!(
            "Hugging Face will prompt for the token in this terminal. MeshHarbor will not read or store it outside the isolated HF cache."
        );
        let command = huggingface_login_command(&executable, &self.runtime_root, &home);
        JobExecution::default().visible(
            command,
            "Hugging Face login failed. The token was not saved by MeshHarbor.",
        )
    }

    fn convert_naf_checkpoint(
        &self,
        uv: &Path,
        home: &Path,
        cache: &Path,
        weights: &Path,
        execution: JobExecution<'_>,
    ) -> CoreResult<()> {
        execution.stage("verify_checkpoint")?;
        let source = weights.join("naf_release.pth");
        if !source.is_file() {
            return Err(CoreError::OperationUnavailable(format!(
                "Downloaded NAF checkpoint is missing: {}",
                source.display()
            )));
        }
        let source_hash = format!("{:x}", Sha256::digest(std::fs::read(&source)?));
        if source_hash != NAF_SHA256 {
            return Err(CoreError::OperationUnavailable(
                "Downloaded NAF checkpoint checksum mismatch; refusing to pass it to PyTorch."
                    .into(),
            ));
        }
        let converter_root = self.runtime_root.join("naf-converter");
        let converter_python = prepare_managed_venv(
            execution,
            &self.runtime_root.join("python"),
            &converter_root,
            || self.uv_command(uv, home, cache),
            |path| self.python_command(path),
        )?;

        execution.stage("sync_dependencies")?;
        let mut install = self.uv_command(uv, home, cache);
        install
            .arg("pip")
            .arg("install")
            .arg("--python")
            .arg(&converter_python)
            .args(CONVERTER_PACKAGES.iter().copied());
        execution.visible(
            install,
            "uv could not install the pinned NAF conversion tools",
        )?;

        execution.stage("convert_checkpoint")?;
        let mut convert = Command::new(converter_python);
        convert
            .env_clear()
            .env("HOME", home)
            .env("PATH", managed_path(&converter_root))
            .env("PYTHONNOUSERSITE", "1")
            .arg(
                self.managed_scripts_root()
                    .join("convert_naf_checkpoint.py"),
            )
            .arg("--weights-dir")
            .arg(weights)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        execution.visible(convert, "Could not convert the verified NAF checkpoint")
    }

    fn managed_scripts_root(&self) -> PathBuf {
        self.runtime_root.join("source/managed")
    }

    fn ensure_apple_silicon(&self) -> CoreResult<()> {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Ok(())
        } else {
            Err(CoreError::OperationUnavailable(
                "The Pixal3D MLX runtime supports Apple Silicon macOS only. Use the separate Linux x86_64/NVIDIA CUDA profile on compatible hosts.".into(),
            ))
        }
    }

    fn validate_source(&self) -> CoreResult<()> {
        for path in [
            self.source_root.join("pyproject.toml"),
            self.source_root.join("uv.lock"),
            self.source_root.join("scripts/download_models.py"),
            self.source_root.join("generate_pixal3d.py"),
            self.managed_scripts_root().join("download_models.py"),
            self.managed_scripts_root()
                .join("convert_naf_checkpoint.py"),
        ] {
            if !path.is_file() {
                return Err(CoreError::OperationUnavailable(format!(
                    "Pinned Pixal3D MLX source is incomplete; missing {}.",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    fn uv_command(&self, uv: &Path, home: &Path, cache: &Path) -> Command {
        let mut command = Command::new(uv);
        command
            .env_clear()
            .env("HOME", home)
            .env("PATH", managed_path(&self.runtime_root))
            .env("UV_PYTHON_INSTALL_DIR", self.runtime_root.join("python"))
            .env("UV_CACHE_DIR", cache)
            .env("UV_PROJECT_ENVIRONMENT", self.runtime_root.join(".venv"))
            .env("UV_NO_CONFIG", "1")
            .env("UV_NO_PROGRESS", "0")
            .env("UV_PYTHON_DOWNLOADS", "automatic")
            .env("UV_DEFAULT_INDEX", "https://pypi.org/simple");
        command
    }

    fn python_command(&self, python: &Path) -> Command {
        let home = self.home_path();
        let mut command = Command::new(python);
        command
            .env_clear()
            .env("HOME", &home)
            .env("HF_HOME", home.join(".cache/huggingface"))
            .env("PATH", managed_path(&self.runtime_root))
            .env("PYTHONNOUSERSITE", "1");
        for temporary_key in ["TMPDIR", "TEMP", "TMP"] {
            if let Some(value) = env::var_os(temporary_key) {
                command.env(temporary_key, value);
            }
        }
        command
    }
}

/// Populate a data-directory copy from the source snapshot linked into the
/// executable. If a developer explicitly overrides the upstream checkout,
/// only the reviewed MeshHarbor adapter scripts are materialized.
pub(crate) fn materialize_embedded_source(
    runtime_root: &Path,
    include_upstream: bool,
) -> CoreResult<()> {
    let source_root = runtime_root.join("source");
    for (relative, contents) in crate::runtime_bundle::PIXAL3D_MLX_FILES.iter().copied() {
        if relative.starts_with("upstream/") && !include_upstream {
            continue;
        }
        let target = source_root.join(relative);
        if target.is_file() && std::fs::read(&target)?.as_slice() == contents {
            continue;
        }
        let parent = target.parent().ok_or_else(|| {
            CoreError::OperationUnavailable("Embedded backend file has no parent directory.".into())
        })?;
        std::fs::create_dir_all(parent)?;
        std::fs::write(target, contents)?;
    }
    Ok(())
}

fn huggingface_login_command(executable: &Path, runtime_root: &Path, home: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .args(["auth", "login"])
        .env_clear()
        .env("HOME", home)
        .env("HF_HOME", home.join(".cache/huggingface"))
        .env("PATH", managed_path(runtime_root))
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("PYTHONNOUSERSITE", "1");
    command
}

fn managed_path(runtime_root: &Path) -> String {
    let mut paths = vec![
        runtime_root.join(".venv/bin"),
        runtime_root.join(".venv/Scripts"),
    ];
    if let Some(system_path) = env::var_os("PATH") {
        paths.extend(env::split_paths(&system_path));
    }
    env::join_paths(paths)
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn prepare_managed_venv(
    execution: JobExecution<'_>,
    python_dir: &Path,
    venv_dir: &Path,
    uv: impl Fn() -> Command,
    python_command: impl Fn(&Path) -> Command,
) -> CoreResult<PathBuf> {
    execution.stage("prepare_python")?;
    match std::fs::symlink_metadata(venv_dir) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(CoreError::OperationUnavailable(format!(
                "The private Python environment path is not a local directory: {}",
                venv_dir.display()
            )));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let python = venv_dir.join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    });
    let version = |path: &Path| {
        execution.capture(
            python_command(path).arg("--version"),
            "Could not verify Python",
        )
    };
    let isolated = std::fs::read_to_string(venv_dir.join("pyvenv.cfg")).is_ok_and(|config| {
        config
            .lines()
            .filter_map(|line| line.split_once('='))
            .any(|(key, value)| {
                key.trim() == "include-system-site-packages"
                    && value.trim().eq_ignore_ascii_case("false")
            })
    });
    if isolated
        && python.is_file()
        && version(&python).is_ok_and(|version| version.starts_with("Python 3.12."))
    {
        execution.note("Reusing the existing private Python 3.12 environment.")?;
        return Ok(python);
    }

    let find = || -> CoreResult<PathBuf> {
        let path = PathBuf::from(execution.capture(
            uv().args([
                "python",
                "find",
                "--no-project",
                "--managed-python",
                "--no-python-downloads",
                PYTHON_REQUEST,
            ]),
            "uv could not find a managed Python 3.12",
        )?);
        if !path.is_absolute() || !path.is_file() {
            return Err(CoreError::OperationUnavailable(
                "uv did not return a usable managed Python path.".into(),
            ));
        }
        Ok(path)
    };
    let base = if let Ok(base) = find() {
        execution.note("Reusing the installed managed Python interpreter.")?;
        base
    } else {
        execution.stage("install_python")?;
        let mut install = uv();
        install
            .args(["python", "install", PYTHON_REQUEST, "--install-dir"])
            .arg(python_dir);
        execution.visible(install, "uv could not install managed Python 3.12")?;
        find()?
    };
    if !version(&base)?.starts_with("Python 3.12.") {
        return Err(CoreError::OperationUnavailable(
            "The managed interpreter must use Python 3.12.".into(),
        ));
    }
    execution.stage("create_environment")?;
    let mut venv = uv();
    venv.args(["venv", "--no-project", "--allow-existing", "--python"])
        .arg(base)
        .arg(venv_dir);
    execution.visible(venv, "uv could not create the isolated Python environment")?;
    Ok(python)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    use uuid::Uuid;

    #[test]
    fn model_download_requires_explicit_large_download_and_gated_access_confirmation() {
        let root = env::temp_dir().join(format!("meshharbor-mlx-consent-{}", Uuid::new_v4()));
        let manager = Pixal3dRuntimeManager::new(&root);
        assert!(manager.install_models(false, false).is_err());
        assert!(manager.install_models(true, false).is_err());
        assert!(manager.install_models(false, true).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn huggingface_login_invokes_auth_login_in_isolated_home() {
        let root = env::temp_dir().join(format!("meshharbor-hf-login-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("fake-hf");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CAPTURE_DIR/args\"\nprintf '%s\\n' \"$HOME\" > \"$CAPTURE_DIR/home\"\nprintf '%s\\n' \"$HF_HOME\" > \"$CAPTURE_DIR/hf_home\"\nprintf '%s\\n' \"${HF_TOKEN+x}\" > \"$CAPTURE_DIR/hf_token_present\"\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();

        let runtime_root = root.join("runtime");
        let isolated_home = runtime_root.join("home");
        let mut command = huggingface_login_command(&executable, &runtime_root, &isolated_home);
        command.env("CAPTURE_DIR", &root);
        let status = command.status().unwrap();
        assert!(status.success());
        assert_eq!(
            fs::read_to_string(root.join("args")).unwrap(),
            "auth\nlogin\n"
        );
        let expected_home = isolated_home.to_string_lossy().into_owned();
        let expected_hf_home = isolated_home
            .join(".cache/huggingface")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            fs::read_to_string(root.join("home")).unwrap().trim(),
            expected_home.as_str()
        );
        assert_eq!(
            fs::read_to_string(root.join("hf_home")).unwrap().trim(),
            expected_hf_home.as_str()
        );
        assert_eq!(
            fs::read_to_string(root.join("hf_token_present")).unwrap(),
            "\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn embedded_source_is_materialized_under_runtime_data() {
        let root = env::temp_dir().join(format!("meshharbor-embedded-source-{}", Uuid::new_v4()));
        materialize_embedded_source(&root, true).unwrap();

        assert!(root.join("source/upstream/uv.lock").is_file());
        assert!(root.join("source/upstream/LICENSE").is_file());
        assert!(root.join("source/managed/download_models.py").is_file());
        assert!(
            root.join("source/managed/convert_naf_checkpoint.py")
                .is_file()
        );

        // Extraction is idempotent, as runtime setup may be retried.
        materialize_embedded_source(&root, true).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    fn write_test_script(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    #[test]
    fn managed_python_setup_retries_failed_sync_in_private_storage_without_credentials() {
        let root = env::temp_dir().join(format!("meshharbor-mlx-setup-retry-{}", Uuid::new_v4()));
        let data_root = root.join("app-data");
        let runtime_root = data_root.join("runtimes/pixal3d-mlx");
        fs::create_dir_all(&data_root).unwrap();
        fs::write(data_root.join("owner-sentinel"), "preserve me").unwrap();
        let manager = Pixal3dRuntimeManager {
            source_root: runtime_root.join("source/upstream"),
            runtime_root: runtime_root.clone(),
        };
        materialize_embedded_source(&runtime_root, true).unwrap();
        manager.validate_source().unwrap();

        let fake_python = root.join("fake-python");
        write_test_script(
            &fake_python,
            r#"#!/bin/sh
printf '%s|%s|%s|%s|%s\n' "$HF_TOKEN" "$HUGGING_FACE_HUB_TOKEN" "$HOME" "$HF_HOME" "$PYTHONNOUSERSITE" >> "$HOME/python-env.log"
case "$1" in
  --version) echo "Python 3.12.9" ;;
  -c) echo "fake imports are available" ;;
  *) exit 0 ;;
esac
"#,
        );
        let fake_uv = root.join("fake-uv");
        let mut uv_script = String::from(
            r#"#!/bin/sh
printf '%s:%s|%s|%s\n' "$1" "$2" "$HF_TOKEN" "$HUGGING_FACE_HUB_TOKEN" >> "$UV_CACHE_DIR/uv.log"
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
  sync:*)
    if [ ! -e "$UV_CACHE_DIR/sync-failed-once" ]; then
      : > "$UV_CACHE_DIR/sync-failed-once"
      echo 'fixture dependency failure' >&2
      exit 17
    fi
    ;;
  *)
    echo "unexpected fake uv arguments: $*" >&2
    exit 81
    ;;
esac
"#,
        );
        uv_script = uv_script.replace(
            "FAKE_PYTHON_PLACEHOLDER",
            &format!(
                "FAKE_PYTHON={}",
                shell_quote(&fake_python.to_string_lossy())
            ),
        );
        write_test_script(&fake_uv, &uv_script);

        let store = crate::Store::open(&data_root).unwrap();
        let start = || {
            store
                .create_running_job("runtime.pixal3d.setup", None, &serde_json::json!({}))
                .unwrap()
        };
        let first = start();
        assert!(
            manager
                .setup_environment_with_uv(&fake_uv, JobExecution::for_job(&store, &first.id))
                .is_err()
        );
        let failed_steps = store.job_steps(&first.id).unwrap();
        let failed_sync = failed_steps
            .iter()
            .find(|step| step.stage.as_deref() == Some("sync_dependencies"))
            .unwrap();
        assert!(
            failed_sync.commands[0]
                .argv
                .iter()
                .any(|arg| arg == "--locked")
        );
        assert_eq!(
            failed_sync.commands[0].stderr,
            "fixture dependency failure\n"
        );
        assert!(
            failed_sync.commands[0]
                .outcome
                .as_deref()
                .unwrap()
                .contains("17")
        );
        let retry = start();
        let report = manager
            .setup_environment_with_uv(&fake_uv, JobExecution::for_job(&store, &retry.id))
            .unwrap();
        let persisted = crate::Store::open(&data_root)
            .unwrap()
            .job_steps(&retry.id)
            .unwrap();
        assert!(persisted.iter().any(|step| {
            step.message
                .as_deref()
                .is_some_and(|text| text.contains("Reusing the existing private Python"))
        }));
        assert!(persisted.iter().any(|step| {
            step.commands
                .iter()
                .any(|command| command.stdout.contains("fake imports are available"))
        }));
        assert_eq!(report.backend_id, "pixal3d-mlx");
        assert_eq!(
            report.status,
            "python_environment_ready_models_not_installed"
        );
        assert!(report.runtime_root.starts_with(&data_root));
        assert!(report.python.starts_with(&runtime_root));
        assert!(report.lockfile.is_file());
        assert!(!runtime_root.join("weights").exists());

        let uv_log = fs::read_to_string(runtime_root.join("uv-cache/uv.log")).unwrap();
        assert_eq!(uv_log.matches("python:install").count(), 1);
        assert_eq!(uv_log.matches("venv:").count(), 1);
        assert_eq!(uv_log.matches("sync:--project").count(), 2);
        assert!(uv_log.lines().all(|line| line.contains("||")));
        let python_log = fs::read_to_string(manager.home_path().join("python-env.log")).unwrap();
        assert!(python_log.lines().all(|line| {
            let fields: Vec<_> = line.split('|').collect();
            fields.len() == 5
                && fields[0].is_empty()
                && fields[1].is_empty()
                && fields[2].starts_with(&manager.home_path().to_string_lossy().to_string())
                && fields[3].starts_with(
                    &manager
                        .home_path()
                        .join(".cache/huggingface")
                        .to_string_lossy()
                        .to_string(),
                )
                && fields[4] == "1"
        }));
        assert_eq!(
            fs::read_to_string(data_root.join("owner-sentinel")).unwrap(),
            "preserve me"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn model_download_retry_keeps_partial_cache_and_reaches_checkpoint_validation() {
        let root =
            env::temp_dir().join(format!("meshharbor-mlx-download-retry-{}", Uuid::new_v4()));
        let data_root = root.join("app-data");
        let runtime_root = data_root.join("runtimes/pixal3d-mlx");
        fs::create_dir_all(&data_root).unwrap();
        fs::write(data_root.join("owner-sentinel"), "preserve me").unwrap();
        let manager = Pixal3dRuntimeManager {
            source_root: runtime_root.join("source/upstream"),
            runtime_root: runtime_root.clone(),
        };
        materialize_embedded_source(&runtime_root, true).unwrap();
        manager.validate_source().unwrap();

        let fake_python = root.join("fake-python");
        write_test_script(
            &fake_python,
            r#"#!/bin/sh
printf '%s|%s|%s|%s|%s\n' "$HF_TOKEN" "$HUGGING_FACE_HUB_TOKEN" "$HOME" "$HF_HOME" "$PYTHONNOUSERSITE" >> "$HOME/python-env.log"
case "$1" in
  -I) echo "verified"; exit 0 ;;
esac
printf '%s\n' "verifying" >> "$HOME/download-steps"
weights=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--weights-dir" ]; then weights="$2"; shift 2; else shift; fi
done
mkdir -p "$weights"
printf 'invalid fixture bytes' > "$weights/naf_release.pth"
exit 0
"#,
        );
        let fake_hf = runtime_root.join(".venv/bin/hf");
        fs::create_dir_all(fake_hf.parent().unwrap()).unwrap();
        write_test_script(
            &fake_hf,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/download-steps"
[ -z "${HF_TOKEN+x}${HUGGING_FACE_HUB_TOKEN+x}${HF_HUB_CACHE+x}" ] || exit 1
[ "$HF_HOME" = "$HOME/.cache/huggingface" ] && [ "$(cat "$HF_HOME/token")" = "local-fixture" ] || exit 1
partial="$HF_HOME/hub/.cache/download/partial"
if [ "$2" = "TencentARC/Pixal3D" ] && [ ! -f "$partial" ]; then
  mkdir -p "${partial%/*}"
  printf 'partial checkpoint bytes' > "$partial"
  exit 23
fi
"#,
        );
        let hf_home = manager.home_path().join(".cache/huggingface");
        fs::create_dir_all(&hf_home).unwrap();
        fs::write(hf_home.join("token"), "local-fixture").unwrap();
        let partial = hf_home.join("hub/.cache/download/partial");
        let unused_uv = root.join("unused-uv");

        let first =
            manager.install_models_with_tools(&unused_uv, &fake_python, None, Default::default());
        let error = first.expect_err("the fake Pixal3D download fails once");
        assert!(error.to_string().contains(huggingface_download::PIXAL_REPO));
        assert_eq!(
            fs::read_to_string(&partial).unwrap(),
            "partial checkpoint bytes"
        );

        let second =
            manager.install_models_with_tools(&unused_uv, &fake_python, None, Default::default());
        let error = second.expect_err("fake fixture must reach the pinned checkpoint hash check");
        assert!(error.to_string().contains("checksum mismatch"));
        let steps = fs::read_to_string(manager.home_path().join("download-steps")).unwrap();
        let dino = format!(
            "download {DINO_REPO} --revision {DINO_REVISION} model.safetensors config.json --max-workers 1"
        );
        let pixal = format!(
            "download {} --revision {} {} --max-workers 2",
            huggingface_download::PIXAL_REPO,
            huggingface_download::PIXAL_REVISION,
            huggingface_download::pixal_files(true).join(" "),
        );
        assert_eq!(
            steps.lines().collect::<Vec<_>>(),
            [
                dino.as_str(),
                pixal.as_str(),
                dino.as_str(),
                pixal.as_str(),
                "verifying"
            ],
        );
        assert_eq!(
            fs::read_to_string(&partial).unwrap(),
            "partial checkpoint bytes"
        );
        assert_eq!(
            fs::read_to_string(runtime_root.join("weights/naf_release.pth")).unwrap(),
            "invalid fixture bytes"
        );
        let python_log = fs::read_to_string(manager.home_path().join("python-env.log")).unwrap();
        assert!(python_log.lines().all(|line| {
            let fields: Vec<_> = line.split('|').collect();
            fields.len() == 5
                && fields[0].is_empty()
                && fields[1].is_empty()
                && fields[2].starts_with(&manager.home_path().to_string_lossy().to_string())
                && fields[3].starts_with(
                    &manager
                        .home_path()
                        .join(".cache/huggingface")
                        .to_string_lossy()
                        .to_string(),
                )
                && fields[4] == "1"
        }));
        assert_eq!(
            fs::read_to_string(data_root.join("owner-sentinel")).unwrap(),
            "preserve me"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
