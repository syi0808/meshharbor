//! Structural validation for the QuadriFlow sidecar release manifest.
//!
//! Network and filesystem effects live in the separate quadriflow_installer
//! module. This module validates manifest structure and exact bundle-byte pins;
//! the separate quadriflow_attestation module performs cryptographic and claim
//! verification. A non-empty manifest is not legal, provenance, or release
//! approval; the embedded manifest remains empty until its release gates pass.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub(crate) const PINNED_SOURCE_REPOSITORY: &str = "https://github.com/hjwdzh/QuadriFlow";
pub(crate) const PINNED_SOURCE_COMMIT: &str = "810b7a0967c35b0dc85b4464e3835e26a756c967";
const RELEASE_REPOSITORY: &str = "syi0808/meshharbor";
const RELEASE_BASE_URL: &str = "https://github.com/syi0808/meshharbor/releases/download/";
const BUILD_WORKFLOW_PATH: &str = ".github/workflows/quadriflow-candidate-build.yml";
const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_NOTICES_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SBOM_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PROVENANCE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_INLINE_BUNDLE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuadriFlowReleaseManifest {
    pub schema_version: u32,
    pub backend: String,
    pub source: QuadriFlowSource,
    #[serde(default)]
    pub release_tag: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<QuadriFlowReleaseArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuadriFlowSource {
    pub repository: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuadriFlowReleaseArtifact {
    pub platform: QuadriFlowPlatform,
    pub archive: QuadriFlowArchive,
    pub workflow: QuadriFlowWorkflowEvidence,
    pub payload: QuadriFlowPayload,
    /// Exact base64-encoded Sigstore bundle bytes, pinned into the signed
    /// application manifest so the signature can be checked before download.
    pub attestation_bundle_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuadriFlowArchive {
    pub filename: String,
    pub format: QuadriFlowArchiveFormat,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuadriFlowArchiveFormat {
    Tar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuadriFlowWorkflowEvidence {
    pub repository: String,
    pub path: String,
    pub run_id: u64,
    /// GitHub Actions attempt number included in the signed SLSA invocation.
    pub run_attempt: u64,
    /// Full lowercase commit SHA of the MeshHarbor workflow checkout.
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuadriFlowPayload {
    pub executable: QuadriFlowPayloadFile,
    pub third_party_notices: QuadriFlowPayloadFile,
    pub sbom: QuadriFlowPayloadFile,
    pub provenance: QuadriFlowPayloadFile,
    pub provenance_signature: QuadriFlowPayloadFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuadriFlowPayloadFile {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuadriFlowPlatform {
    #[serde(rename = "linux-x86_64")]
    LinuxX86_64,
    #[serde(rename = "macos-aarch64")]
    MacosAarch64,
    #[serde(rename = "windows-x86_64")]
    WindowsX86_64,
}

impl QuadriFlowPlatform {
    fn archive_filename(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "quadriflow-linux-x86_64.tar",
            Self::MacosAarch64 => "quadriflow-macos-aarch64.tar",
            Self::WindowsX86_64 => "quadriflow-windows-x86_64.tar",
        }
    }

    fn archive_format(self) -> QuadriFlowArchiveFormat {
        QuadriFlowArchiveFormat::Tar
    }

    fn executable_path(self) -> &'static str {
        match self {
            Self::WindowsX86_64 => "bin/quadriflow.exe",
            Self::LinuxX86_64 | Self::MacosAarch64 => "bin/quadriflow",
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QuadriFlowManifestError {
    #[error("QuadriFlow release manifest is empty or exceeds the 2 MiB limit.")]
    ManifestSize,
    #[error("QuadriFlow release manifest JSON is invalid: {0}")]
    Json(String),
    #[error("QuadriFlow release manifest has an unsupported schema version.")]
    SchemaVersion,
    #[error("QuadriFlow manifest does not match the pinned QuadriFlow source.")]
    SourcePin,
    #[error("QuadriFlow release tag or artifact list is inconsistent.")]
    ReleaseTag,
    #[error("QuadriFlow release manifest contains a duplicate platform.")]
    DuplicatePlatform,
    #[error("QuadriFlow artifact has invalid release or workflow metadata.")]
    ArtifactMetadata,
    #[error("QuadriFlow artifact exceeds the configured package size limit.")]
    ArtifactSize,
    #[error("QuadriFlow artifact contains a malformed SHA-256 digest.")]
    Digest,
    #[error("QuadriFlow attestation bundle is invalid or mismatched with its payload pin.")]
    AttestationBundle,
    #[error("QuadriFlow release payload has an unexpected size or SHA-256 digest.")]
    PayloadDigest,
}

/// Parses and validates the manifest's structure and pinned release metadata.
///
/// This does not cryptographically verify a Sigstore attestation, inspect
/// archive entries, download or install files, or imply that the release is
/// approved for use.
pub fn parse_quadriflow_release_manifest(
    bytes: &[u8],
) -> Result<QuadriFlowReleaseManifest, QuadriFlowManifestError> {
    if bytes.is_empty() || bytes.len() > MAX_MANIFEST_BYTES {
        return Err(QuadriFlowManifestError::ManifestSize);
    }
    let manifest: QuadriFlowReleaseManifest = serde_json::from_slice(bytes)
        .map_err(|error| QuadriFlowManifestError::Json(error.to_string()))?;
    validate_quadriflow_release_manifest(&manifest)?;
    Ok(manifest)
}

/// Validates structural constraints for a decoded manifest without side effects.
pub fn validate_quadriflow_release_manifest(
    manifest: &QuadriFlowReleaseManifest,
) -> Result<(), QuadriFlowManifestError> {
    if manifest.schema_version != 1 || manifest.backend != "quadriflow" {
        return Err(QuadriFlowManifestError::SchemaVersion);
    }
    if manifest.source.repository != PINNED_SOURCE_REPOSITORY
        || manifest.source.commit != PINNED_SOURCE_COMMIT
    {
        return Err(QuadriFlowManifestError::SourcePin);
    }

    if manifest.artifacts.is_empty() {
        return if manifest.release_tag.is_none() {
            Ok(())
        } else {
            Err(QuadriFlowManifestError::ReleaseTag)
        };
    }
    let Some(tag) = manifest.release_tag.as_deref() else {
        return Err(QuadriFlowManifestError::ReleaseTag);
    };
    if !valid_release_tag(tag) || manifest.artifacts.len() > 3 {
        return Err(QuadriFlowManifestError::ReleaseTag);
    }

    let mut platforms = std::collections::HashSet::new();
    for artifact in &manifest.artifacts {
        if !platforms.insert(artifact.platform) {
            return Err(QuadriFlowManifestError::DuplicatePlatform);
        }
        validate_artifact(tag, artifact)?;
    }
    Ok(())
}

/// Returns the embedded release manifest after structural validation.
/// An empty manifest is valid but not installable; the installer enforces
/// installability separately after validating the listed release evidence.
pub fn bundled_quadriflow_release_manifest()
-> Result<QuadriFlowReleaseManifest, QuadriFlowManifestError> {
    parse_quadriflow_release_manifest(include_bytes!("../../../registry/backends/quadriflow.json"))
}

/// Verifies exact byte length and SHA-256 for one manifest-listed package file.
/// This alone does not establish provenance, signature validity, or safety.
pub fn verify_quadriflow_payload_file(
    bytes: &[u8],
    expected_size_bytes: u64,
    expected_sha256: &str,
) -> Result<(), QuadriFlowManifestError> {
    if !valid_digest(expected_sha256) {
        return Err(QuadriFlowManifestError::Digest);
    }
    if expected_size_bytes == 0
        || expected_size_bytes > MAX_ARCHIVE_BYTES
        || bytes.len() as u64 != expected_size_bytes
    {
        return Err(QuadriFlowManifestError::PayloadDigest);
    }
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != expected_sha256 {
        return Err(QuadriFlowManifestError::PayloadDigest);
    }
    Ok(())
}

fn validate_artifact(
    tag: &str,
    artifact: &QuadriFlowReleaseArtifact,
) -> Result<(), QuadriFlowManifestError> {
    let filename = artifact.platform.archive_filename();
    if artifact.archive.filename != filename
        || artifact.archive.format != artifact.platform.archive_format()
        || artifact.archive.url != format!("{RELEASE_BASE_URL}{tag}/{filename}")
        || !artifact.archive.url.is_ascii()
        || artifact
            .archive
            .url
            .chars()
            .any(|c| matches!(c, '?' | '#' | '%' | '\\'))
        || !valid_digest(&artifact.archive.sha256)
        || artifact.archive.size_bytes == 0
        || artifact.archive.size_bytes > MAX_ARCHIVE_BYTES
    {
        return Err(QuadriFlowManifestError::ArtifactMetadata);
    }
    if artifact.workflow.repository != RELEASE_REPOSITORY
        || artifact.workflow.path != BUILD_WORKFLOW_PATH
        || artifact.workflow.run_id == 0
        || artifact.workflow.run_attempt == 0
        || !valid_commit(&artifact.workflow.commit)
    {
        return Err(QuadriFlowManifestError::ArtifactMetadata);
    }

    validate_payload_file(
        &artifact.payload.executable,
        artifact.platform.executable_path(),
        MAX_BINARY_BYTES,
    )?;
    validate_payload_file(
        &artifact.payload.third_party_notices,
        "THIRD-PARTY-NOTICES.md",
        MAX_NOTICES_BYTES,
    )?;
    validate_payload_file(&artifact.payload.sbom, "sbom.spdx.json", MAX_SBOM_BYTES)?;
    validate_payload_file(
        &artifact.payload.provenance,
        "provenance.json",
        MAX_PROVENANCE_BYTES,
    )?;
    validate_payload_file(
        &artifact.payload.provenance_signature,
        "provenance.sigstore.json",
        MAX_SIGNATURE_BYTES,
    )?;
    validate_attestation_bundle_pin(artifact)?;
    Ok(())
}

fn validate_attestation_bundle_pin(
    artifact: &QuadriFlowReleaseArtifact,
) -> Result<(), QuadriFlowManifestError> {
    let encoded = artifact.attestation_bundle_base64.as_bytes();
    if encoded.is_empty() || encoded.len() > MAX_INLINE_BUNDLE_BYTES.div_ceil(3) * 4 {
        return Err(QuadriFlowManifestError::AttestationBundle);
    }
    let bytes = BASE64
        .decode(encoded)
        .map_err(|_| QuadriFlowManifestError::AttestationBundle)?;
    if bytes.is_empty()
        || bytes.len() > MAX_INLINE_BUNDLE_BYTES
        || BASE64.encode(&bytes) != artifact.attestation_bundle_base64
        || bytes.len() as u64 != artifact.payload.provenance_signature.size_bytes
        || !valid_digest(&artifact.payload.provenance_signature.sha256)
        || format!("{:x}", Sha256::digest(&bytes)) != artifact.payload.provenance_signature.sha256
    {
        return Err(QuadriFlowManifestError::AttestationBundle);
    }
    Ok(())
}

fn validate_payload_file(
    file: &QuadriFlowPayloadFile,
    expected_path: &str,
    max_bytes: u64,
) -> Result<(), QuadriFlowManifestError> {
    if file.path != expected_path {
        return Err(QuadriFlowManifestError::ArtifactMetadata);
    }
    if file.size_bytes == 0 || file.size_bytes > max_bytes {
        return Err(QuadriFlowManifestError::ArtifactSize);
    }
    if !valid_digest(&file.sha256) {
        return Err(QuadriFlowManifestError::Digest);
    }
    Ok(())
}

fn valid_release_tag(tag: &str) -> bool {
    tag.strip_prefix("quadriflow-v").is_some_and(|version| {
        !version.is_empty()
            && version.as_bytes()[0].is_ascii_digit()
            && tag.len() <= 100
            && tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            && !tag.contains("..")
    })
}

fn valid_commit(commit: &str) -> bool {
    commit.len() == 40
        && commit
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_manifest() -> QuadriFlowReleaseManifest {
        QuadriFlowReleaseManifest {
            schema_version: 1,
            backend: "quadriflow".into(),
            source: QuadriFlowSource {
                repository: PINNED_SOURCE_REPOSITORY.into(),
                commit: PINNED_SOURCE_COMMIT.into(),
            },
            release_tag: None,
            artifacts: Vec::new(),
        }
    }

    fn test_artifact(platform: QuadriFlowPlatform) -> QuadriFlowReleaseArtifact {
        let filename = platform.archive_filename();
        let attestation_bytes = b"{}";
        let payload_file = |path: &str| QuadriFlowPayloadFile {
            path: path.into(),
            sha256: "a".repeat(64),
            size_bytes: 10,
        };
        QuadriFlowReleaseArtifact {
            platform,
            archive: QuadriFlowArchive {
                filename: filename.into(),
                format: platform.archive_format(),
                url: format!("{RELEASE_BASE_URL}quadriflow-v0.1.0/{filename}"),
                sha256: "b".repeat(64),
                size_bytes: 100,
            },
            workflow: QuadriFlowWorkflowEvidence {
                repository: RELEASE_REPOSITORY.into(),
                path: BUILD_WORKFLOW_PATH.into(),
                run_id: 1,
                run_attempt: 1,
                commit: "c".repeat(40),
            },
            payload: QuadriFlowPayload {
                executable: payload_file(platform.executable_path()),
                third_party_notices: payload_file("THIRD-PARTY-NOTICES.md"),
                sbom: payload_file("sbom.spdx.json"),
                provenance: payload_file("provenance.json"),
                provenance_signature: QuadriFlowPayloadFile {
                    path: "provenance.sigstore.json".into(),
                    sha256: format!("{:x}", Sha256::digest(attestation_bytes)),
                    size_bytes: attestation_bytes.len() as u64,
                },
            },
            attestation_bundle_base64: BASE64.encode(attestation_bytes),
        }
    }

    #[test]
    fn bundled_manifest_is_empty_and_valid() {
        let manifest = bundled_quadriflow_release_manifest().unwrap();
        assert!(manifest.artifacts.is_empty());
        assert!(manifest.release_tag.is_none());
    }

    #[test]
    fn parses_empty_manifest() {
        let manifest = empty_manifest();
        let bytes = serde_json::to_vec(&manifest).unwrap();
        assert_eq!(parse_quadriflow_release_manifest(&bytes).unwrap(), manifest);
    }

    #[test]
    fn accepts_structurally_valid_future_release_record() {
        let mut manifest = empty_manifest();
        manifest.release_tag = Some("quadriflow-v0.1.0".into());
        manifest.artifacts = vec![test_artifact(QuadriFlowPlatform::LinuxX86_64)];
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["artifacts"][0]["platform"], "linux-x86_64");
        assert_eq!(value["artifacts"][0]["archive"]["format"], "tar");
        assert_eq!(parse_quadriflow_release_manifest(&bytes).unwrap(), manifest);
    }

    #[test]
    fn rejects_unpinned_source() {
        let mut manifest = empty_manifest();
        manifest.source.commit.replace_range(..1, "0");
        assert_eq!(
            validate_quadriflow_release_manifest(&manifest),
            Err(QuadriFlowManifestError::SourcePin)
        );
    }

    #[test]
    fn rejects_release_tag_on_empty_manifest() {
        let mut manifest = empty_manifest();
        manifest.release_tag = Some("quadriflow-v0.1.0".into());
        assert_eq!(
            validate_quadriflow_release_manifest(&manifest),
            Err(QuadriFlowManifestError::ReleaseTag)
        );
    }

    #[test]
    fn rejects_unsupported_or_unsafe_release_tag() {
        for tag in [
            "v0.1.0",
            "quadriflow-v../x",
            "quadriflow-v1?x",
            "quadriflow-v1%2f",
        ] {
            let mut manifest = empty_manifest();
            manifest.release_tag = Some(tag.into());
            manifest.artifacts = vec![test_artifact(QuadriFlowPlatform::LinuxX86_64)];
            assert_eq!(
                validate_quadriflow_release_manifest(&manifest),
                Err(QuadriFlowManifestError::ReleaseTag)
            );
        }
    }

    #[test]
    fn rejects_duplicate_platforms() {
        let mut manifest = empty_manifest();
        manifest.release_tag = Some("quadriflow-v0.1.0".into());
        manifest.artifacts = vec![
            test_artifact(QuadriFlowPlatform::LinuxX86_64),
            test_artifact(QuadriFlowPlatform::LinuxX86_64),
        ];
        assert_eq!(
            validate_quadriflow_release_manifest(&manifest),
            Err(QuadriFlowManifestError::DuplicatePlatform)
        );
    }

    #[test]
    fn rejects_release_url_with_other_host_or_path() {
        let mut artifact = test_artifact(QuadriFlowPlatform::LinuxX86_64);
        artifact.archive.url = "https://example.invalid/quadriflow".into();
        let mut manifest = empty_manifest();
        manifest.release_tag = Some("quadriflow-v0.1.0".into());
        manifest.artifacts = vec![artifact];
        assert_eq!(
            validate_quadriflow_release_manifest(&manifest),
            Err(QuadriFlowManifestError::ArtifactMetadata)
        );
    }

    #[test]
    fn rejects_platform_filename_mismatch() {
        let mut artifact = test_artifact(QuadriFlowPlatform::LinuxX86_64);
        artifact.archive.filename = "quadriflow-macos-aarch64.tar.gz".into();
        let mut manifest = empty_manifest();
        manifest.release_tag = Some("quadriflow-v0.1.0".into());
        manifest.artifacts = vec![artifact];
        assert_eq!(
            validate_quadriflow_release_manifest(&manifest),
            Err(QuadriFlowManifestError::ArtifactMetadata)
        );
    }

    #[test]
    fn rejects_unexpected_workflow_provenance() {
        let mut artifact = test_artifact(QuadriFlowPlatform::WindowsX86_64);
        artifact.workflow.commit = "not-a-full-commit".into();
        let mut manifest = empty_manifest();
        manifest.release_tag = Some("quadriflow-v0.1.0".into());
        manifest.artifacts = vec![artifact];
        assert_eq!(
            validate_quadriflow_release_manifest(&manifest),
            Err(QuadriFlowManifestError::ArtifactMetadata)
        );
    }

    #[test]
    fn rejects_missing_payload_notice_or_sbom() {
        for mutate in ["notice", "sbom", "provenance", "signature"] {
            let mut artifact = test_artifact(QuadriFlowPlatform::LinuxX86_64);
            match mutate {
                "notice" => artifact.payload.third_party_notices.path = "NOTICE".into(),
                "sbom" => artifact.payload.sbom.path = "sbom.json".into(),
                "provenance" => artifact.payload.provenance.path = "attestation.json".into(),
                _ => artifact.payload.provenance_signature.path = "sig".into(),
            }
            let mut manifest = empty_manifest();
            manifest.release_tag = Some("quadriflow-v0.1.0".into());
            manifest.artifacts = vec![artifact];
            assert_eq!(
                validate_quadriflow_release_manifest(&manifest),
                Err(QuadriFlowManifestError::ArtifactMetadata)
            );
        }
    }

    #[test]
    fn rejects_oversized_and_malformed_digests() {
        let mut artifact = test_artifact(QuadriFlowPlatform::LinuxX86_64);
        artifact.archive.size_bytes = MAX_ARCHIVE_BYTES + 1;
        let mut manifest = empty_manifest();
        manifest.release_tag = Some("quadriflow-v0.1.0".into());
        manifest.artifacts = vec![artifact.clone()];
        assert_eq!(
            validate_quadriflow_release_manifest(&manifest),
            Err(QuadriFlowManifestError::ArtifactMetadata)
        );

        artifact.archive.size_bytes = 1;
        artifact.archive.sha256 = "A".repeat(64);
        manifest.artifacts = vec![artifact];
        assert_eq!(
            validate_quadriflow_release_manifest(&manifest),
            Err(QuadriFlowManifestError::ArtifactMetadata)
        );
    }

    #[test]
    fn rejects_unknown_json_fields() {
        let mut value = serde_json::to_value(empty_manifest()).unwrap();
        value["unexpected"] = serde_json::Value::Bool(true);
        assert!(matches!(
            parse_quadriflow_release_manifest(&serde_json::to_vec(&value).unwrap()),
            Err(QuadriFlowManifestError::Json(_))
        ));
    }

    #[test]
    fn enforces_manifest_byte_limit() {
        assert_eq!(
            parse_quadriflow_release_manifest(&vec![b' '; MAX_MANIFEST_BYTES + 1]),
            Err(QuadriFlowManifestError::ManifestSize)
        );
    }

    #[test]
    fn verifies_exact_payload_size_and_digest() {
        let bytes = b"trusted-bytes";
        let digest = format!("{:x}", Sha256::digest(bytes));
        assert!(verify_quadriflow_payload_file(bytes, bytes.len() as u64, &digest).is_ok());
        assert_eq!(
            verify_quadriflow_payload_file(bytes, bytes.len() as u64 + 1, &digest),
            Err(QuadriFlowManifestError::PayloadDigest)
        );
        assert_eq!(
            verify_quadriflow_payload_file(b"", 0, &format!("{:x}", Sha256::digest(b""))),
            Err(QuadriFlowManifestError::PayloadDigest)
        );
        assert_eq!(
            verify_quadriflow_payload_file(bytes, bytes.len() as u64, &"0".repeat(64)),
            Err(QuadriFlowManifestError::PayloadDigest)
        );
    }
}
