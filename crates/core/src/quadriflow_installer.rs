//! Fail-closed downloader and transactional installer for a reviewed QuadriFlow sidecar.
//!
//! The checked-in release manifest is empty, so the public installer stops before
//! touching disk or starting a downloader. A future nonempty manifest must still
//! be reviewed and committed with the application. Its embedded Sigstore bundle
//! and claims are verified before I/O; SHA-256 pins then protect downloaded bytes.
//! Cryptographic provenance does not certify license clearance or release approval.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    QuadriFlowArchiveFormat, QuadriFlowManifestError, QuadriFlowPlatform,
    QuadriFlowReleaseArtifact, QuadriFlowReleaseManifest, bundled_quadriflow_release_manifest,
    validate_quadriflow_release_manifest,
};

const MAX_CURL_TRAILER_BYTES: usize = 4096;
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const ACTIVE_MARKER_SCHEMA: u32 = 1;

#[derive(Debug, Error)]
pub enum QuadriFlowInstallError {
    #[error(transparent)]
    Manifest(#[from] QuadriFlowManifestError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("No approved QuadriFlow release artifacts are available in this build.")]
    NoReleaseArtifact,
    #[error("QuadriFlow release signature verification is unavailable in this build.")]
    ReleaseVerificationUnavailable,
    #[error("QuadriFlow release signature or provenance does not match the pinned policy.")]
    ReleaseVerificationFailed,
    #[error("QuadriFlow does not publish a package for this host platform.")]
    UnsupportedPlatform,
    #[error("The QuadriFlow installer is already running for this data directory.")]
    Busy,
    #[error("QuadriFlow download failed: {0}")]
    Download(String),
    #[error("QuadriFlow archive is invalid or unsafe: {0}")]
    Archive(String),
    #[error("QuadriFlow installer data directory is unsafe or not private.")]
    UnsafeDirectory,
    #[error("An installed version with this release tag has different bytes.")]
    ExistingVersionMismatch,
    #[error("No verified previous QuadriFlow installation is available for rollback.")]
    NoRollback,
    #[error("Installed QuadriFlow activation metadata is invalid.")]
    ActivationMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuadriFlowInstallResult {
    pub release_tag: String,
    pub executable: PathBuf,
    pub already_installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuadriFlowBackendStatus {
    pub backend_id: String,
    pub status: String,
    pub available: bool,
    pub managed: bool,
    pub sidecar_source: Option<String>,
    pub install_available: bool,
    pub message: String,
    pub next_step: String,
}

/// Capability token required to resolve or execute a managed sidecar.
/// This token is issued only after the complete manifest and signed provenance
/// have passed the in-process verifier.
struct VerifiedQuadriFlowManifest<'a> {
    manifest: &'a QuadriFlowReleaseManifest,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActiveMarker {
    schema_version: u32,
    release_tag: String,
    platform: QuadriFlowPlatform,
    executable_sha256: String,
}

struct InstallLock(PathBuf);

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct PrivateTempDir {
    path: PathBuf,
    cleanup: bool,
}

fn create_private_directory(path: &Path) -> Result<(), QuadriFlowInstallError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}

impl PrivateTempDir {
    fn create(parent: &Path, prefix: &str) -> Result<Self, QuadriFlowInstallError> {
        for _ in 0..4 {
            let path = parent.join(format!(".{prefix}-{}", Uuid::new_v4()));
            match create_private_directory(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        cleanup: true,
                    });
                }
                Err(QuadriFlowInstallError::Io(error))
                    if error.kind() == io::ErrorKind::AlreadyExists =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        Err(QuadriFlowInstallError::Busy)
    }

    fn disarm(&mut self) {
        self.cleanup = false;
    }
}

impl Drop for PrivateTempDir {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Fail-closed entrypoint for installing a native QuadriFlow sidecar.
/// A release must pass the in-process signature/provenance verifier before any
/// filesystem or network side effect. The bundled manifest is currently empty.
pub fn install_bundled_quadriflow_sidecar(
    data_root: &Path,
) -> Result<QuadriFlowInstallResult, QuadriFlowInstallError> {
    let platform = host_platform().ok_or(QuadriFlowInstallError::UnsupportedPlatform)?;
    let manifest = bundled_quadriflow_release_manifest()?;
    let verified = verified_release_manifest(&manifest)?;
    install_manifest_with_fetcher(data_root, &verified, platform, download_archive)
}

/// Finds the current user's verified, managed executable for the requested host.
/// This checks the marker and binary digest; it never downloads or executes.
pub fn installed_quadriflow_sidecar(
    data_root: &Path,
    platform: QuadriFlowPlatform,
) -> Option<PathBuf> {
    let manifest = bundled_quadriflow_release_manifest().ok()?;
    let verified = verified_release_manifest(&manifest).ok()?;
    resolve_installed(data_root, platform, &verified)
        .ok()
        .flatten()
}

/// Roll back activation to the previous verified version while preserving both
/// immutable version directories. Removing the newest marker is the atomic switch.
pub fn rollback_quadriflow_sidecar(
    data_root: &Path,
    platform: QuadriFlowPlatform,
) -> Result<PathBuf, QuadriFlowInstallError> {
    let manifest = bundled_quadriflow_release_manifest()?;
    let verified = verified_release_manifest(&manifest)?;
    rollback_with_verified_manifest(data_root, platform, &verified)
}

fn rollback_with_verified_manifest(
    data_root: &Path,
    platform: QuadriFlowPlatform,
    verified: &VerifiedQuadriFlowManifest<'_>,
) -> Result<PathBuf, QuadriFlowInstallError> {
    let base = prepare_install_base(data_root)?;
    let _lock = acquire_lock(&base)?;
    let active_dir = base.join("active");
    let markers = active_markers(&active_dir)?;
    if markers.len() < 2 {
        return Err(QuadriFlowInstallError::NoRollback);
    }

    let previous_marker_path = &markers[markers.len() - 2];
    let previous = resolve_marker(&base, previous_marker_path, platform, verified)?
        .ok_or(QuadriFlowInstallError::NoRollback)?;
    let previous_marker: ActiveMarker = serde_json::from_slice(&fs::read(previous_marker_path)?)
        .map_err(|_| QuadriFlowInstallError::ActivationMetadata)?;

    // Append a new generation instead of deleting the newest marker. This keeps
    // generation numbers monotonic after rollback and preserves atomic activation.
    append_active_marker(&active_dir, &previous_marker)?;
    trim_active_history(&active_dir);
    Ok(previous)
}

pub(crate) fn default_data_root() -> PathBuf {
    if let Some(path) = std::env::var_os("MESHHARBOR_HOME") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("MeshHarbor");
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path).join("Library/Application Support/MeshHarbor");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("meshharbor");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".local/share/meshharbor")
}

