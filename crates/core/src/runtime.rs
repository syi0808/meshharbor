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

use crate::{CoreError, CoreResult, uv_bootstrap::ensure_uv};

const PYTHON_REQUEST: &str = "3.12";
const NAF_SHA256: &str = "c096c1ab2217a5c3ac136365f721685e2201379cb69d509cfb0261183847c98f";
const CONVERTER_PACKAGES: &[&str] = &["torch==2.5.1", "safetensors==0.7.0", "numpy==1.26.4"];

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

    /// Provision a managed Python 3.12 and the source-pinned, locked MLX
    /// dependencies. If needed, `uv` is bootstrapped from a pinned official
    /// release after the user explicitly starts runtime setup.
    pub fn setup_environment(&self) -> CoreResult<RuntimeSetupReport> {
        self.ensure_apple_silicon()?;
        self.ensure_source()?;
        self.validate_source()?;
        let uv = ensure_uv(&self.runtime_root)?;

        let python_dir = self.runtime_root.join("python");
        let cache_dir = self.runtime_root.join("uv-cache");
        let home_dir = self.home_path();
        let venv_dir = self.runtime_root.join(".venv");
        std::fs::create_dir_all(&python_dir)?;
        std::fs::create_dir_all(&cache_dir)?;
        std::fs::create_dir_all(&home_dir)?;

        eprintln!("[1/4] Installing an isolated managed Python {PYTHON_REQUEST} runtime…");
        let mut install = self.uv_command(&uv, &home_dir, &cache_dir);
        install
            .args(["python", "install", PYTHON_REQUEST, "--install-dir"])
            .arg(&python_dir);
        run_visible(install, "uv could not install managed Python 3.12")?;

        eprintln!("[2/4] Locating the managed Python interpreter…");
        let python = self.find_managed_python(&uv, &home_dir, &cache_dir)?;
        let python_version = run_capture(
            Command::new(&python).arg("--version"),
            "Could not verify the managed Python interpreter",
        )?;
        if !python_version.starts_with("Python 3.12.") {
            return Err(CoreError::OperationUnavailable(format!(
                "uv returned an unexpected interpreter ({python_version}); MeshHarbor requires Python 3.12."
            )));
        }

        eprintln!("[3/4] Creating the isolated MLX environment…");
        let mut venv = self.uv_command(&uv, &home_dir, &cache_dir);
        venv.args(["venv", "--no-project", "--allow-existing", "--python"])
            .arg(&python)
            .arg(&venv_dir);
        run_visible(venv, "uv could not create the isolated Python environment")?;

        eprintln!("[4/4] Installing locked backend dependencies (uv.lock)…");
        let venv_python = if cfg!(windows) {
            venv_dir.join("Scripts/python.exe")
        } else {
            venv_dir.join("bin/python")
        };
        let mut sync = self.uv_command(&uv, &home_dir, &cache_dir);
        sync.env("UV_PROJECT_ENVIRONMENT", &venv_dir)
            .arg("sync")
            .arg("--project")
            .arg(&self.source_root)
            .arg("--python")
            .arg(&venv_python)
            .args(["--locked", "--no-dev", "--no-editable"]);
        run_visible(sync, "uv could not install the pinned MLX dependencies")?;

        let verified_version = run_capture(
            Command::new(&venv_python).arg("--version"),
            "Could not verify the created MLX environment",
        )?;
        if !verified_version.starts_with("Python 3.12.") {
            return Err(CoreError::OperationUnavailable(format!(
                "The managed environment uses unexpected interpreter {verified_version}."
            )));
        }
        run_capture(
            Command::new(&venv_python).args([
                "-c",
                "import mlx, huggingface_hub, safetensors; print('MLX runtime imports are available')",
            ]),
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
        let home = self.home_path();
        std::fs::create_dir_all(&home)?;
        let weights = self.runtime_root.join("weights");
        std::fs::create_dir_all(&weights)?;
        let uv = ensure_uv(&self.runtime_root)?;
        let scripts = self.managed_scripts_root();
        eprintln!(
            "[1/3] Checking access to the gated DINOv3 model before downloading large checkpoints…"
        );
        let mut preflight = Command::new(&python);
        preflight
            .current_dir(&self.source_root)
            .env_clear()
            .env("HOME", &home)
            .env("HF_HOME", home.join(".cache/huggingface"))
            .env("PATH", managed_path(&self.runtime_root))
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("PYTHONNOUSERSITE", "1")
            .arg(scripts.join("download_models.py"))
            .arg("--source-root")
            .arg(&self.source_root)
            .arg("--weights-dir")
            .arg(&weights)
            .arg("--preflight-only")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        run_visible(
            preflight,
            "Hugging Face model access is not ready. Accept DINOv3 access terms in your own account and log in to the isolated MeshHarbor HF cache, then retry.",
        )?;

        eprintln!(
            "[2/3] Downloading pinned model files. Large files can take time; progress is shown below…"
        );
        let mut download = Command::new(&python);
        download
            .current_dir(&self.source_root)
            .env_clear()
            .env("HOME", &home)
            .env("HF_HOME", home.join(".cache/huggingface"))
            .env("PATH", managed_path(&self.runtime_root))
            .env("PYTHONNOUSERSITE", "1")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .arg(scripts.join("download_models.py"))
            .arg("--source-root")
            .arg(&self.source_root)
            .arg("--weights-dir")
            .arg(&weights)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        run_visible(
            download,
            "Model download failed; retrying is safe and resumes cached Hugging Face downloads",
        )?;
        eprintln!(
            "[3/3] Preparing an isolated CPU-only conversion environment for the NAF checkpoint…"
        );
        self.convert_naf_checkpoint(&uv, &home, &self.runtime_root.join("uv-cache"), &weights)?;
        Ok(RuntimeSetupReport {
            backend_id: "pixal3d-mlx".into(),
            status: "model_files_ready".into(),
            runtime_root: self.runtime_root.clone(),
            python,
            lockfile: self.source_root.join("uv.lock"),
            message: "Pinned Pixal3D and gated DINOv3 snapshots were downloaded; Pixal3D/NAF hashes were verified, and NAF was converted in an isolated CPU-only environment.".into(),
        })
    }

    /// Retry the offline NAF conversion after model files are already present.
    /// Only PyPI conversion packages are downloaded; no HF login is used.
    pub fn convert_naf(&self) -> CoreResult<RuntimeSetupReport> {
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
        let uv = ensure_uv(&self.runtime_root)?;
        let home = self.home_path();
        std::fs::create_dir_all(&home)?;
        let cache = self.runtime_root.join("uv-cache");
        std::fs::create_dir_all(&cache)?;
        let weights = self.runtime_root.join("weights");
        self.convert_naf_checkpoint(&uv, &home, &cache, &weights)?;
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
        run_visible(
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
    ) -> CoreResult<()> {
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
        let managed_python = self.find_managed_python(uv, home, cache)?;
        let converter_root = self.runtime_root.join("naf-converter");
        let converter_python = if cfg!(windows) {
            converter_root.join("Scripts/python.exe")
        } else {
            converter_root.join("bin/python")
        };
        let mut venv = self.uv_command(uv, home, cache);
        venv.args(["venv", "--no-project", "--allow-existing", "--python"])
            .arg(managed_python)
            .arg(&converter_root);
        run_visible(
            venv,
            "uv could not create the isolated NAF conversion environment",
        )?;

        let mut install = self.uv_command(uv, home, cache);
        install
            .arg("pip")
            .arg("install")
            .arg("--python")
            .arg(&converter_python)
            .args(CONVERTER_PACKAGES.iter().copied());
        run_visible(
            install,
            "uv could not install the pinned NAF conversion tools",
        )?;

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
        run_visible(convert, "Could not convert the verified NAF checkpoint")
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

    fn find_managed_python(&self, uv: &Path, home: &Path, cache: &Path) -> CoreResult<PathBuf> {
        let mut command = self.uv_command(uv, home, cache);
        let output = command
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
                "uv returned an empty path for its managed Python interpreter.".into(),
            ));
        }
        Ok(PathBuf::from(path))
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
        .env("HF_HUB_DISABLE_TELEMETRY", "1");
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
}
