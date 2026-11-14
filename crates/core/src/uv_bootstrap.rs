//! Platform-aware bootstrap for the pinned Astral uv release.
//!
//! Downloads only official GitHub release assets over HTTPS, verifies the
//! embedded SHA-256 before extraction, and installs under MeshHarbor's private
//! application-data directory. No shell is used for download or extraction.

use std::{
    env, ffi::OsStr, fs,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{CoreError, CoreResult};

const UV_VERSION: &str = "0.13.8";
const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UvAsset {
    triple: &'static str,
    archive_ext: &'static str,
    sha256: &'static str,
    windows: bool,
}

impl UvAsset {
    fn filename(self) -> String {
        format!("uv-{}{}", self.triple, self.archive_ext)
    }

    fn archive_name(self) -> String {
        format!("uv-{}{}", self.triple, self.archive_ext)
    }

    fn executable_relative(self) -> PathBuf {
        if self.windows {
            // Astral's Windows ZIP stores uv.exe at the archive root.
            PathBuf::from("uv.exe")
        } else {
            PathBuf::from(format!("uv-{}", self.triple)).join("uv")
        }
    }

    fn executable_name(self) -> &'static str {
        if self.windows { "uv.exe" } else { "uv" }
    }
}

/// Find an existing explicit/system uv, or bootstrap the pinned official
/// release into `<data-root>/tools/uv-<version>`.
pub(crate) fn ensure_uv(data_root: &Path) -> CoreResult<PathBuf> {
    if let Some(override_path) = env::var_os("MESHHARBOR_UV") {
        let path = PathBuf::from(override_path);
        if !path.is_file() {
            return Err(CoreError::OperationUnavailable(format!(
                "MESHHARBOR_UV points to a missing file: {}",
                path.display()
            )));
        }
        return Ok(path);
    }

    let app_managed = managed_executable(data_root);
    if app_managed.is_file() {
        verify_uv_version(&app_managed)?;
        return Ok(app_managed);
    }

    if let Some(system_uv) = find_on_path() {
        // Keep user-managed uv usable when already installed. uv.lock fixes
        // package versions; its presence avoids an unnecessary tool download.
        if verify_uv_command(&system_uv).is_ok() {
            return Ok(system_uv);
        }
    }

    install_pinned(data_root)
}

fn managed_executable(data_root: &Path) -> PathBuf {
    let bin = data_root.join("tools").join(format!("uv-{UV_VERSION}"));
    if cfg!(windows) {
        bin.join("uv.exe")
    } else {
        bin.join("uv")
    }
}

fn find_on_path() -> Option<PathBuf> {
    let executable = if cfg!(windows) { "uv.exe" } else { "uv" };
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(executable))
        .find(|candidate| candidate.is_file())
}

fn current_asset() -> CoreResult<UvAsset> {
    let os = env::consts::OS;
    let arch = env::consts::ARCH;
    let abi = if cfg!(all(target_os = "linux", target_env = "musl")) {
        "musl"
    } else {
        "gnu"
    };
    select_asset(os, arch, abi).ok_or_else(|| {
        CoreError::OperationUnavailable(format!(
            "Automatic uv installation is not yet supported for this MeshHarbor target ({os}/{arch}/{abi}). Install uv manually, or use a supported uv target."
        ))
    })
}

fn select_asset(os: &str, arch: &str, abi: &str) -> Option<UvAsset> {
    let (triple, archive_ext, sha256, windows) = match (os, arch, abi) {
        ("macos", "aarch64", _) => (
            "aarch64-apple-darwin",
            ".tar.gz",
            "6b71028a6b904f2ae3340ef52e595512de807d06c0a20eaef2f692581b246558",
            false,
        ),
        ("macos", "x86_64", _) => (
            "x86_64-apple-darwin",
            ".tar.gz",
            "a04c2ffeac28b2003a2c13caf538ab425fcd7def3e4a41571f4a8fb370a66af3",
            false,
        ),
        ("windows", "x86_64", _) => (
            "x86_64-pc-windows-msvc",
            ".zip",
            "529e78a9cec6cd7e3e014f7ecd8bda0c113969d4ed0f2efeedb85c30943325b8",
            true,
        ),
        ("windows", "aarch64", _) => (
            "aarch64-pc-windows-msvc",
            ".zip",
            "59e6d919bed1f72035e622a7c930058f08a8931c4c78ceaf9f56e7fb74dd2c48",
            true,
        ),
        ("linux", "x86_64", "gnu") => (
            "x86_64-unknown-linux-gnu",
            ".tar.gz",
            "5acfd9177cfecf0bf87578f059728215ccc884388988f55f7a65041b57387d91",
            false,
        ),
        ("linux", "x86_64", "musl") => (
            "x86_64-unknown-linux-musl",
            ".tar.gz",
            "61fb26dabc4f0cf1d050fc6c67d40e43e4fae2994af21ebc1e050b233bf2b5ed",
            false,
        ),
        ("linux", "aarch64", "gnu") => (
            "aarch64-unknown-linux-gnu",
            ".tar.gz",
            "74f2f5c6024d397481db228db8fc25eed420db192243f88438c118e7e013e585",
            false,
        ),
        ("linux", "aarch64", "musl") => (
            "aarch64-unknown-linux-musl",
            ".tar.gz",
            "0c6f8593fcf1eadfc60e663b685c408267559d7c5d61c689bbb9f3a7e369ba32",
            false,
        ),
        _ => return None,
    };
    Some(UvAsset {
        triple,
        archive_ext,
        sha256,
        windows,
    })
}

