use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use tokio::{process::Command as AsyncCommand, time::timeout};
use uuid::Uuid;

use crate::{
    CoreError, CoreResult, HuggingFaceAccess, Pixal3dCudaRuntimeManager, Pixal3dProfile,
    Pixal3dRuntimeManager, huggingface_access::check_access, uv_bootstrap::ensure_uv,
};

const TOOL_DIR: &str = "hf-browser-cli-1.20.1";
const PACKAGE: &str = "huggingface-hub==1.20.1";
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);

pub struct HuggingFaceBrowserTool {
    profile: Pixal3dProfile,
    data_root: PathBuf,
    runtime_root: PathBuf,
    base_python: PathBuf,
    home: PathBuf,
}

impl HuggingFaceBrowserTool {
    pub fn new(profile: Pixal3dProfile, data_root: &Path) -> CoreResult<Self> {
        let data_root = fs::canonicalize(data_root)?;
        profile.ensure_managed_credentials(&data_root).map_err(|_| {
            CoreError::OperationUnavailable(
                "Browser sign-in requires the default managed Hugging Face storage without linked paths. Use an access token instead.".into(),
            )
        })?;
        let (runtime_root, base_python, home) = match profile {
            Pixal3dProfile::Mlx if cfg!(all(target_os = "macos", target_arch = "aarch64")) => {
                let manager = Pixal3dRuntimeManager::new(&data_root);
                (
                    manager.runtime_root().to_path_buf(),
                    manager.python_path(),
                    manager.home_path(),
                )
            }
            Pixal3dProfile::Cuda => {
                let manager = Pixal3dCudaRuntimeManager::new(&data_root);
                if !manager.host_supported() {
                    return Err(unsupported());
                }
                (
                    manager.runtime_root().to_path_buf(),
                    manager.python_path(),
                    manager.home_path(),
                )
            }
            _ => return Err(unsupported()),
        };
        if !base_python.is_file() {
            return Err(CoreError::OperationUnavailable(
                "Set up the backend's Python environment before starting browser sign-in.".into(),
            ));
        }
        Ok(Self {
            profile,
            data_root,
            runtime_root,
            base_python,
            home,
        })
    }

    pub async fn install(&self) -> CoreResult<()> {
        let directory = self.runtime_root.join(TOOL_DIR);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if !metadata.file_type().is_dir() => return Err(install_error()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(install_error()),
        }
        if self.hf_path().is_file() && access_python(&self.runtime_root).is_some() {
            return Ok(());
        }
        let uv_root = self.runtime_root.clone();
        let uv = tokio::task::spawn_blocking(move || ensure_uv(&uv_root))
            .await
            .map_err(|_| install_error())?
            .map_err(|_| install_error())?;
        let uv_command = || {
            let mut command = AsyncCommand::new(&uv);
            command
                .env_clear()
                .env("HOME", &self.home)
                .env("UV_CACHE_DIR", self.runtime_root.join("uv-cache"))
                .env("UV_NO_CONFIG", "1")
                .env("UV_NO_PROGRESS", "1")
                .env("UV_HTTP_TIMEOUT", "20")
                .env("UV_PYTHON_DOWNLOADS", "never")
                .current_dir(&self.runtime_root);
            command
        };
        let mut venv = uv_command();
        venv.args(["venv", "--no-project", "--allow-existing", "--python"])
            .arg(&self.base_python)
            .arg(&directory);
        install_step(venv).await?;
        let mut pip = uv_command();
        pip.args(["pip", "install", "--python"])
            .arg(directory.join("bin/python"))
            .args([
                "--default-index",
                "https://pypi.org/simple",
                "--only-binary",
                ":all:",
                PACKAGE,
            ]);
        install_step(pip).await?;
        if !self.hf_path().is_file() {
            return Err(install_error());
        }
        fs::write(directory.join(".installed"), b"1.20.1").map_err(|_| install_error())?;
        Ok(())
    }