pub(crate) fn native_installed_sidecar(data_root: &Path) -> Option<PathBuf> {
    installed_quadriflow_sidecar(data_root, host_platform()?)
}

fn host_platform() -> Option<QuadriFlowPlatform> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(QuadriFlowPlatform::LinuxX86_64)
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(QuadriFlowPlatform::MacosAarch64)
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some(QuadriFlowPlatform::WindowsX86_64)
    } else {
        None
    }
}

fn verified_release_manifest<'a>(
    manifest: &'a QuadriFlowReleaseManifest,
) -> Result<VerifiedQuadriFlowManifest<'a>, QuadriFlowInstallError> {
    validate_quadriflow_release_manifest(manifest)?;
    if manifest.artifacts.is_empty() {
        return Err(QuadriFlowInstallError::NoReleaseArtifact);
    }
    // The bundle bytes are app-pinned in the manifest, so this performs no
    // network or filesystem I/O and must succeed before issuing the capability.
    for artifact in &manifest.artifacts {
        crate::quadriflow_attestation::verify_release_artifact(&manifest.source.commit, artifact)
            .map_err(|_| QuadriFlowInstallError::ReleaseVerificationFailed)?;
    }
    Ok(VerifiedQuadriFlowManifest { manifest })
}

pub fn quadriflow_backend_status(data_root: &Path) -> QuadriFlowBackendStatus {
    let managed = host_platform()
        .and_then(|platform| installed_quadriflow_sidecar(data_root, platform))
        .is_some();
    if managed {
        return QuadriFlowBackendStatus {
            backend_id: "quadriflow".into(),
            status: "ready".into(),
            available: true,
            managed: true,
            sidecar_source: Some("managed_verified".into()),
            install_available: false,
            message: "A verified, manifest-matched managed QuadriFlow sidecar is available.".into(),
            next_step: "No installation is needed.".into(),
        };
    }

    let (status, message, next_step, install_available) = match bundled_quadriflow_release_manifest(
    ) {
        Ok(manifest) if manifest.artifacts.is_empty() => (
            if host_platform().is_some() {
                "release_unavailable"
            } else {
                "unsupported_platform"
            },
            "This build does not include an approved QuadriFlow release artifact.",
            "Wait for a reviewed, signed release with verified provenance. This build does not download or install unsigned or temporary CI artifacts.",
            false,
        ),
        Ok(manifest) => match verified_release_manifest(&manifest) {
            Ok(_) => match host_platform() {
                Some(platform)
                    if manifest
                        .artifacts
                        .iter()
                        .any(|artifact| artifact.platform == platform) =>
                {
                    (
                        "install_available",
                        "A manifest-pinned QuadriFlow artifact has a verified public Sigstore signature and provenance.",
                        "An installation request will recheck archive and payload hashes before activation.",
                        true,
                    )
                }
                Some(_) => (
                    "platform_unavailable",
                    "The signed QuadriFlow manifest has no artifact for this host platform.",
                    "Use a supported host platform with a signed package in the manifest.",
                    false,
                ),
                None => (
                    "unsupported_platform",
                    "QuadriFlow installation is not supported on this host platform.",
                    "Use a supported host platform with a signed package in the manifest.",
                    false,
                ),
            },
            Err(_) => (
                "release_verification_failed",
                "A release record exists, but its signature or provenance does not match the pinned policy.",
                "Installation is disabled. Review the manifest, signature, and signed source materials.",
                false,
            ),
        },
        Err(_) => (
            "manifest_invalid",
            "The bundled QuadriFlow release manifest is invalid; installation is disabled.",
            "Use a build with a valid, reviewed release manifest.",
            false,
        ),
    };
    QuadriFlowBackendStatus {
        backend_id: "quadriflow".into(),
        status: status.into(),
        available: false,
        managed: false,
        sidecar_source: None,
        install_available,
        message: message.into(),
        next_step: next_step.into(),
    }
}

fn install_manifest_with_fetcher<F>(
    data_root: &Path,
    verified: &VerifiedQuadriFlowManifest<'_>,
    platform: QuadriFlowPlatform,
    fetcher: F,
) -> Result<QuadriFlowInstallResult, QuadriFlowInstallError>
where
    F: FnOnce(&str, u64, &Path) -> Result<(), QuadriFlowInstallError>,
{
    let manifest = verified.manifest;
    validate_quadriflow_release_manifest(manifest)?;
    let artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == platform)
        .ok_or(QuadriFlowInstallError::NoReleaseArtifact)?;
    if artifact.archive.format != QuadriFlowArchiveFormat::Tar
        || artifact.archive.size_bytes > MAX_ARCHIVE_BYTES
    {
        return Err(QuadriFlowInstallError::Archive(
            "Only bounded, uncompressed USTAR packages are supported.".into(),
        ));
    }
    let release_tag = manifest
        .release_tag
        .as_deref()
        .ok_or(QuadriFlowInstallError::NoReleaseArtifact)?;

    let base = prepare_install_base(data_root)?;
    let _lock = acquire_lock(&base)?;
    let versions_dir = base.join("versions");
    let active_dir = base.join("active");
    ensure_private_directory(&versions_dir)?;
    ensure_private_directory(&active_dir)?;

    let version_dir = versions_dir.join(release_tag);
    let executable = version_dir.join(&artifact.payload.executable.path);
    let already_installed = match symlink_metadata_optional(&version_dir)? {
        Some(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(QuadriFlowInstallError::UnsafeDirectory);
            }
            if !verify_installed_payloads(&version_dir, artifact)? {
                return Err(QuadriFlowInstallError::ExistingVersionMismatch);
            }
            true
        }
        None => false,
    };

    let mut newly_created_version = false;
    if !already_installed {
        let download_dir = PrivateTempDir::create(&base, "download")?;
        let archive_path = download_dir.path.join("package.tar");
        fetcher(
            &artifact.archive.url,
            artifact.archive.size_bytes,
            &archive_path,
        )?;
        if !verify_file(
            &archive_path,
            artifact.archive.size_bytes,
            &artifact.archive.sha256,
            MAX_ARCHIVE_BYTES,
        )? {
            return Err(archive_error(
                "The downloaded archive size or SHA-256 digest did not match the manifest.",
            ));
        }

        let mut staged_version = PrivateTempDir::create(&versions_dir, "stage")?;
        extract_ustar(&archive_path, &staged_version.path, artifact)?;
        fs::rename(&staged_version.path, &version_dir)?;
        staged_version.disarm();
        newly_created_version = true;
    }

    let marker_result = append_active_marker(
        &active_dir,
        &ActiveMarker {
            schema_version: ACTIVE_MARKER_SCHEMA,
            release_tag: release_tag.to_owned(),
            platform,
            executable_sha256: artifact.payload.executable.sha256.clone(),
        },
    );
    if let Err(error) = marker_result {
        if newly_created_version {
            let _ = fs::remove_dir_all(&version_dir);
        }
        return Err(error);
    }
    trim_active_history(&active_dir);
    Ok(QuadriFlowInstallResult {
        release_tag: release_tag.to_owned(),
        executable,
        already_installed,
    })
}

