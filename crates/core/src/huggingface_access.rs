use std::{
    io::Write,
    process::{Command, Stdio},
};

use serde::Serialize;

use crate::{CoreError, CoreResult};

pub(crate) const DINO_REVISION: &str = "ea8dc2863c51be0a264bab82070e3e8836b02d51";
const ACCESS_SCRIPT: &str = include_str!("huggingface_access.py");

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HuggingFaceAccess {
    NotConnected,
    AccessDenied,
    Verified,
}

pub(crate) fn check_access(
    mut python: Command,
    token: Option<&str>,
) -> CoreResult<HuggingFaceAccess> {
    let token = token.map(str::trim).map(validate_token).transpose()?;
    let failure = || {
        CoreError::OperationUnavailable(
            "Could not check DINOv3 file access. Check your connection to Hugging Face and retry."
                .into(),
        )
    };
    let mut child = python
        .args(["-I", "-c", ACCESS_SCRIPT, DINO_REVISION])
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| failure())?;
    let written = child
        .stdin
        .take()
        .expect("piped child stdin")
        .write_all(token.unwrap_or_default().as_bytes());
    let output = child.wait_with_output().map_err(|_| failure())?;
    if written.is_err() || !output.status.success() {
        return Err(failure());
    }
    match output.stdout.as_slice() {
        b"not_connected\n" => Ok(HuggingFaceAccess::NotConnected),
        b"access_denied\n" => Ok(HuggingFaceAccess::AccessDenied),
        b"verified\n" => Ok(HuggingFaceAccess::Verified),
        b"storage_error\n" => Err(CoreError::OperationUnavailable(
            "DINOv3 access was verified, but MeshHarbor could not save the token in its isolated cache."
                .into(),
        )),
        _ => Err(failure()),
    }
}

fn validate_token(token: &str) -> CoreResult<&str> {
    if token.len() <= 512
        && token.strip_prefix("hf_").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        Ok(token)
    } else {
        Err(CoreError::InvalidInput(
            "Enter a valid Hugging Face user access token (hf_…).".into(),
        ))
    }
}

pub(crate) fn require_access(python: Command, login_command: &str) -> CoreResult<()> {
    match check_access(python, None)? {
        HuggingFaceAccess::Verified => Ok(()),
        HuggingFaceAccess::NotConnected => Err(CoreError::OperationUnavailable(format!(
            "No Hugging Face token is connected. Connect it in Studio or run `{login_command}`."
        ))),
        HuggingFaceAccess::AccessDenied => Err(CoreError::OperationUnavailable(format!(
            "Hugging Face denied access to the DINOv3 model file. Obtain access on the model page, then connect a token with gated read access in Studio or run `{login_command}`."
        ))),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn replacement_token_goes_only_to_child_stdin() {
        let mut python = Command::new("/bin/sh");
        python.env_clear().args([
            "-c",
            r#"test "$#" -eq 4 && test "$(cat)" = "hf_fixture" && printf 'verified\n'"#,
            "fixture",
        ]);
        assert_eq!(
            check_access(python, Some(" hf_fixture\n")).unwrap(),
            HuggingFaceAccess::Verified
        );
    }
}
