//! Offline Sigstore verification and strict SLSA policy for managed QuadriFlow.
//!
//! A signed bundle is embedded byte-for-byte in the app-pinned manifest. This
//! lets us authenticate the executable digest and all provenance claims before
//! the installer makes a network or filesystem change. It is evidence of
//! provenance only, not release or license approval.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{
    Deserialize,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use sigstore_verify::{
    VerificationPolicy, VerificationResult, Verifier,
    trust_root::{SigstoreInstance, TrustedRoot},
    types::{Bundle, MediaType, Sha256Hash, SignatureContent},
};
use thiserror::Error;

use crate::{
    QuadriFlowPlatform, QuadriFlowReleaseArtifact, QuadriFlowWorkflowEvidence,
    quadriflow_manifest::PINNED_SOURCE_COMMIT,
};

const MESH_HARBOR_REPOSITORY_URI: &str = "https://github.com/syi0808/meshharbor";
const MESH_HARBOR_REPOSITORY_NAME: &str = "syi0808/meshharbor";
// MeshHarbor was created after GitHub's immutable-subject default rollout.
const MESH_HARBOR_TOKEN_SUBJECT: &str =
    "repo:syi0808@66503450/meshharbor@1454899411:ref:refs/heads/main";
const MESH_HARBOR_OWNER_URI: &str = "https://github.com/syi0808";
const MESH_HARBOR_REPOSITORY_ID: &str = "1454899411";
const MESH_HARBOR_OWNER_ID: &str = "66503450";
const SOURCE_REF: &str = "refs/heads/main";
const WORKFLOW_PATH: &str = ".github/workflows/quadriflow-candidate-build.yml";
const WORKFLOW_IDENTITY: &str = concat!(
    "https://github.com/syi0808/meshharbor",
    "/.github/workflows/quadriflow-candidate-build.yml@refs/heads/main"
);
const OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";
const WORKFLOW_BUILD_TYPE: &str =
    "https://slsa-framework.github.io/github-actions-buildtypes/workflow/v1";
const SLSA_PREDICATE_TYPE: &str = "https://slsa.dev/provenance/v1";
const IN_TOTO_STATEMENT_V1: &str = "https://in-toto.io/Statement/v1";
const IN_TOTO_PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";
const MAX_BUNDLE_BYTES: usize = 256 * 1024;

const PINNED_BUILD_RECIPE: &[u8] = include_bytes!("../../../scripts/quadriflow-platform-smoke.py");

#[derive(Debug, Error, PartialEq, Eq)]
#[error("QuadriFlow Sigstore signature or provenance does not match the pinned policy.")]
pub(crate) struct AttestationVerificationError;

type VerifyResult<T> = Result<T, AttestationVerificationError>;

/// Verifies the exact manifest-pinned bundle, certificate claims, artifact
/// digest, and signed SLSA statement. This is called before any installer I/O.
pub(crate) fn verify_release_artifact(
    source_commit: &str,
    artifact: &QuadriFlowReleaseArtifact,
) -> VerifyResult<()> {
    let bundle_bytes = BASE64
        .decode(&artifact.attestation_bundle_base64)
        .map_err(|_| AttestationVerificationError)?;
    if bundle_bytes.is_empty()
        || bundle_bytes.len() > MAX_BUNDLE_BYTES
        || BASE64.encode(&bundle_bytes) != artifact.attestation_bundle_base64
        || bundle_bytes.len() as u64 != artifact.payload.provenance_signature.size_bytes
        || sha256_hex(&bundle_bytes) != artifact.payload.provenance_signature.sha256
    {
        return Err(AttestationVerificationError);
    }

    let (bundle, verification) = verify_sigstore_bundle(
        &bundle_bytes,
        &artifact.payload.executable.sha256,
        WORKFLOW_IDENTITY,
        OIDC_ISSUER,
        SigstoreInstance::PublicGood,
    )?;
    if bundle.media_type != MediaType::Bundle0_3 {
        return Err(AttestationVerificationError);
    }
    validate_certificate_claims(
        &verification,
        &workflow_certificate_policy(&artifact.workflow),
    )?;

    let envelope = match &bundle.content {
        SignatureContent::DsseEnvelope(envelope) => envelope,
        _ => return Err(AttestationVerificationError),
    };
    if envelope.payload_type != IN_TOTO_PAYLOAD_TYPE {
        return Err(AttestationVerificationError);
    }
    let statement = parse_unique_json(envelope.payload.as_bytes())?;
    validate_slsa_statement(&statement, source_commit, artifact)
}