fn install_pinned(data_root: &Path) -> CoreResult<PathBuf> {
    let asset = current_asset()?;
    let filename = asset.filename();
    let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{filename}");
    let tools_dir = data_root.join("tools");
    fs::create_dir_all(&tools_dir)?;
    let stage = tools_dir.join(format!(".uv-bootstrap-{}", Uuid::new_v4()));
    fs::create_dir(&stage)?;
    let _cleanup = RemoveOnDrop(stage.clone());
    restrict_directory(&stage)?;

    let archive = stage.join(asset.archive_name());
    eprintln!("Downloading official uv {UV_VERSION} for {}…", asset.triple);
    download_https(&url, &archive)?;
    verify_archive(&archive, asset.sha256)?;

    let extract_dir = stage.join("extract");
    fs::create_dir(&extract_dir)?;
    extract_archive(&archive, &extract_dir, asset.windows)?;

    let candidate = extract_dir.join(asset.executable_relative());
    if !candidate.is_file() {
        return Err(CoreError::OperationUnavailable(format!(
            "The verified uv archive did not contain the expected executable {}.",
            candidate.display()
        )));
    }

    let install_stage = stage.join("install");
    fs::create_dir(&install_stage)?;
    let staged_executable = install_stage.join(asset.executable_name());
    fs::copy(&candidate, &staged_executable)?;
    make_executable(&staged_executable)?;
    verify_uv_version(&staged_executable)?;

    let destination = tools_dir.join(format!("uv-{UV_VERSION}"));
    if destination.exists() {
        // A concurrent setup may have completed while this process downloaded.
        let existing = destination.join(asset.executable_name());
        verify_uv_version(&existing)?;
        return Ok(existing);
    }
    match fs::rename(&install_stage, &destination) {
        Ok(()) => {}
        Err(error) if destination.exists() => {
            let existing = destination.join(asset.executable_name());
            verify_uv_version(&existing)?;
            let _ = error;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(destination.join(asset.executable_name()))
}

fn download_https(url: &str, destination: &Path) -> CoreResult<()> {
    // URL is constructed solely from a compile-time release pin and a
    // compile-time target mapping. curl is invoked without a shell.
    let curl = if cfg!(windows) { "curl.exe" } else { "curl" };
    download_https_with(curl, url, destination)
}

fn download_https_with(curl: impl AsRef<OsStr>, url: &str, destination: &Path) -> CoreResult<()> {
    let mut command = Command::new(curl);
    command.env_clear();
    for name in [
        "PATH",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "CURL_CA_BUNDLE",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "TMPDIR",
        "TEMP",
        "TMP",
    ] {
        if let Some(value) = env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .args([
            "--disable",
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--retry",
            "2",
            "--connect-timeout",
            "20",
            "--max-filesize",
            "33554432",
            "--output",
        ])
        .arg(destination)
        .arg(url);
    let status = command.status().map_err(|error| {
        CoreError::OperationUnavailable(format!(
            "Could not run curl to install uv. Install curl or provide MESHHARBOR_UV. Details: {error}"
        ))
    })?;
    if !status.success() {
        // curl can leave a truncated archive after a timeout. Do not leave it
        // looking like a reusable download; the app-managed stage is removed
        // by its owner as well.
        let _ = fs::remove_file(destination);
        return Err(CoreError::OperationUnavailable(format!(
            "The official uv download failed (curl exit {}). Check network access and retry.",
            status
                .code()
                .map_or_else(|| "signal".into(), |code| code.to_string())
        )));
    }
    Ok(())
}

fn verify_archive(path: &Path, expected: &str) -> CoreResult<()> {
    let metadata = fs::metadata(path)?;
    if metadata.len() == 0 || metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(CoreError::OperationUnavailable(
            "The downloaded uv archive has an unexpected size; refusing to extract it.".into(),
        ));
    }
    let bytes = fs::read(path)?;
    if !archive_hash_matches(&bytes, expected) {
        return Err(CoreError::OperationUnavailable(
            "The official uv archive checksum did not match the pinned SHA-256; refusing to extract or execute it.".into(),
        ));
    }
    Ok(())
}

fn extract_archive(archive: &Path, destination: &Path, windows: bool) -> CoreResult<()> {
    let status = if windows {
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Expand-Archive -LiteralPath $env:MESHHARBOR_UV_ARCHIVE -DestinationPath $env:MESHHARBOR_UV_DESTINATION -Force",
            ])
            .env("MESHHARBOR_UV_ARCHIVE", archive)
            .env("MESHHARBOR_UV_DESTINATION", destination);
        command.status()?
    } else {
        Command::new("tar")
            .args(["-xzf"])
            .arg(archive)
            .arg("-C")
            .arg(destination)
            .status()?
    };
    if !status.success() {
        return Err(CoreError::OperationUnavailable(format!(
            "Could not extract the SHA-256-verified uv archive (exit {}).",
            status
                .code()
                .map_or_else(|| "signal".into(), |code| code.to_string())
        )));
    }
    Ok(())
}