fn download_archive(
    url: &str,
    expected_size: u64,
    destination: &Path,
) -> Result<(), QuadriFlowInstallError> {
    let curl = system_curl().ok_or_else(|| {
        QuadriFlowInstallError::Download("A supported system curl executable was not found.".into())
    })?;
    let size = expected_size.to_string();
    let write_out = "\n__MESHHARBOR_META__%{size_download}\n%{url_effective}\n";
    let mut command = Command::new(curl);
    command
        .env_clear()
        .args([
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-redirs",
            "3",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "15",
            "--max-time",
            "180",
            "--speed-limit",
            "1024",
            "--speed-time",
            "30",
            "--max-filesize",
            &size,
            "--write-out",
            write_out,
            url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = ChildGuard(command.spawn().map_err(|error| {
        QuadriFlowInstallError::Download(format!("Could not start system curl: {error}"))
    })?);
    let mut stdout =
        child.0.stdout.take().ok_or_else(|| {
            QuadriFlowInstallError::Download("curl output is unavailable.".into())
        })?;
    let mut output = create_private_file(destination)?;
    let mut remaining = expected_size;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let amount = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| QuadriFlowInstallError::Download("Invalid download size.".into()))?;
        stdout.read_exact(&mut buffer[..amount]).map_err(|error| {
            QuadriFlowInstallError::Download(format!(
                "The release download was incomplete: {error}"
            ))
        })?;
        output.write_all(&buffer[..amount])?;
        remaining -= amount as u64;
    }
    output.sync_all()?;

    let mut trailer = Vec::new();
    stdout
        .by_ref()
        .take((MAX_CURL_TRAILER_BYTES + 1) as u64)
        .read_to_end(&mut trailer)?;
    if trailer.len() > MAX_CURL_TRAILER_BYTES {
        return Err(QuadriFlowInstallError::Download(
            "The release response exceeded its bounded metadata trailer.".into(),
        ));
    }
    drop(stdout);

    let status = child.0.wait()?;
    if !status.success() {
        return Err(QuadriFlowInstallError::Download(format!(
            "curl exited with {}.",
            status
                .code()
                .map_or_else(|| "a signal".into(), |code| code.to_string())
        )));
    }
    validate_curl_trailer(&trailer, expected_size)?;
    Ok(())
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn validate_curl_trailer(trailer: &[u8], expected_size: u64) -> Result<(), QuadriFlowInstallError> {
    let text = std::str::from_utf8(trailer)
        .map_err(|_| QuadriFlowInstallError::Download("Invalid curl response metadata.".into()))?;
    let remainder = text.strip_prefix("\n__MESHHARBOR_META__").ok_or_else(|| {
        QuadriFlowInstallError::Download(
            "The response exceeded its pinned archive size or omitted metadata.".into(),
        )
    })?;
    if remainder.matches("__MESHHARBOR_META__").count() != 0 {
        return Err(QuadriFlowInstallError::Download(
            "Unexpected bytes followed the pinned release archive.".into(),
        ));
    }
    let lines: Vec<_> = remainder.trim_end_matches('\n').split('\n').collect();
    if lines.len() != 2
        || lines[0].parse::<u64>().ok() != Some(expected_size)
        || !trusted_effective_url(lines[1])
    {
        return Err(QuadriFlowInstallError::Download(
            "The release response size or final HTTPS host is not trusted.".into(),
        ));
    }
    Ok(())
}

fn trusted_effective_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty()
        || authority
            .chars()
            .any(|character| matches!(character, '@' | ':' | '\\'))
    {
        return false;
    }
    matches!(
        authority,
        "github.com" | "release-assets.githubusercontent.com" | "objects.githubusercontent.com"
    )
}

fn system_curl() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let root = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("WINDIR"))?;
        let path = PathBuf::from(root).join("System32/curl.exe");
        return path.is_file().then_some(path);
    }
    #[cfg(not(target_os = "windows"))]
    {
        ["/usr/bin/curl", "/bin/curl"]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| {
                path.is_file() && {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::metadata(path)
                            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
                            .unwrap_or(false)
                    }
                    #[cfg(not(unix))]
                    {
                        true
                    }
                }
            })
    }
}

