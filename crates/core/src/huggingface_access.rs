use std::{
    io::Write,
    process::{Command, Stdio},
};

use serde::Serialize;

use crate::{CoreError, CoreResult};

pub(crate) const DINO_REVISION: &str = "ea8dc2863c51be0a264bab82070e3e8836b02d51";
const ACCESS_SCRIPT: &str = include_str!("huggingface_access.py");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HuggingFaceAccess {
    NotConnected,
    AccessDenied,
    Verified,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HuggingFaceConnection {
    NotConnected,
    InvalidToken,
    Connected,
}

fn run_check(
    mut python: Command,
    args: &[&str],
    token: &str,
    failure: &str,
) -> CoreResult<Vec<u8>> {
    let unavailable = || CoreError::OperationUnavailable(failure.into());
    let mut child = python
        .args(["-I", "-c", ACCESS_SCRIPT])
        .args(args)
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| unavailable())?;
    let written = child
        .stdin
        .take()
        .expect("piped child stdin")
        .write_all(token.as_bytes());
    let output = child.wait_with_output().map_err(|_| unavailable())?;
    if written.is_err() || !output.status.success() {
        return Err(unavailable());
    }
    Ok(output.stdout)
}

pub(crate) fn check_connection(
    python: Command,
    token: Option<&str>,
) -> CoreResult<HuggingFaceConnection> {
    const FAILURE: &str =
        "Could not verify the Hugging Face account. Check the network connection and try again.";
    let token = token
        .map(str::trim)
        .map(validate_token)
        .transpose()?
        .unwrap_or_default();
    match run_check(python, &["account"], token, FAILURE)?.as_slice() {
        b"not_connected\n" => Ok(HuggingFaceConnection::NotConnected),
        b"invalid_token\n" => Ok(HuggingFaceConnection::InvalidToken),
        b"connected\n" => Ok(HuggingFaceConnection::Connected),
        b"storage_error\n" => Err(CoreError::OperationUnavailable(
            "Hugging Face verified the account, but MeshHarbor could not save the token in its isolated cache.".into(),
        )),
        _ => Err(CoreError::OperationUnavailable(FAILURE.into())),
    }
}

pub(crate) fn check_model_access(python: Command) -> CoreResult<HuggingFaceAccess> {
    const FAILURE: &str =
        "Could not check DINOv3 file access. Check your connection to Hugging Face and retry.";
    match run_check(python, &["model", DINO_REVISION], "", FAILURE)?.as_slice() {
        b"not_connected\n" => Ok(HuggingFaceAccess::NotConnected),
        b"access_denied\n" => Ok(HuggingFaceAccess::AccessDenied),
        b"verified\n" => Ok(HuggingFaceAccess::Verified),
        _ => Err(CoreError::OperationUnavailable(FAILURE.into())),
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
    match check_model_access(python)? {
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
            r#"test "$#" -eq 4 && test "$4" = "account" && test "$(cat)" = "hf_fixture" && printf 'connected\n'"#,
            "fixture",
        ]);
        assert_eq!(
            check_connection(python, Some(" hf_fixture\n")).unwrap(),
            HuggingFaceConnection::Connected
        );
    }
}
