use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use tokio::process::Command as AsyncCommand;
use uuid::Uuid;

use crate::{
    CoreError, CoreResult, Pixal3dCudaRuntimeManager, Pixal3dProfile, Pixal3dRuntimeManager,
    model_environment::ModelEnvironment,
};

const TOOL_DIR: &str = "hf-browser-cli-1.20.1";
pub(crate) const PACKAGE: &str = "huggingface-hub==1.20.1";

pub struct HuggingFaceBrowserTool {
    profile: Pixal3dProfile,
    data_root: PathBuf,
    cli: PathBuf,
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
        let cli = ModelEnvironment::require(&data_root)?.hf_cli();
        let home = match profile {
            Pixal3dProfile::Mlx if cfg!(all(target_os = "macos", target_arch = "aarch64")) => {
                Pixal3dRuntimeManager::new(&data_root).home_path()
            }
            Pixal3dProfile::Cuda => {
                let manager = Pixal3dCudaRuntimeManager::new(&data_root);
                if !manager.host_supported() {
                    return Err(unsupported());
                }
                manager.home_path()
            }
            _ => return Err(unsupported()),
        };
        fs::create_dir_all(&home)?;
        Ok(Self {
            profile,
            data_root,
            cli,
            home,
        })
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
        let mut command = AsyncCommand::new(&self.cli);
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

    pub fn commit(&self, credentials: BrowserCredentials) -> CoreResult<()> {
        self.profile.ensure_managed_credentials(&self.data_root)?;
        credentials.commit()
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

pub(crate) fn installed_browser_cli(runtime_root: &Path) -> Option<PathBuf> {
    let executable = browser_cli_path(runtime_root);
    (access_python(runtime_root).is_some() && executable.is_file()).then_some(executable)
}

fn browser_cli_path(runtime_root: &Path) -> PathBuf {
    runtime_root.join(TOOL_DIR).join("bin/hf")
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

fn unsupported() -> CoreError {
    CoreError::OperationUnavailable(
        "Browser sign-in is available for the supported MLX and CUDA runtimes.".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_only_reuse_a_completed_browser_tool_installation() {
        let runtime_root =
            std::env::temp_dir().join(format!("meshharbor-browser-tool-{}", Uuid::new_v4()));
        let executable = browser_cli_path(&runtime_root);
        let directory = runtime_root.join(TOOL_DIR);
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"hf fixture").unwrap();
        fs::write(directory.join("bin/python"), b"python fixture").unwrap();
        assert_eq!(installed_browser_cli(&runtime_root), None);
        fs::write(directory.join(".installed"), b"1.20.1").unwrap();
        assert_eq!(installed_browser_cli(&runtime_root), Some(executable));
        fs::remove_dir_all(runtime_root).unwrap();
    }

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