fn extract_ustar(
    archive: &Path,
    destination: &Path,
    artifact: &QuadriFlowReleaseArtifact,
) -> Result<(), QuadriFlowInstallError> {
    let metadata = fs::metadata(archive)?;
    if metadata.len() != artifact.archive.size_bytes
        || metadata.len() < 1024
        || metadata.len() > MAX_ARCHIVE_BYTES
        || !metadata.len().is_multiple_of(512)
    {
        return Err(QuadriFlowInstallError::Archive(
            "The USTAR archive has an invalid size.".into(),
        ));
    }
    let mut input = File::open(archive)?;
    let expected = [
        (&artifact.payload.executable, true),
        (&artifact.payload.third_party_notices, false),
        (&artifact.payload.sbom, false),
        (&artifact.payload.provenance, false),
        (&artifact.payload.provenance_signature, false),
    ];
    let mut seen = HashSet::new();
    let mut found = HashSet::new();
    let mut header = [0u8; 512];

    loop {
        input.read_exact(&mut header)?;
        if header.iter().all(|byte| *byte == 0) {
            let mut second = [0u8; 512];
            input.read_exact(&mut second)?;
            if second.iter().any(|byte| *byte != 0) {
                return Err(archive_error(
                    "The archive lacks two end-of-archive blocks.",
                ));
            }
            let mut trailing = [0u8; 512];
            loop {
                let count = input.read(&mut trailing)?;
                if count == 0 {
                    break;
                }
                if trailing[..count].iter().any(|byte| *byte != 0) {
                    return Err(archive_error("Nonzero bytes follow the USTAR trailer."));
                }
            }
            break;
        }

        validate_ustar_header(&header)?;
        let raw_name = header_string(&header[0..100])?;
        let raw_prefix = header_string(&header[345..500])?;
        let path = if raw_prefix.is_empty() {
            raw_name
        } else if raw_name.is_empty() {
            return Err(archive_error("An archive path has an empty file name."));
        } else {
            format!("{raw_prefix}/{raw_name}")
        };
        let kind = header[156];
        let normalized = normalize_archive_path(&path, kind == b'5')?;
        let size = parse_octal(&header[124..136])?;

        if !seen.insert(normalized.clone()) {
            return Err(archive_error("The archive contains a duplicate path."));
        }
        if kind == b'5' {
            if normalized != "bin" || size != 0 {
                return Err(archive_error("Only the empty bin/ directory is permitted."));
            }
            ensure_private_directory(&destination.join("bin"))?;
            continue;
        }
        if kind != 0 && kind != b'0' {
            return Err(archive_error(
                "Links and special tar entry types are forbidden.",
            ));
        }
        let Some((expected_file, executable)) =
            expected.iter().find(|(file, _)| file.path == normalized)
        else {
            return Err(archive_error("The archive contains an unexpected file."));
        };
        if size == 0 || size != expected_file.size_bytes {
            return Err(archive_error("A payload file has an unexpected size."));
        }

        let output_path = destination.join(&normalized);
        if let Some(parent) = output_path.parent() {
            ensure_private_directory(parent)?;
        }
        let mut output = create_private_file(&output_path)?;
        let mut hasher = Sha256::new();
        let mut remaining = size;
        let mut chunk = [0u8; 64 * 1024];
        while remaining > 0 {
            let amount = usize::try_from(remaining.min(chunk.len() as u64))
                .map_err(|_| archive_error("A payload length is not representable."))?;
            input.read_exact(&mut chunk[..amount])?;
            output.write_all(&chunk[..amount])?;
            hasher.update(&chunk[..amount]);
            remaining -= amount as u64;
        }
        output.sync_all()?;
        let digest = format!("{:x}", hasher.finalize());
        if digest != expected_file.sha256 {
            return Err(archive_error("A payload SHA-256 digest did not match."));
        }
        set_installed_file_mode(&output_path, *executable)?;
        found.insert(normalized);

        let padding = (512 - (size % 512)) % 512;
        if padding > 0 {
            let mut pad = [0u8; 512];
            let amount = usize::try_from(padding)
                .map_err(|_| archive_error("Invalid USTAR padding length."))?;
            input.read_exact(&mut pad[..amount])?;
            if pad[..amount].iter().any(|byte| *byte != 0) {
                return Err(archive_error("USTAR padding must contain only zero bytes."));
            }
        }
    }

    let expected_paths: HashSet<_> = expected
        .iter()
        .map(|(file, _)| file.path.as_str())
        .collect();
    let found_paths: HashSet<_> = found.iter().map(String::as_str).collect();
    if expected_paths != found_paths {
        return Err(archive_error(
            "The USTAR archive is missing required payload files.",
        ));
    }
    Ok(())
}

fn validate_ustar_header(header: &[u8; 512]) -> Result<(), QuadriFlowInstallError> {
    if &header[257..263] != b"ustar\0" || &header[263..265] != b"00" {
        return Err(archive_error("Only POSIX USTAR headers are supported."));
    }
    let stored = parse_octal(&header[148..156])?;
    let actual: u64 = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                b' ' as u64
            } else {
                *byte as u64
            }
        })
        .sum();
    if stored != actual || !header[157..257].iter().all(|byte| *byte == 0) {
        return Err(archive_error("A USTAR checksum or link field is invalid."));
    }
    Ok(())
}

fn header_string(field: &[u8]) -> Result<String, QuadriFlowInstallError> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    if field[end..].iter().any(|byte| *byte != 0) {
        return Err(archive_error(
            "A USTAR path field has nonzero data after NUL.",
        ));
    }
    let value = std::str::from_utf8(&field[..end])
        .map_err(|_| archive_error("A USTAR path is not valid UTF-8."))?;
    if !value.is_ascii() {
        return Err(archive_error("Only ASCII USTAR paths are permitted."));
    }
    Ok(value.to_owned())
}

fn normalize_archive_path(
    path: &str,
    is_directory: bool,
) -> Result<String, QuadriFlowInstallError> {
    let value = if is_directory {
        path.strip_suffix('/').unwrap_or(path)
    } else {
        path
    };
    if value.is_empty()
        || value.starts_with('/')
        || value
            .chars()
            .any(|character| matches!(character, '\\' | ':'))
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(archive_error(
            "A USTAR path is absolute or contains unsafe components.",
        ));
    }
    Ok(value.to_owned())
}

fn parse_octal(field: &[u8]) -> Result<u64, QuadriFlowInstallError> {
    let end = field
        .iter()
        .position(|byte| *byte == 0 || *byte == b' ')
        .unwrap_or(field.len());
    if field[end..].iter().any(|byte| *byte != 0 && *byte != b' ') {
        return Err(archive_error("A USTAR numeric field is malformed."));
    }
    let digits = &field[..end];
    if digits.is_empty() || digits.iter().any(|byte| !(b'0'..=b'7').contains(byte)) {
        return Err(archive_error(
            "Only octal USTAR numeric fields are supported.",
        ));
    }
    digits.iter().try_fold(0u64, |value, digit| {
        value
            .checked_mul(8)
            .and_then(|value| value.checked_add((*digit - b'0') as u64))
            .ok_or_else(|| archive_error("A USTAR numeric field overflowed."))
    })
}