#[derive(Debug, Clone, Copy)]
struct CertificatePolicy<'a> {
    identity: &'a str,
    issuer: &'a str,
    repository_uri: &'a str,
    repository_name: &'a str,
    repository_id: &'a str,
    owner_uri: &'a str,
    owner_id: &'a str,
    source_ref: &'a str,
    workflow_commit: &'a str,
    run_id: u64,
    run_attempt: u64,
    trigger: &'a str,
    token_subject: TokenSubjectPolicy<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenSubjectPolicy<'a> {
    Required(&'a str),
    #[cfg(test)]
    AbsentInHistoricalFixture,
}

fn workflow_certificate_policy(evidence: &QuadriFlowWorkflowEvidence) -> CertificatePolicy<'_> {
    CertificatePolicy {
        identity: WORKFLOW_IDENTITY,
        issuer: OIDC_ISSUER,
        repository_uri: MESH_HARBOR_REPOSITORY_URI,
        repository_name: MESH_HARBOR_REPOSITORY_NAME,
        repository_id: MESH_HARBOR_REPOSITORY_ID,
        owner_uri: MESH_HARBOR_OWNER_URI,
        owner_id: MESH_HARBOR_OWNER_ID,
        source_ref: SOURCE_REF,
        workflow_commit: &evidence.commit,
        run_id: evidence.run_id,
        run_attempt: evidence.run_attempt,
        trigger: "workflow_dispatch",
        token_subject: TokenSubjectPolicy::Required(MESH_HARBOR_TOKEN_SUBJECT),
    }
}

fn verify_sigstore_bundle(
    bundle_bytes: &[u8],
    artifact_sha256: &str,
    identity: &str,
    issuer: &str,
    instance: SigstoreInstance,
) -> VerifyResult<(Bundle, VerificationResult)> {
    if bundle_bytes.is_empty() || bundle_bytes.len() > MAX_BUNDLE_BYTES {
        return Err(AttestationVerificationError);
    }
    // Reject duplicate JSON keys before the Sigstore parser sees the bytes.
    let _ = parse_unique_json(bundle_bytes)?;
    let json = std::str::from_utf8(bundle_bytes).map_err(|_| AttestationVerificationError)?;
    let bundle = Bundle::from_json(json).map_err(|_| AttestationVerificationError)?;
    if bundle.media_type != MediaType::Bundle0_3 {
        return Err(AttestationVerificationError);
    }

    let trusted_root =
        TrustedRoot::from_embedded(instance).map_err(|_| AttestationVerificationError)?;
    let verifier = Verifier::new(&trusted_root).map_err(|_| AttestationVerificationError)?;
    let artifact_digest =
        Sha256Hash::from_hex(artifact_sha256).map_err(|_| AttestationVerificationError)?;
    let policy = VerificationPolicy::new(identity, issuer);
    let result = verifier
        .verify(artifact_digest, &bundle, &policy)
        .map_err(|_| AttestationVerificationError)?;

    // Do not accept a successful result produced by a weaker verification
    // policy or a bundle missing its authenticated timestamp/transparency data.
    if !result.certificate_verified()
        || !result.sct_verified()
        || !result.tlog_verified()
        || !result.identity_policy_checked()
        || result.identity().map(|value| value.as_str()) != Some(identity)
        || result.issuer() != Some(issuer)
        || result.verified_timestamps().is_empty()
    {
        return Err(AttestationVerificationError);
    }
    Ok((bundle, result))
}

