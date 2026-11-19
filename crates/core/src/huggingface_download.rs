use std::{
    ffi::OsStr,
    path::Path,
    process::{Command, Stdio},
};

use crate::{CoreError, CoreResult, huggingface_browser::installed_browser_cli};

pub(crate) const DINO_REPO: &str = "facebook/dinov3-vitl16-pretrain-lvd1689m";
pub(crate) const PIXAL_REPO: &str = "TencentARC/Pixal3D";
pub(crate) const PIXAL_REVISION: &str = "0b31f9160aa400719af409098bff7936a932f726";

pub(crate) fn download(
    runtime_root: &Path,
    home: &Path,
    repo: &str,
    revision: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> CoreResult<()> {
    let executable =
        installed_browser_cli(runtime_root).unwrap_or_else(|| runtime_root.join(".venv/bin/hf"));
    if !executable.is_file() {
        return Err(CoreError::OperationUnavailable(
            "The managed Hugging Face CLI is missing. Set up the backend again or use browser sign-in to prepare it."
                .into(),
        ));
    }
    let status = Command::new(executable)
        .env_clear()
        .env("HOME", home)
        .env("HF_HOME", home.join(".cache/huggingface"))
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("HF_HUB_DISABLE_UPDATE_CHECK", "1")
        .env("PYTHONNOUSERSITE", "1")
        .args(["download", repo, "--revision", revision])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::io::stderr()))
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| {
            CoreError::OperationUnavailable(format!(
                "Could not start hf download for {repo}: {error}"
            ))
        })?;
    if !status.success() {
        return Err(CoreError::OperationUnavailable(format!(
            "Hugging Face download failed for {repo} ({status}). Check your connection and model access, then retry to reuse the local HF cache."
        )));
    }
    Ok(())
}