fn verify_installed_payloads(
    directory: &Path,
    artifact: &QuadriFlowReleaseArtifact,
) -> Result<bool, QuadriFlowInstallError> {
    for (file, executable) in [
        (&artifact.payload.executable, true),
        (&artifact.payload.third_party_notices, false),
        (&artifact.payload.sbom, false),
        (&artifact.payload.provenance, false),
        (&artifact.payload.provenance_signature, false),
    ] {
        let path = directory.join(&file.path);
        if file.path.starts_with("bin/") {
            let bin_dir = directory.join("bin");
            match symlink_metadata_optional(&bin_dir)? {
                Some(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
                _ => return Ok(false),
            }
        }
        let Some(metadata) = symlink_metadata_optional(&path)? else {
            return Ok(false);
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Ok(false);
        }
        if !verify_file(&path, file.size_bytes, &file.sha256, file.size_bytes)? {
            return Ok(false);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            if mode & 0o077 != 0 || executable != (mode & 0o111 != 0) {
                return Ok(false);
            }
        }
        #[cfg(not(unix))]
        let _ = executable;
    }
    Ok(true)
}

fn resolve_installed(
    data_root: &Path,
    platform: QuadriFlowPlatform,
    verified: &VerifiedQuadriFlowManifest<'_>,
) -> Result<Option<PathBuf>, QuadriFlowInstallError> {
    let base = install_base_if_present(data_root)?;
    let Some(base) = base else {
        return Ok(None);
    };
    let active_dir = base.join("active");
    let markers = active_markers(&active_dir)?;
    let Some(marker) = markers.last() else {
        return Ok(None);
    };
    resolve_marker(&base, marker, platform, verified)
}

fn resolve_marker(
    base: &Path,
    path: &Path,
    platform: QuadriFlowPlatform,
    verified: &VerifiedQuadriFlowManifest<'_>,
) -> Result<Option<PathBuf>, QuadriFlowInstallError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 4096 {
        return Err(QuadriFlowInstallError::ActivationMetadata);
    }
    let marker_file = File::open(path)?;
    let mut bytes = Vec::new();
    marker_file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(QuadriFlowInstallError::ActivationMetadata);
    }
    let marker: ActiveMarker =
        serde_json::from_slice(&bytes).map_err(|_| QuadriFlowInstallError::ActivationMetadata)?;
    validate_quadriflow_release_manifest(verified.manifest)?;
    let Some(expected_release_tag) = verified.manifest.release_tag.as_deref() else {
        return Ok(None);
    };
    let Some(artifact) = verified
        .manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == platform)
    else {
        return Ok(None);
    };
    if marker.schema_version != ACTIVE_MARKER_SCHEMA
        || marker.platform != platform
        || marker.release_tag != expected_release_tag
        || marker.executable_sha256 != artifact.payload.executable.sha256
        || marker.executable_sha256.len() != 64
        || !marker
            .executable_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !valid_tag(&marker.release_tag)
    {
        return Ok(None);
    }
    let versions_dir = base.join("versions");
    match symlink_metadata_optional(&versions_dir)? {
        Some(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
        _ => return Ok(None),
    }
    let version_dir = versions_dir.join(&marker.release_tag);
    match symlink_metadata_optional(&version_dir)? {
        Some(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
        _ => return Ok(None),
    }
    let executable_relative = artifact.payload.executable.path.as_str();
    if executable_relative.starts_with("bin/") {
        let bin_dir = version_dir.join("bin");
        match symlink_metadata_optional(&bin_dir)? {
            Some(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
            _ => return Ok(None),
        }
    }
    let executable = version_dir.join(executable_relative);
    let Some(executable_metadata) = symlink_metadata_optional(&executable)? else {
        return Ok(None);
    };
    if executable_metadata.file_type().is_symlink()
        || !executable_metadata.is_file()
        || !verify_file(
            &executable,
            artifact.payload.executable.size_bytes,
            &artifact.payload.executable.sha256,
            128 * 1024 * 1024,
        )?
    {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = executable_metadata.permissions().mode();
        if mode & 0o077 != 0 || mode & 0o111 == 0 {
            return Ok(None);
        }
    }
    Ok(Some(executable))
}

fn active_markers(directory: &Path) -> Result<Vec<PathBuf>, QuadriFlowInstallError> {
    let Some(metadata) = symlink_metadata_optional(directory)? else {
        return Ok(Vec::new());
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(QuadriFlowInstallError::UnsafeDirectory);
    }
    let mut markers = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if marker_sequence(name).is_some() {
            let metadata = entry.file_type()?;
            if metadata.is_symlink() || !metadata.is_file() {
                return Err(QuadriFlowInstallError::ActivationMetadata);
            }
            markers.push(entry.path());
        }
    }
    markers.sort_by(|left, right| {
        let left_sequence = left
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(marker_sequence)
            .unwrap_or(0);
        let right_sequence = right
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(marker_sequence)
            .unwrap_or(0);
        left_sequence
            .cmp(&right_sequence)
            .then_with(|| left.file_name().cmp(&right.file_name()))
    });
    Ok(markers)
}

fn marker_sequence(name: &str) -> Option<u64> {
    let value = name
        .strip_prefix("active-")?
        .strip_suffix(".json")?
        .split_once('-')?
        .0;
    if value.len() != 20 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn append_active_marker(
    directory: &Path,
    marker: &ActiveMarker,
) -> Result<(), QuadriFlowInstallError> {
    let current = active_markers(directory)?;
    let sequence = current
        .last()
        .and_then(|path| path.file_name()?.to_str())
        .and_then(marker_sequence)
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(QuadriFlowInstallError::ActivationMetadata)?;
    let identifier = Uuid::new_v4();
    let temporary = directory.join(format!(".pending-{identifier}"));
    let final_path = directory.join(format!("active-{sequence:020}-{identifier}.json"));
    let result = (|| {
        let bytes =
            serde_json::to_vec(marker).map_err(|_| QuadriFlowInstallError::ActivationMetadata)?;
        let mut file = create_private_file(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &final_path)?;
        sync_directory(directory);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn trim_active_history(directory: &Path) {
    let Ok(markers) = active_markers(directory) else {
        return;
    };
    for old in markers.iter().take(markers.len().saturating_sub(2)) {
        let _ = fs::remove_file(old);
    }
    sync_directory(directory);
}

fn acquire_lock(base: &Path) -> Result<InstallLock, QuadriFlowInstallError> {
    let path = base.join(".quadriflow-install.lock");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            let lock = InstallLock(path.clone());
            file.write_all(b"MeshHarbor QuadriFlow installer lock\n")?;
            file.sync_all()?;
            Ok(lock)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(QuadriFlowInstallError::Busy)
        }
        Err(error) => Err(error.into()),
    }
}

fn prepare_install_base(data_root: &Path) -> Result<PathBuf, QuadriFlowInstallError> {
    let metadata = fs::symlink_metadata(data_root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(QuadriFlowInstallError::UnsafeDirectory);
    }
    let root = data_root.canonicalize()?;
    let backends = root.join("backends");
    ensure_private_directory(&backends)?;
    let base = backends.join("quadriflow");
    ensure_private_directory(&base)?;
    Ok(base)
}

fn install_base_if_present(data_root: &Path) -> Result<Option<PathBuf>, QuadriFlowInstallError> {
    let Some(metadata) = symlink_metadata_optional(data_root)? else {
        return Ok(None);
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(QuadriFlowInstallError::UnsafeDirectory);
    }
    let backends = data_root.join("backends");
    let Some(backends_metadata) = symlink_metadata_optional(&backends)? else {
        return Ok(None);
    };
    if backends_metadata.file_type().is_symlink() || !backends_metadata.is_dir() {
        return Err(QuadriFlowInstallError::UnsafeDirectory);
    }
    let base = backends.join("quadriflow");
    let Some(base_metadata) = symlink_metadata_optional(&base)? else {
        return Ok(None);
    };
    if base_metadata.file_type().is_symlink() || !base_metadata.is_dir() {
        return Err(QuadriFlowInstallError::UnsafeDirectory);
    }
    Ok(Some(base))
}

fn ensure_private_directory(path: &Path) -> Result<(), QuadriFlowInstallError> {
    match symlink_metadata_optional(path)? {
        Some(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(QuadriFlowInstallError::UnsafeDirectory);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err(QuadriFlowInstallError::UnsafeDirectory);
                }
            }
        }
        None => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(path)?;
        }
    }
    Ok(())
}

fn create_private_file(path: &Path) -> Result<File, QuadriFlowInstallError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn set_installed_file_mode(path: &Path, executable: bool) -> Result<(), QuadriFlowInstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o700 } else { 0o600 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, executable);
    Ok(())
}