fn validate_certificate_claims(
    result: &VerificationResult,
    expected: &CertificatePolicy<'_>,
) -> VerifyResult<()> {
    let certificate = result.certificate().ok_or(AttestationVerificationError)?;
    let claims = &certificate.ci_claims;
    let expected_run = format!(
        "{}/actions/runs/{}/attempts/{}",
        expected.repository_uri, expected.run_id, expected.run_attempt
    );
    let token_subject_matches = match expected.token_subject {
        TokenSubjectPolicy::Required(value) => claims.token_subject.as_deref() == Some(value),
        #[cfg(test)]
        TokenSubjectPolicy::AbsentInHistoricalFixture => claims.token_subject.is_none(),
    };

    let required = [
        (claims.build_signer_uri.as_deref(), expected.identity),
        (
            claims.build_signer_digest.as_deref(),
            expected.workflow_commit,
        ),
        (claims.runner_environment.as_deref(), "github-hosted"),
        (
            claims.source_repository_uri.as_deref(),
            expected.repository_uri,
        ),
        (
            claims.source_repository_digest.as_deref(),
            expected.workflow_commit,
        ),
        (claims.source_repository_ref.as_deref(), expected.source_ref),
        (
            claims.source_repository_identifier.as_deref(),
            expected.repository_id,
        ),
        (
            claims.source_repository_owner_uri.as_deref(),
            expected.owner_uri,
        ),
        (
            claims.source_repository_owner_identifier.as_deref(),
            expected.owner_id,
        ),
        (claims.build_config_uri.as_deref(), expected.identity),
        (
            claims.build_config_digest.as_deref(),
            expected.workflow_commit,
        ),
        (claims.build_trigger.as_deref(), expected.trigger),
        (claims.run_invocation_uri.as_deref(), expected_run.as_str()),
        (
            claims.source_repository_visibility_at_signing.as_deref(),
            "public",
        ),
    ];
    if result.identity().map(|value| value.as_str()) != Some(expected.identity)
        || result.issuer() != Some(expected.issuer)
        || required
            .iter()
            .any(|(actual, value)| *actual != Some(*value))
        || !token_subject_matches
        || claims.deployment_environment.is_some()
    {
        return Err(AttestationVerificationError);
    }

    // Current GitHub certificates still carry these deprecated claims. Check
    // them if present, but do not depend on their continued availability.
    let deprecated = &claims.deprecated_github;
    let optional_legacy = [
        (deprecated.workflow_trigger.as_deref(), expected.trigger),
        (deprecated.workflow_sha.as_deref(), expected.workflow_commit),
        (
            deprecated.workflow_repository.as_deref(),
            expected.repository_name,
        ),
        (deprecated.workflow_ref.as_deref(), expected.source_ref),
    ];
    if optional_legacy
        .iter()
        .any(|(actual, value)| actual.is_some_and(|actual| actual != *value))
    {
        return Err(AttestationVerificationError);
    }
    Ok(())
}