fn verify_uv_version(path: &Path) -> CoreResult<()> {
    let version = verify_uv_command(path)?;
    let mut parts = version.split_whitespace();
    if parts.next() != Some("uv") || parts.next() != Some(UV_VERSION) {
        return Err(CoreError::OperationUnavailable(format!(
            "Expected pinned uv {UV_VERSION}, but {} reported {version}. Remove the mismatched app-managed executable or set MESHHARBOR_UV to a compatible uv.",
            path.display()
        )));
    }
    Ok(())
}

fn verify_uv_command(path: &Path) -> CoreResult<String> {
    let output = Command::new(path).arg("--version").output()?;
    if !output.status.success() {
        return Err(CoreError::OperationUnavailable(format!(
            "uv executable {} failed its --version check.",
            path.display()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> CoreResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(windows)]
fn make_executable(_path: &Path) -> CoreResult<()> {
    Ok(())
}

fn restrict_directory(path: &Path) -> CoreResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn archive_hash_matches(bytes: &[u8], expected: &str) -> bool {
    format!("{:x}", Sha256::digest(bytes)) == expected
}

#[cfg(test)]
fn verify_archive_bytes(bytes: &[u8], expected: &str) -> bool {
    archive_hash_matches(bytes, expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_release_maps_supported_platform_targets() {
        assert_eq!(
            select_asset("macos", "aarch64", "gnu").unwrap().filename(),
            "uv-aarch64-apple-darwin.tar.gz"
        );
        assert_eq!(
            select_asset("macos", "x86_64", "gnu").unwrap().filename(),
            "uv-x86_64-apple-darwin.tar.gz"
        );
        let windows_x64 = select_asset("windows", "x86_64", "gnu").unwrap();
        assert_eq!(windows_x64.filename(), "uv-x86_64-pc-windows-msvc.zip");
        assert_eq!(windows_x64.executable_relative(), PathBuf::from("uv.exe"));
        let windows_arm64 = select_asset("windows", "aarch64", "gnu").unwrap();
        assert_eq!(windows_arm64.filename(), "uv-aarch64-pc-windows-msvc.zip");
        assert_eq!(windows_arm64.executable_relative(), PathBuf::from("uv.exe"));
        assert_eq!(
            select_asset("linux", "x86_64", "gnu").unwrap().filename(),
            "uv-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            select_asset("linux", "x86_64", "musl").unwrap().filename(),
            "uv-x86_64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(
            select_asset("linux", "aarch64", "gnu").unwrap().filename(),
            "uv-aarch64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            select_asset("linux", "aarch64", "musl").unwrap().filename(),
            "uv-aarch64-unknown-linux-musl.tar.gz"
        );
        assert!(select_asset("linux", "riscv64", "gnu").is_none());
    }

    #[test]
    fn pinned_archive_hash_accepts_only_matching_content() {
        const BLOB: &[u8] = b"verified release fixture";
        let expected = format!("{:x}", Sha256::digest(BLOB));
        assert!(verify_archive_bytes(BLOB, &expected));
        assert!(!verify_archive_bytes(BLOB, &"0".repeat(64)));
    }


    #[cfg(unix)]
    #[test]
    fn failed_curl_download_drops_partial_archive_and_hf_tokens() {
        use std::os::unix::fs::PermissionsExt;
        use uuid::Uuid;

        let root = env::temp_dir().join(format!("meshharbor-uv-download-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let fake_curl = root.join("fake-curl");
        fs::write(
            &fake_curl,
            r#"#!/bin/sh
output=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then output="$2"; shift 2; else shift; fi
done
printf 'partial archive bytes' > "$output"
printf '%s|%s\n' "$HF_TOKEN" "$HUGGING_FACE_HUB_TOKEN" > "$output.env"
exit 28
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake_curl).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_curl, permissions).unwrap();

        let destination = root.join("uv.tar.gz");
        let error = download_https_with(&fake_curl, "https://example.invalid/uv.tar.gz", &destination)
            .expect_err("the fake curl deliberately simulates a timeout");
        assert!(error.to_string().contains("curl exit 28"));
        assert!(!destination.exists());
        assert_eq!(
            fs::read_to_string(root.join("uv.tar.gz.env")).unwrap(),
            "|\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

}