fn verify_file(
    path: &Path,
    expected_size: u64,
    expected_digest: &str,
    max_size: u64,
) -> Result<bool, QuadriFlowInstallError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != expected_size
        || metadata.len() == 0
        || metadata.len() > max_size
    {
        return Ok(false);
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut read = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        read = read.saturating_add(count as u64);
        if read > max_size {
            return Ok(false);
        }
        hasher.update(&buffer[..count]);
    }
    Ok(read == expected_size && format!("{:x}", hasher.finalize()) == expected_digest)
}

fn sync_directory(directory: &Path) {
    #[cfg(unix)]
    if let Ok(file) = File::open(directory) {
        let _ = file.sync_all();
    }
    #[cfg(not(unix))]
    let _ = directory;
}

fn symlink_metadata_optional(path: &Path) -> Result<Option<fs::Metadata>, QuadriFlowInstallError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn valid_tag(tag: &str) -> bool {
    tag.strip_prefix("quadriflow-v").is_some_and(|version| {
        !version.is_empty()
            && version.as_bytes()[0].is_ascii_digit()
            && tag.len() <= 100
            && tag
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            && !tag.contains("..")
    })
}

fn archive_error(message: &str) -> QuadriFlowInstallError {
    QuadriFlowInstallError::Archive(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        QuadriFlowArchive, QuadriFlowPayload, QuadriFlowPayloadFile, QuadriFlowSource,
        QuadriFlowWorkflowEvidence,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("meshharbor-qf-installer-{}", Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn artifact(
        platform: QuadriFlowPlatform,
        tag: &str,
        payload_bytes: [&[u8]; 5],
        archive: &[u8],
    ) -> QuadriFlowReleaseManifest {
        let payload = |path: &str, bytes: &[u8]| QuadriFlowPayloadFile {
            path: path.into(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size_bytes: bytes.len() as u64,
        };
        let artifact = QuadriFlowReleaseArtifact {
            platform,
            archive: QuadriFlowArchive {
                filename: platform.archive_filename_for_test().into(),
                format: QuadriFlowArchiveFormat::Tar,
                url: format!(
                    "https://github.com/syi0808/meshharbor/releases/download/{tag}/{}",
                    platform.archive_filename_for_test()
                ),
                sha256: format!("{:x}", Sha256::digest(archive)),
                size_bytes: archive.len() as u64,
            },
            workflow: QuadriFlowWorkflowEvidence {
                repository: "syi0808/meshharbor".into(),
                path: ".github/workflows/quadriflow-candidate-build.yml".into(),
                run_id: 1,
                run_attempt: 1,
                commit: "a".repeat(40),
            },
            payload: QuadriFlowPayload {
                executable: payload(platform.executable_path_for_test(), payload_bytes[0]),
                third_party_notices: payload("THIRD-PARTY-NOTICES.md", payload_bytes[1]),
                sbom: payload("sbom.spdx.json", payload_bytes[2]),
                provenance: payload("provenance.json", payload_bytes[3]),
                provenance_signature: payload("provenance.sigstore.json", payload_bytes[4]),
            },
            attestation_bundle_base64: BASE64.encode(payload_bytes[4]),
        };
        QuadriFlowReleaseManifest {
            schema_version: 1,
            backend: "quadriflow".into(),
            source: QuadriFlowSource {
                repository: "https://github.com/hjwdzh/QuadriFlow".into(),
                commit: "810b7a0967c35b0dc85b4464e3835e26a756c967".into(),
            },
            release_tag: Some(tag.into()),
            artifacts: vec![artifact],
        }
    }

    fn test_verified_manifest<'a>(
        manifest: &'a QuadriFlowReleaseManifest,
    ) -> VerifiedQuadriFlowManifest<'a> {
        validate_quadriflow_release_manifest(manifest).unwrap();
        // Test-only capability: production code cannot construct this token.
        VerifiedQuadriFlowManifest { manifest }
    }

    fn install_test_manifest_with_fetcher<F>(
        data_root: &Path,
        manifest: &QuadriFlowReleaseManifest,
        platform: QuadriFlowPlatform,
        fetcher: F,
    ) -> Result<QuadriFlowInstallResult, QuadriFlowInstallError>
    where
        F: FnOnce(&str, u64, &Path) -> Result<(), QuadriFlowInstallError>,
    {
        let verified = test_verified_manifest(manifest);
        install_manifest_with_fetcher(data_root, &verified, platform, fetcher)
    }

    fn make_tar(entries: &[(&str, &[u8], u8)]) -> Vec<u8> {
        let mut archive = Vec::new();
        for (name, contents, kind) in entries {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            write_octal(&mut header[100..108], 0o755);
            write_octal(&mut header[108..116], 0);
            write_octal(&mut header[116..124], 0);
            write_octal(&mut header[124..136], contents.len() as u64);
            write_octal(&mut header[136..148], 0);
            header[148..156].fill(b' ');
            header[156] = *kind;
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");
            let checksum: u64 = header.iter().map(|byte| *byte as u64).sum();
            write_checksum(&mut header[148..156], checksum);
            archive.extend_from_slice(&header);
            archive.extend_from_slice(contents);
            let padding = (512 - (contents.len() as u64 % 512)) % 512;
            archive.resize(archive.len() + padding as usize, 0);
        }
        archive.resize(archive.len() + 1024, 0);
        archive
    }

    fn write_octal(field: &mut [u8], value: u64) {
        field.fill(b'0');
        let digits = format!("{value:o}");
        let start = field.len() - digits.len() - 1;
        field[start..start + digits.len()].copy_from_slice(digits.as_bytes());
        field[field.len() - 1] = 0;
    }

    fn write_checksum(field: &mut [u8], value: u64) {
        field.fill(0);
        let digits = format!("{value:06o}");
        field[..6].copy_from_slice(digits.as_bytes());
        field[6] = 0;
        field[7] = b' ';
    }

    fn test_package(
        platform: QuadriFlowPlatform,
        tag: &str,
        executable: &[u8],
    ) -> (QuadriFlowReleaseManifest, Vec<u8>) {
        let notices = b"notices";
        let sbom = b"{}";
        let provenance = b"{}";
        let signature = b"signature";
        let executable_path = platform.executable_path_for_test();
        let entries: [(&str, &[u8], u8); 6] = [
            ("bin/", &b""[..], b'5'),
            (executable_path, executable, b'0'),
            ("THIRD-PARTY-NOTICES.md", notices, b'0'),
            ("sbom.spdx.json", sbom, b'0'),
            ("provenance.json", provenance, b'0'),
            ("provenance.sigstore.json", signature, b'0'),
        ];
        let archive = make_tar(&entries);
        let manifest = artifact(
            platform,
            tag,
            [executable, notices, sbom, provenance, signature],
            &archive,
        );
        (manifest, archive)
    }

    #[test]
    fn empty_bundled_manifest_stops_before_network_or_disk_writes() {
        let root = std::env::temp_dir().join(format!("meshharbor-qf-empty-{}", Uuid::new_v4()));
        assert!(matches!(
            install_bundled_quadriflow_sidecar(&root),
            Err(QuadriFlowInstallError::NoReleaseArtifact)
        ));
        assert!(matches!(
            rollback_quadriflow_sidecar(&root, QuadriFlowPlatform::LinuxX86_64),
            Err(QuadriFlowInstallError::NoReleaseArtifact)
        ));
        assert!(installed_quadriflow_sidecar(&root, QuadriFlowPlatform::LinuxX86_64).is_none());
        assert!(!root.exists());
    }

    #[test]
    fn nonempty_release_manifest_stays_blocked_without_valid_signature() {
        let platform = QuadriFlowPlatform::LinuxX86_64;
        let (manifest, _) = test_package(platform, "quadriflow-v1.0.0", b"binary");
        assert!(matches!(
            verified_release_manifest(&manifest),
            Err(QuadriFlowInstallError::ReleaseVerificationFailed)
        ));
    }

    #[test]
    fn installs_only_hashed_ustar_payloads_and_supports_rollback() {
        let root = TestRoot::new();
        let platform = QuadriFlowPlatform::LinuxX86_64;
        let (first, first_archive) = test_package(platform, "quadriflow-v1.0.0", b"binary-one");
        let result =
            install_test_manifest_with_fetcher(&root.0, &first, platform, |_, _, destination| {
                fs::write(destination, &first_archive)?;
                Ok(())
            })
            .unwrap();
        assert!(!result.already_installed);
        assert_eq!(fs::read(&result.executable).unwrap(), b"binary-one");
        assert!(
            resolve_installed(&root.0, platform, &test_verified_manifest(&first))
                .unwrap()
                .is_some()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&result.executable)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
            assert_ne!(mode & 0o100, 0);
        }

        let (second, second_archive) = test_package(platform, "quadriflow-v1.1.0", b"binary-two");
        let updated =
            install_test_manifest_with_fetcher(&root.0, &second, platform, |_, _, destination| {
                fs::write(destination, &second_archive)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(fs::read(&updated.executable).unwrap(), b"binary-two");
        let rolled_back =
            rollback_with_verified_manifest(&root.0, platform, &test_verified_manifest(&first))
                .unwrap();
        assert_eq!(fs::read(rolled_back).unwrap(), b"binary-one");

        let active = root.0.join("backends/quadriflow/active");
        let after_rollback = active_markers(&active).unwrap();
        assert_eq!(after_rollback.len(), 2);
        let rollback_generation = after_rollback
            .last()
            .and_then(|path| path.file_name()?.to_str())
            .and_then(marker_sequence)
            .unwrap();

        let reactivated =
            install_test_manifest_with_fetcher(&root.0, &second, platform, |_, _, _| {
                panic!("an existing verified version must not be downloaded again")
            })
            .unwrap();
        assert!(reactivated.already_installed);
        let after_reactivation = active_markers(&active).unwrap();
        let reactivation_generation = after_reactivation
            .last()
            .and_then(|path| path.file_name()?.to_str())
            .and_then(marker_sequence)
            .unwrap();
        assert!(reactivation_generation > rollback_generation);

        let rolled_back_again =
            rollback_with_verified_manifest(&root.0, platform, &test_verified_manifest(&first))
                .unwrap();
        assert_eq!(fs::read(rolled_back_again).unwrap(), b"binary-one");
    }

    #[test]
    fn rejects_traversal_link_duplicate_and_bad_payload_hashes() {
        let root = TestRoot::new();
        let platform = QuadriFlowPlatform::LinuxX86_64;
        let (manifest, _) = test_package(platform, "quadriflow-v1.0.0", b"binary");
        let artifact = &manifest.artifacts[0];
        for entries in [
            vec![("../escape", &b"x"[..], b'0')],
            vec![("bin/quadriflow", &b"x"[..], b'2')],
            vec![
                ("bin/quadriflow", &b"binary"[..], b'0'),
                ("bin/quadriflow", &b"binary"[..], b'0'),
            ],
        ] {
            let tar = make_tar(&entries);
            let mut malformed_artifact = artifact.clone();
            malformed_artifact.archive.size_bytes = tar.len() as u64;
            let staged = PrivateTempDir::create(&root.0, "bad-archive").unwrap();
            assert!(
                extract_ustar(
                    &write_temp_archive(&root.0, &tar),
                    &staged.path,
                    &malformed_artifact
                )
                .is_err()
            );
        }

        let mut wrong = manifest.clone();
        wrong.artifacts[0].payload.executable.sha256 = "0".repeat(64);
        let tar = make_tar(&[
            ("bin/quadriflow", &b"binary"[..], b'0'),
            ("THIRD-PARTY-NOTICES.md", &b"notices"[..], b'0'),
            ("sbom.spdx.json", &b"{}"[..], b'0'),
            ("provenance.json", &b"{}"[..], b'0'),
            ("provenance.sigstore.json", &b"signature"[..], b'0'),
        ]);
        wrong.artifacts[0].archive.size_bytes = tar.len() as u64;
        let staged = PrivateTempDir::create(&root.0, "bad-hash").unwrap();
        assert!(
            extract_ustar(
                &write_temp_archive(&root.0, &tar),
                &staged.path,
                &wrong.artifacts[0]
            )
            .is_err()
        );
    }

    #[test]
    fn archive_digest_failure_never_activates_or_overwrites() {
        let root = TestRoot::new();
        let platform = QuadriFlowPlatform::LinuxX86_64;
        let (manifest, archive) = test_package(platform, "quadriflow-v1.0.0", b"binary");
        let mut fetched_bytes = archive;
        fetched_bytes[0] ^= 1;
        let result = install_test_manifest_with_fetcher(
            &root.0,
            &manifest,
            platform,
            |_, _, destination| {
                fs::write(destination, &fetched_bytes)?;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(
            resolve_installed(&root.0, platform, &test_verified_manifest(&manifest))
                .unwrap()
                .is_none()
        );
        let base = root.0.join("backends/quadriflow");
        assert!(active_markers(&base.join("active")).unwrap().is_empty());
    }

    #[test]
    fn missing_and_unexpected_archive_members_are_rejected() {
        let root = TestRoot::new();
        let platform = QuadriFlowPlatform::LinuxX86_64;
        let (manifest, _) = test_package(platform, "quadriflow-v1.0.0", b"binary");
        let artifact = &manifest.artifacts[0];
        for entries in [
            vec![("bin/quadriflow", &b"binary"[..], b'0')],
            vec![
                ("bin/quadriflow", &b"binary"[..], b'0'),
                ("unexpected.txt", &b"extra"[..], b'0'),
            ],
        ] {
            let archive = make_tar(&entries);
            let mut test_artifact = artifact.clone();
            test_artifact.archive.size_bytes = archive.len() as u64;
            let stage = PrivateTempDir::create(&root.0, "member-set").unwrap();
            assert!(
                extract_ustar(
                    &write_temp_archive(&root.0, &archive),
                    &stage.path,
                    &test_artifact
                )
                .is_err()
            );
        }
    }

    #[test]
    fn create_new_lock_blocks_a_second_installer() {
        let root = TestRoot::new();
        let base = prepare_install_base(&root.0).unwrap();
        let _lock = acquire_lock(&base).unwrap();
        let (manifest, archive) = test_package(
            QuadriFlowPlatform::LinuxX86_64,
            "quadriflow-v1.0.0",
            b"binary",
        );
        let mut fetched = false;
        let result = install_test_manifest_with_fetcher(
            &root.0,
            &manifest,
            QuadriFlowPlatform::LinuxX86_64,
            |_, _, destination| {
                fetched = true;
                fs::write(destination, &archive)?;
                Ok(())
            },
        );
        assert!(matches!(result, Err(QuadriFlowInstallError::Busy)));
        assert!(!fetched);
    }

    #[test]
    fn corrupted_active_executable_is_not_returned() {
        let root = TestRoot::new();
        let platform = QuadriFlowPlatform::LinuxX86_64;
        let (manifest, archive) = test_package(platform, "quadriflow-v1.0.0", b"binary");
        let result = install_test_manifest_with_fetcher(
            &root.0,
            &manifest,
            platform,
            |_, _, destination| {
                fs::write(destination, &archive)?;
                Ok(())
            },
        )
        .unwrap();
        fs::write(&result.executable, b"changed").unwrap();
        assert!(
            resolve_installed(&root.0, platform, &test_verified_manifest(&manifest))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn activation_marker_cannot_self_authorize_a_payload_outside_the_manifest() {
        let root = TestRoot::new();
        let platform = QuadriFlowPlatform::LinuxX86_64;
        let (manifest, archive) = test_package(platform, "quadriflow-v1.0.0", b"approved");
        let result = install_test_manifest_with_fetcher(
            &root.0,
            &manifest,
            platform,
            |_, _, destination| {
                fs::write(destination, &archive)?;
                Ok(())
            },
        )
        .unwrap();

        fs::write(&result.executable, b"unapproved").unwrap();
        let forged_marker = ActiveMarker {
            schema_version: ACTIVE_MARKER_SCHEMA,
            release_tag: "quadriflow-v1.0.0".into(),
            platform,
            executable_sha256: format!("{:x}", Sha256::digest(b"unapproved")),
        };
        append_active_marker(&root.0.join("backends/quadriflow/active"), &forged_marker).unwrap();

        assert!(
            resolve_installed(&root.0, platform, &test_verified_manifest(&manifest))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_bad_ustar_checksum_truncation_and_nonzero_padding() {
        let root = TestRoot::new();
        let platform = QuadriFlowPlatform::LinuxX86_64;
        let (manifest, valid_archive) = test_package(platform, "quadriflow-v1.0.0", b"binary");
        let artifact = &manifest.artifacts[0];

        let mut bad_checksum = valid_archive.clone();
        bad_checksum[0] ^= 1;
        let mut bad_artifact = artifact.clone();
        bad_artifact.archive.size_bytes = bad_checksum.len() as u64;
        let stage = PrivateTempDir::create(&root.0, "bad-checksum").unwrap();
        assert!(
            extract_ustar(
                &write_temp_archive(&root.0, &bad_checksum),
                &stage.path,
                &bad_artifact
            )
            .is_err()
        );

        let truncated = &valid_archive[..valid_archive.len() - 512];
        let mut truncated_artifact = artifact.clone();
        truncated_artifact.archive.size_bytes = truncated.len() as u64;
        let stage = PrivateTempDir::create(&root.0, "truncated").unwrap();
        assert!(
            extract_ustar(
                &write_temp_archive(&root.0, truncated),
                &stage.path,
                &truncated_artifact
            )
            .is_err()
        );

        let mut bad_padding = valid_archive.clone();
        // First member is the empty bin directory; the next member is the executable.
        let executable_data_offset = 512 + 512;
        bad_padding[executable_data_offset + b"binary".len()] = 1;
        let mut padding_artifact = artifact.clone();
        padding_artifact.archive.size_bytes = bad_padding.len() as u64;
        let stage = PrivateTempDir::create(&root.0, "bad-padding").unwrap();
        assert!(
            extract_ustar(
                &write_temp_archive(&root.0, &bad_padding),
                &stage.path,
                &padding_artifact
            )
            .is_err()
        );
    }

    #[test]
    fn redirect_url_must_remain_on_allowlisted_https_hosts() {
        assert!(trusted_effective_url(
            "https://release-assets.githubusercontent.com/path?token=opaque"
        ));
        assert!(trusted_effective_url(
            "https://github.com/owner/repo/release"
        ));
        assert!(!trusted_effective_url(
            "http://github.com/owner/repo/release"
        ));
        assert!(!trusted_effective_url("https://evil.example/release"));
        assert!(!trusted_effective_url(
            "https://release-assets.githubusercontent.com.evil.example/file"
        ));
        assert!(!trusted_effective_url(
            "https://release-assets.githubusercontent.com:443/file"
        ));
    }

    fn write_temp_archive(root: &Path, bytes: &[u8]) -> PathBuf {
        let path = root.join(format!("archive-{}.tar", Uuid::new_v4()));
        fs::write(&path, bytes).unwrap();
        path
    }

    trait TestPlatformPaths {
        fn archive_filename_for_test(self) -> &'static str;
        fn executable_path_for_test(self) -> &'static str;
    }

    impl TestPlatformPaths for QuadriFlowPlatform {
        fn archive_filename_for_test(self) -> &'static str {
            match self {
                QuadriFlowPlatform::LinuxX86_64 => "quadriflow-linux-x86_64.tar",
                QuadriFlowPlatform::MacosAarch64 => "quadriflow-macos-aarch64.tar",
                QuadriFlowPlatform::WindowsX86_64 => "quadriflow-windows-x86_64.tar",
            }
        }

        fn executable_path_for_test(self) -> &'static str {
            match self {
                QuadriFlowPlatform::WindowsX86_64 => "bin/quadriflow.exe",
                QuadriFlowPlatform::LinuxX86_64 | QuadriFlowPlatform::MacosAarch64 => {
                    "bin/quadriflow"
                }
            }
        }
    }
}