fn validate_slsa_statement(
    statement: &Value,
    source_commit: &str,
    artifact: &QuadriFlowReleaseArtifact,
) -> VerifyResult<()> {
    let statement = object(statement)?;
    require_exact_keys(
        statement,
        &["_type", "subject", "predicateType", "predicate"],
    )?;
    require_string(statement, "_type", IN_TOTO_STATEMENT_V1)?;
    require_string(statement, "predicateType", SLSA_PREDICATE_TYPE)?;

    let subjects = statement
        .get("subject")
        .and_then(Value::as_array)
        .filter(|subjects| subjects.len() == 1)
        .ok_or(AttestationVerificationError)?;
    let subject = object(&subjects[0])?;
    require_exact_keys(subject, &["name", "digest"])?;
    let executable_name = artifact
        .payload
        .executable
        .path
        .rsplit('/')
        .next()
        .ok_or(AttestationVerificationError)?;
    require_string(subject, "name", executable_name)?;
    let digest = object(subject.get("digest").ok_or(AttestationVerificationError)?)?;
    require_exact_keys(digest, &["sha256"])?;
    require_string(digest, "sha256", &artifact.payload.executable.sha256)?;

    let predicate = object(
        statement
            .get("predicate")
            .ok_or(AttestationVerificationError)?,
    )?;
    require_exact_keys(predicate, &["buildDefinition", "runDetails"])?;
    validate_build_definition(
        object(
            predicate
                .get("buildDefinition")
                .ok_or(AttestationVerificationError)?,
        )?,
        source_commit,
        artifact,
    )?;
    validate_run_details(
        object(
            predicate
                .get("runDetails")
                .ok_or(AttestationVerificationError)?,
        )?,
        &artifact.workflow,
    )?;
    Ok(())
}

fn validate_build_definition(
    build: &Map<String, Value>,
    source_commit: &str,
    artifact: &QuadriFlowReleaseArtifact,
) -> VerifyResult<()> {
    require_exact_keys(
        build,
        &[
            "buildType",
            "externalParameters",
            "internalParameters",
            "resolvedDependencies",
        ],
    )?;
    require_string(build, "buildType", WORKFLOW_BUILD_TYPE)?;

    let external = object(
        build
            .get("externalParameters")
            .ok_or(AttestationVerificationError)?,
    )?;
    let expected_external = serde_json::json!({
        "workflow": {
            "repository": MESH_HARBOR_REPOSITORY_URI,
            "ref": SOURCE_REF,
            "path": WORKFLOW_PATH,
        },
        "inputs": {"confirm_candidate": "true"},
    });
    if Value::Object(external.clone()) != expected_external {
        return Err(AttestationVerificationError);
    }

    let internal = object(
        build
            .get("internalParameters")
            .ok_or(AttestationVerificationError)?,
    )?;
    let expected_internal = serde_json::json!({
        "github": {
            "repository_id": MESH_HARBOR_REPOSITORY_ID,
            "repository_owner_id": MESH_HARBOR_OWNER_ID,
            "event_name": "workflow_dispatch",
        },
        "meshharbor_platform": artifact.platform,
    });
    if Value::Object(internal.clone()) != expected_internal {
        return Err(AttestationVerificationError);
    }

    let recipe_digest = sha256_hex(PINNED_BUILD_RECIPE);
    let workflow_commit = &artifact.workflow.commit;
    let recipe_uri = format!(
        "{MESH_HARBOR_REPOSITORY_URI}/blob/{workflow_commit}/scripts/quadriflow-platform-smoke.py"
    );
    let expected_dependencies = [
        serde_json::json!({
            "uri": format!("git+{MESH_HARBOR_REPOSITORY_URI}@{SOURCE_REF}"),
            "digest": {"gitCommit": workflow_commit},
        }),
        serde_json::json!({
            "uri": format!("git+https://github.com/hjwdzh/QuadriFlow.git@{source_commit}"),
            "digest": {"gitCommit": source_commit},
        }),
        serde_json::json!({
            "uri": recipe_uri,
            "digest": {"sha256": recipe_digest},
        }),
    ];
    let dependencies = build
        .get("resolvedDependencies")
        .and_then(Value::as_array)
        .filter(|dependencies| dependencies.len() == expected_dependencies.len())
        .ok_or(AttestationVerificationError)?;
    if expected_dependencies.iter().any(|expected| {
        dependencies
            .iter()
            .filter(|actual| *actual == expected)
            .count()
            != 1
    }) {
        return Err(AttestationVerificationError);
    }
    Ok(())
}