    pub fn stage(&self) -> CoreResult<BrowserCredentials> {
        self.profile.ensure_managed_credentials(&self.data_root)?;
        let live = self.home.join(".cache/huggingface");
        let staged = live.join(format!(".browser-login-{}", Uuid::new_v4()));
        fs::create_dir_all(&live)?;
        clear_staged_credentials(&live)?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&staged)?;
        let credentials = BrowserCredentials { staged, live };
        let named_tokens = credentials.live.join("stored_tokens");
        if named_tokens.is_file() {
            fs::copy(named_tokens, credentials.staged.join("stored_tokens"))?;
        }
        Ok(credentials)
    }

    pub fn login_command(&self, credentials: &BrowserCredentials) -> AsyncCommand {
        let mut command = AsyncCommand::new(self.hf_path());
        command
            .env_clear()
            .env("HOME", &credentials.staged)
            .env("HF_HOME", &credentials.staged)
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("HF_HUB_DISABLE_UPDATE_CHECK", "1")
            .env("PYTHONUNBUFFERED", "1")
            .env("NO_COLOR", "1")
            .args(["auth", "login", "--format", "agent", "--force"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }

    pub fn verify(&self, credentials: &BrowserCredentials) -> CoreResult<HuggingFaceAccess> {
        let python = access_python(&self.runtime_root).ok_or_else(install_error)?;
        let mut command = Command::new(python);
        command
            .env_clear()
            .env("HOME", &credentials.staged)
            .env("HF_HOME", &credentials.staged);
        check_access(command, None)
    }

    pub fn commit(&self, credentials: BrowserCredentials) -> CoreResult<()> {
        self.profile.ensure_managed_credentials(&self.data_root)?;
        credentials.commit()
    }

    fn hf_path(&self) -> PathBuf {
        self.runtime_root.join(TOOL_DIR).join("bin/hf")
    }
}

pub struct BrowserCredentials {
    staged: PathBuf,
    live: PathBuf,
}

impl BrowserCredentials {
    fn commit(&self) -> CoreResult<()> {
        for name in ["token", "stored_tokens"] {
            let metadata = fs::symlink_metadata(self.staged.join(name))?;
            if !metadata.file_type().is_file() || metadata.len() == 0 {
                return Err(CoreError::OperationUnavailable(
                    "The login did not produce usable credentials.".into(),
                ));
            }
        }
        fs::rename(
            self.staged.join("stored_tokens"),
            self.live.join("stored_tokens"),
        )?;
        fs::rename(self.staged.join("token"), self.live.join("token"))?;
        Ok(())
    }
}

impl Drop for BrowserCredentials {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.staged);
    }
}

pub(crate) fn access_python(runtime_root: &Path) -> Option<PathBuf> {
    let directory = runtime_root.join(TOOL_DIR);
    let python = directory.join("bin/python");
    (directory.join(".installed").is_file() && python.is_file()).then_some(python)
}

pub(crate) fn clear_staged_credentials(hf_home: &Path) -> CoreResult<()> {
    let entries = match fs::read_dir(hf_home) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let is_stage = name
            .to_str()
            .and_then(|name| name.strip_prefix(".browser-login-"))
            .is_some_and(|id| Uuid::parse_str(id).is_ok());
        if is_stage && entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

async fn install_step(mut command: AsyncCommand) -> CoreResult<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| install_error())?;
    let status = timeout(INSTALL_TIMEOUT, child.wait())
        .await
        .map_err(|_| install_error())?
        .map_err(|_| install_error())?;
    if !status.success() {
        return Err(install_error());
    }
    Ok(())
}

fn unsupported() -> CoreError {
    CoreError::OperationUnavailable(
        "Browser sign-in is available for the supported MLX and CUDA runtimes.".into(),
    )
}

fn install_error() -> CoreError {
    CoreError::OperationUnavailable("Could not prepare the managed Hugging Face CLI. Check your network connection and try again, or use an access token.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_credentials_do_not_replace_the_active_token_until_commit() {
        let live =
            std::env::temp_dir().join(format!("meshharbor-browser-stage-{}", Uuid::new_v4()));
        fs::create_dir(&live).unwrap();
        fs::write(live.join("token"), b"prior").unwrap();
        for commit in [false, true] {
            let staged = live.join("staged");
            fs::create_dir(&staged).unwrap();
            let credentials = BrowserCredentials {
                staged: staged.clone(),
                live: live.clone(),
            };
            fs::write(staged.join("token"), b"browser").unwrap();
            fs::write(staged.join("stored_tokens"), b"refresh metadata").unwrap();
            assert_eq!(fs::read(live.join("token")).unwrap(), b"prior");
            if commit {
                credentials.commit().unwrap();
            }
            drop(credentials);
            assert!(!staged.exists());
        }
        assert_eq!(fs::read(live.join("token")).unwrap(), b"browser");
        assert_eq!(
            fs::read(live.join("stored_tokens")).unwrap(),
            b"refresh metadata"
        );
        fs::remove_dir_all(live).unwrap();
    }
}