fn validate_run_details(
    run_details: &Map<String, Value>,
    workflow: &QuadriFlowWorkflowEvidence,
) -> VerifyResult<()> {
    require_exact_keys(run_details, &["builder", "metadata"])?;
    let expected_builder = serde_json::json!({"id": WORKFLOW_IDENTITY});
    if run_details.get("builder") != Some(&expected_builder) {
        return Err(AttestationVerificationError);
    }
    let expected_invocation = format!(
        "{MESH_HARBOR_REPOSITORY_URI}/actions/runs/{}/attempts/{}",
        workflow.run_id, workflow.run_attempt
    );
    let expected_metadata = serde_json::json!({"invocationId": expected_invocation});
    if run_details.get("metadata") != Some(&expected_metadata) {
        return Err(AttestationVerificationError);
    }
    Ok(())
}

fn require_string(object: &Map<String, Value>, key: &str, expected: &str) -> VerifyResult<()> {
    if object.get(key).and_then(Value::as_str) == Some(expected) {
        Ok(())
    } else {
        Err(AttestationVerificationError)
    }
}

fn require_exact_keys(object: &Map<String, Value>, expected: &[&str]) -> VerifyResult<()> {
    if object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)) {
        Ok(())
    } else {
        Err(AttestationVerificationError)
    }
}

fn object(value: &Value) -> VerifyResult<&Map<String, Value>> {
    value.as_object().ok_or(AttestationVerificationError)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A serde_json::Value visitor that rejects duplicate object keys. The same
/// representation is then given to the upstream parser to avoid parser
/// differentials between policy checks and cryptographic verification.
struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct UniqueJsonVisitor;

        impl<'de> Visitor<'de> for UniqueJsonVisitor {
            type Value = UniqueJson;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON value with unique object keys")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(Number::from(value))))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(Number::from(value))))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Number::from_f64(value)
                    .map(Value::Number)
                    .map(UniqueJson)
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value.to_owned())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value)))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                deserializer.deserialize_any(UniqueJsonVisitor)
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueJson>()? {
                    values.push(value.0);
                }
                Ok(UniqueJson(Value::Array(values)))
            }

            fn visit_map<A>(self, mut entries: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = Map::new();
                while let Some((key, value)) = entries.next_entry::<String, UniqueJson>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate JSON object key"));
                    }
                    values.insert(key, value.0);
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }

        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

fn parse_unique_json(bytes: &[u8]) -> VerifyResult<Value> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueJson::deserialize(&mut deserializer)
        .map_err(|_| AttestationVerificationError)?
        .0;
    deserializer
        .end()
        .map_err(|_| AttestationVerificationError)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONDA_BUNDLE: &[u8] =
        include_bytes!("../tests/fixtures/sigstore-conda-attestation.sigstore.json");
    const CONDA_ARTIFACT: &[u8] =
        include_bytes!("../tests/fixtures/sigstore-signed-package-2.1.0-hb0f4dca_0.conda");
    const CONDA_IDENTITY: &str = concat!(
        "https://github.com/prefix-dev/sigstore-example",
        "/.github/workflows/action.yaml@refs/heads/main"
    );
    const CONDA_COMMIT: &str = "193b5bd7d3985809503963ae400594ea16df31cf";

    fn fixture_certificate_policy() -> CertificatePolicy<'static> {
        CertificatePolicy {
            identity: CONDA_IDENTITY,
            issuer: OIDC_ISSUER,
            repository_uri: "https://github.com/prefix-dev/sigstore-example",
            repository_name: "prefix-dev/sigstore-example",
            repository_id: "1048392115",
            owner_uri: "https://github.com/prefix-dev",
            owner_id: "111356225",
            source_ref: "refs/heads/main",
            workflow_commit: CONDA_COMMIT,
            run_id: 17_377_272_645,
            run_attempt: 1,
            trigger: "push",
            token_subject: TokenSubjectPolicy::AbsentInHistoricalFixture,
        }
    }

    fn test_artifact() -> QuadriFlowReleaseArtifact {
        QuadriFlowReleaseArtifact {
            platform: QuadriFlowPlatform::LinuxX86_64,
            archive: crate::QuadriFlowArchive {
                filename: "quadriflow-linux-x86_64.tar".into(),
                format: crate::QuadriFlowArchiveFormat::Tar,
                url: concat!(
                    "https://github.com/syi0808/meshharbor/releases/download/",
                    "quadriflow-v1.0.0/quadriflow-linux-x86_64.tar"
                )
                .into(),
                sha256: "c".repeat(64),
                size_bytes: 10,
            },
            workflow: QuadriFlowWorkflowEvidence {
                repository: "syi0808/meshharbor".into(),
                path: WORKFLOW_PATH.into(),
                run_id: 123,
                run_attempt: 2,
                commit: "a".repeat(40),
            },
            payload: crate::QuadriFlowPayload {
                executable: crate::QuadriFlowPayloadFile {
                    path: "bin/quadriflow".into(),
                    sha256: "b".repeat(64),
                    size_bytes: 10,
                },
                third_party_notices: crate::QuadriFlowPayloadFile {
                    path: "THIRD-PARTY-NOTICES.md".into(),
                    sha256: "d".repeat(64),
                    size_bytes: 10,
                },
                sbom: crate::QuadriFlowPayloadFile {
                    path: "sbom.spdx.json".into(),
                    sha256: "e".repeat(64),
                    size_bytes: 10,
                },
                provenance: crate::QuadriFlowPayloadFile {
                    path: "provenance.json".into(),
                    sha256: "f".repeat(64),
                    size_bytes: 10,
                },
                provenance_signature: crate::QuadriFlowPayloadFile {
                    path: "provenance.sigstore.json".into(),
                    sha256: "0".repeat(64),
                    size_bytes: 10,
                },
            },
            attestation_bundle_base64: String::new(),
        }
    }

    fn valid_slsa_statement(artifact: &QuadriFlowReleaseArtifact) -> Value {
        let workflow = &artifact.workflow;
        let source_commit = PINNED_SOURCE_COMMIT;
        let recipe_uri = format!(
            "{MESH_HARBOR_REPOSITORY_URI}/blob/{}/scripts/quadriflow-platform-smoke.py",
            workflow.commit
        );
        serde_json::json!({
            "_type": IN_TOTO_STATEMENT_V1,
            "subject": [{
                "name": "quadriflow",
                "digest": {"sha256": artifact.payload.executable.sha256},
            }],
            "predicateType": SLSA_PREDICATE_TYPE,
            "predicate": {
                "buildDefinition": {
                    "buildType": WORKFLOW_BUILD_TYPE,
                    "externalParameters": {
                        "workflow": {
                            "repository": MESH_HARBOR_REPOSITORY_URI,
                            "ref": SOURCE_REF,
                            "path": WORKFLOW_PATH,
                        },
                        "inputs": {"confirm_candidate": "true"},
                    },
                    "internalParameters": {
                        "github": {
                            "repository_id": MESH_HARBOR_REPOSITORY_ID,
                            "repository_owner_id": MESH_HARBOR_OWNER_ID,
                            "event_name": "workflow_dispatch",
                        },
                        "meshharbor_platform": artifact.platform,
                    },
                    "resolvedDependencies": [
                        {
                            "uri": format!("git+{MESH_HARBOR_REPOSITORY_URI}@{SOURCE_REF}"),
                            "digest": {"gitCommit": workflow.commit},
                        },
                        {
                            "uri": format!(
                                "git+https://github.com/hjwdzh/QuadriFlow.git@{source_commit}"
                            ),
                            "digest": {"gitCommit": source_commit},
                        },
                        {
                            "uri": recipe_uri,
                            "digest": {"sha256": sha256_hex(PINNED_BUILD_RECIPE)},
                        },
                    ],
                },
                "runDetails": {
                    "builder": {"id": WORKFLOW_IDENTITY},
                    "metadata": {
                        "invocationId": format!(
                            "{MESH_HARBOR_REPOSITORY_URI}/actions/runs/{}/attempts/{}",
                            workflow.run_id, workflow.run_attempt
                        ),
                    },
                },
            },
        })
    }

    #[test]
    fn mesh_harbor_policy_requires_immutable_token_subject() {
        let artifact = test_artifact();
        let policy = workflow_certificate_policy(&artifact.workflow);

        assert_eq!(policy.repository_name, MESH_HARBOR_REPOSITORY_NAME);
        assert_eq!(
            policy.token_subject,
            TokenSubjectPolicy::Required(MESH_HARBOR_TOKEN_SUBJECT)
        );
    }

    #[test]
    fn official_public_good_fixture_verifies_signature_and_workflow_claims() {
        let artifact_digest = sha256_hex(CONDA_ARTIFACT);
        let (bundle, result) = verify_sigstore_bundle(
            CONDA_BUNDLE,
            &artifact_digest,
            CONDA_IDENTITY,
            OIDC_ISSUER,
            SigstoreInstance::PublicGood,
        )
        .expect("upstream Sigstore fixture should verify against the embedded public-good root");
        assert_eq!(bundle.media_type, MediaType::Bundle0_3);
        validate_certificate_claims(&result, &fixture_certificate_policy())
            .expect("fixture certificate's GitHub CI claims should match the pinned test policy");
        let strict_subject_policy = CertificatePolicy {
            token_subject: TokenSubjectPolicy::Required(
                "repo:prefix-dev/sigstore-example:ref:refs/heads/main",
            ),
            ..fixture_certificate_policy()
        };
        assert!(
            validate_certificate_claims(&result, &strict_subject_policy).is_err(),
            "the historical fixture certificate predates the tokenSubject claim"
        );

        let SignatureContent::DsseEnvelope(envelope) = bundle.content else {
            panic!("expected a DSSE envelope")
        };
        let statement = parse_unique_json(envelope.payload.as_bytes()).unwrap();
        assert!(
            validate_slsa_statement(&statement, PINNED_SOURCE_COMMIT, &test_artifact()).is_err()
        );
    }

    #[test]
    fn signed_digest_mismatch_is_rejected() {
        assert!(
            verify_sigstore_bundle(
                CONDA_BUNDLE,
                &"0".repeat(64),
                CONDA_IDENTITY,
                OIDC_ISSUER,
                SigstoreInstance::PublicGood,
            )
            .is_err()
        );
    }

    #[test]
    fn slsa_policy_requires_exact_source_recipe_subject_and_run() {
        let artifact = test_artifact();
        let valid = valid_slsa_statement(&artifact);
        assert!(validate_slsa_statement(&valid, PINNED_SOURCE_COMMIT, &artifact).is_ok());

        let mut wrong_material = valid.clone();
        wrong_material["predicate"]["buildDefinition"]["resolvedDependencies"][1]["digest"]["gitCommit"] =
            Value::String("0".repeat(40));
        assert!(validate_slsa_statement(&wrong_material, PINNED_SOURCE_COMMIT, &artifact).is_err());

        let mut extra_material = valid;
        extra_material["predicate"]["buildDefinition"]["resolvedDependencies"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "uri": "https://unexpected.invalid",
                "digest": {"sha256": "a".repeat(64)}
            }));
        assert!(validate_slsa_statement(&extra_material, PINNED_SOURCE_COMMIT, &artifact).is_err());
    }

    #[test]
    fn duplicate_json_keys_are_rejected_before_parsing() {
        assert!(parse_unique_json(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse_unique_json(br#"{"a":{"b":1,"b":2}}"#).is_err());
    }
}
