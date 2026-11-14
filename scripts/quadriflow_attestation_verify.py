#!/usr/bin/env python3
"""Fail-closed maintainer verification for a temporary QuadriFlow build candidate.

This uses the official GitHub CLI and its TUF-verified Sigstore trust root. It
verifies a local v0.3 bundle for one executable, then additionally checks
MeshHarbor's fixed repository IDs, main ref, source/workflow digests, workflow
path, manual trigger, hosted runner, OIDC issuer, SLSA predicate, and subject
digest. It does not enable or implement the product installer.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from typing import Any

REPOSITORY = "syi0808/meshharbor"
REPOSITORY_URI = "https://github.com/syi0808/meshharbor"
OWNER_URI = "https://github.com/syi0808"
REPOSITORY_ID = "1454899411"
OWNER_ID = "66503450"
SOURCE_REF = "refs/heads/main"
WORKFLOW_PATH = ".github/workflows/quadriflow-candidate-build.yml"
WORKFLOW_SAN = (
    "https://github.com/syi0808/meshharbor/"
    ".github/workflows/quadriflow-candidate-build.yml@refs/heads/main"
)
OIDC_ISSUER = "https://token.actions.githubusercontent.com"
PREDICATE_TYPE = "https://slsa.dev/provenance/v1"
GITHUB_WORKFLOW_BUILD_TYPE = "https://slsa-framework.github.io/github-actions-buildtypes/workflow/v1"
UPSTREAM_REPOSITORY = "https://github.com/hjwdzh/QuadriFlow.git"
UPSTREAM_COMMIT = "810b7a0967c35b0dc85b4464e3835e26a756c967"
PLATFORMS = {"linux-x86_64": "quadriflow", "macos-aarch64": "quadriflow", "windows-x86_64": "quadriflow.exe"}
BUNDLE_MEDIA_TYPE = "application/vnd.dev.sigstore.bundle.v0.3+json"
MIN_GH_CLI = (2, 96, 0)
MAX_ARTIFACT_BYTES = 128 * 1024 * 1024
MAX_BUNDLE_BYTES = 4 * 1024 * 1024
MAX_TRUST_ROOT_BYTES = 8 * 1024 * 1024
MAX_CANDIDATE_FILES = 2048
MAX_CANDIDATE_BYTES = 256 * 1024 * 1024
SHA1 = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")


class VerificationError(RuntimeError):
    """A candidate did not satisfy the fixed verification policy."""


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise VerificationError("duplicate JSON object key")
        result[key] = value
    return result


def _json_bytes(data: bytes, label: str) -> Any:
    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=_unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise VerificationError(f"{label} is not valid UTF-8 JSON") from error


def _regular_file(value: str | Path, label: str, maximum: int) -> tuple[Path, bytes]:
    path = Path(value)
    if path.is_symlink() or not path.is_file():
        raise VerificationError(f"{label} must be a regular non-symlink file")
    try:
        resolved = path.resolve(strict=True)
        size = resolved.stat().st_size
    except OSError as error:
        raise VerificationError(f"{label} cannot be read") from error
    if size <= 0 or size > maximum:
        raise VerificationError(f"{label} is empty or exceeds its size limit")
    try:
        data = resolved.read_bytes()
    except OSError as error:
        raise VerificationError(f"{label} cannot be read") from error
    if len(data) != size or len(data) > maximum:
        raise VerificationError(f"{label} changed or exceeded its size limit")
    return resolved, data


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _parse_sha(value: str, label: str) -> str:
    if not isinstance(value, str) or not SHA1.fullmatch(value):
        raise VerificationError(f"{label} must be a 40-character lowercase Git SHA")
    return value


def _parse_positive_int(value: int, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 1:
        raise VerificationError(f"{label} must be a positive integer")
    return value


def _build_recipe_sha256() -> str:
    repository_root = Path(__file__).resolve().parents[1]
    _, recipe = _regular_file(
        repository_root / "scripts" / "quadriflow-platform-smoke.py",
        "pinned build recipe",
        4 * 1024 * 1024,
    )
    return _sha256(recipe)


def _validate_predicate(
    predicate: Any,
    source_commit: str,
    workflow_commit: str,
    workflow_run_id: int,
    workflow_run_attempt: int,
    platform: str,
    artifact_sha256: str,
) -> None:
    if workflow_commit != source_commit:
        raise VerificationError("workflow and source commits must match for main-branch builds")
    if platform not in PLATFORMS:
        raise VerificationError("platform is not supported by the fixed candidate policy")
    if not isinstance(predicate, dict):
        raise VerificationError("SLSA provenance predicate is missing")

    build_definition = predicate.get("buildDefinition")
    if not isinstance(build_definition, dict):
        raise VerificationError("SLSA build definition is missing")
    if build_definition.get("buildType") != GITHUB_WORKFLOW_BUILD_TYPE:
        raise VerificationError("SLSA build type did not match the pinned policy")

    expected_workflow = {
        "repository": REPOSITORY_URI,
        "ref": SOURCE_REF,
        "path": WORKFLOW_PATH,
    }
    expected_external = {
        "workflow": expected_workflow,
        "inputs": {"confirm_candidate": "true"},
    }
    external_parameters = build_definition.get("externalParameters")
    if external_parameters != expected_external:
        raise VerificationError("SLSA external parameters did not match the pinned policy")

    expected_internal = {
        "github": {
            "repository_id": REPOSITORY_ID,
            "repository_owner_id": OWNER_ID,
            "event_name": "workflow_dispatch",
        },
        "meshharbor_platform": platform,
        "meshharbor_reproducibility": {
            "strategy": "independent-job-byte-identical",
            "primary_replica": "primary",
            "independent_replica": "independent",
            "independent_build_sha256": artifact_sha256,
        },
    }
    if build_definition.get("internalParameters") != expected_internal:
        raise VerificationError("SLSA GitHub context did not match the pinned policy")

    recipe_uri = (
        f"{REPOSITORY_URI}/blob/{source_commit}/scripts/quadriflow-platform-smoke.py"
    )
    expected_dependencies = [
        {
            "uri": f"git+{REPOSITORY_URI}@{SOURCE_REF}",
            "digest": {"gitCommit": source_commit},
        },
        {
            "uri": f"git+{UPSTREAM_REPOSITORY}@{UPSTREAM_COMMIT}",
            "digest": {"gitCommit": UPSTREAM_COMMIT},
        },
        {
            "uri": recipe_uri,
            "digest": {"sha256": _build_recipe_sha256()},
        },
    ]
    dependencies = build_definition.get("resolvedDependencies")
    if not isinstance(dependencies, list) or len(dependencies) != len(expected_dependencies):
        raise VerificationError("SLSA materials do not match the pinned source set")
    actual_dependencies = sorted(
        dependencies,
        key=lambda item: str(item.get("uri")) if isinstance(item, dict) else "",
    )
    expected_dependencies = sorted(expected_dependencies, key=lambda item: item["uri"])
    if actual_dependencies != expected_dependencies:
        raise VerificationError("SLSA materials do not match the pinned source set")

    run_details = predicate.get("runDetails")
    if not isinstance(run_details, dict):
        raise VerificationError("SLSA run details are missing")
    if run_details.get("builder") != {"id": WORKFLOW_SAN}:
        raise VerificationError("SLSA builder identity did not match the pinned workflow")
    metadata = run_details.get("metadata")
    expected_invocation = (
        f"{REPOSITORY_URI}/actions/runs/{workflow_run_id}/attempts/{workflow_run_attempt}"
    )
    if not isinstance(metadata, dict) or metadata.get("invocationId") != expected_invocation:
        raise VerificationError("SLSA invocation did not match the current workflow run")


def _run(argv: list[str]) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            argv,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=180,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise VerificationError("GitHub CLI did not complete successfully") from error


def _require_success(result: subprocess.CompletedProcess[str], label: str) -> str:
    if result.returncode != 0:
        raise VerificationError(f"GitHub CLI {label} failed")
    return result.stdout


def _gh_version() -> str:
    output = _require_success(_run(["gh", "version"]), "version check")
    match = re.search(r"(?m)^gh version (\d+)\.(\d+)\.(\d+)(?:\s|$)", output)
    if match is None:
        raise VerificationError("GitHub CLI version output is unsupported")
    version = tuple(int(part) for part in match.groups())
    if version[0] != 2 or version < MIN_GH_CLI:
        raise VerificationError("GitHub CLI 2.96.0 or newer in the 2.x series is required")
    return ".".join(match.groups())


def _verified_trust_root() -> bytes:
    output = _require_success(_run(["gh", "attestation", "trusted-root"]), "trusted-root")
    data = output.encode("utf-8")
    if not data or len(data) > MAX_TRUST_ROOT_BYTES:
        raise VerificationError("GitHub CLI trusted root is empty or oversized")
    records = [line for line in data.splitlines() if line.strip()]
    if not records or len(records) > 16:
        raise VerificationError("GitHub CLI trusted root has an unsupported JSONL shape")
    for record in records:
        value = _json_bytes(record, "GitHub CLI trusted root")
        if not isinstance(value, dict):
            raise VerificationError("GitHub CLI trusted root entry is not an object")
    return data


def _verify_command(
    artifact: Path,
    bundle: Path,
    root_path: Path,
    source_commit: str,
    workflow_commit: str,
) -> list[str]:
    return [
        "gh",
        "attestation",
        "verify",
        "--hostname",
        "github.com",
        str(artifact),
        "--bundle",
        str(bundle),
        "--custom-trusted-root",
        str(root_path),
        "--repo",
        REPOSITORY,
        "--cert-identity",
        WORKFLOW_SAN,
        "--signer-digest",
        workflow_commit,
        "--source-ref",
        SOURCE_REF,
        "--source-digest",
        source_commit,
        "--cert-oidc-issuer",
        OIDC_ISSUER,
        "--predicate-type",
        PREDICATE_TYPE,
        "--deny-self-hosted-runners",
        "--format",
        "json",
    ]


def _require_equal(actual: Any, expected: str, label: str) -> None:
    if actual != expected:
        raise VerificationError(f"attestation {label} did not match the pinned policy")


def _validate_cli_result(
    output: str,
    artifact_digest: str,
    artifact_name: str,
    source_commit: str,
    workflow_commit: str,
    workflow_run_id: int,
    workflow_run_attempt: int,
    platform: str,
) -> int:
    result = _json_bytes(output.encode("utf-8"), "GitHub CLI verification output")
    if not isinstance(result, list) or len(result) != 1 or not isinstance(result[0], dict):
        raise VerificationError("GitHub CLI did not return exactly one verified attestation")
    verified = result[0].get("verificationResult")
    if not isinstance(verified, dict):
        raise VerificationError("GitHub CLI verification result is missing")
    signature = verified.get("signature")
    certificate = signature.get("certificate") if isinstance(signature, dict) else None
    if not isinstance(certificate, dict):
        raise VerificationError("verified signing certificate summary is missing")

    checks = (
        (certificate.get("issuer"), OIDC_ISSUER, "OIDC issuer"),
        (certificate.get("subjectAlternativeName"), WORKFLOW_SAN, "workflow identity"),
        (certificate.get("buildSignerURI"), WORKFLOW_SAN, "signer workflow URI"),
        (certificate.get("buildSignerDigest"), workflow_commit, "signer workflow revision"),
        (certificate.get("sourceRepositoryURI"), REPOSITORY_URI, "source repository"),
        (certificate.get("sourceRepositoryOwnerURI"), OWNER_URI, "source owner"),
        (certificate.get("sourceRepositoryIdentifier"), REPOSITORY_ID, "source repository id"),
        (certificate.get("sourceRepositoryOwnerIdentifier"), OWNER_ID, "source owner id"),
        (certificate.get("sourceRepositoryRef"), SOURCE_REF, "source ref"),
        (certificate.get("sourceRepositoryDigest"), source_commit, "source commit"),
        (certificate.get("runnerEnvironment"), "github-hosted", "runner environment"),
        (certificate.get("buildTrigger"), "workflow_dispatch", "workflow trigger"),
    )
    for actual, expected, label in checks:
        _require_equal(actual, expected, label)

    statement = verified.get("statement")
    if not isinstance(statement, dict):
        raise VerificationError("verified in-toto statement is missing")
    _require_equal(statement.get("predicateType"), PREDICATE_TYPE, "predicate type")
    subjects = statement.get("subject")
    if not isinstance(subjects, list) or len(subjects) != 1 or not isinstance(subjects[0], dict):
        raise VerificationError("attestation must contain exactly one subject")
    _require_equal(subjects[0].get("name"), artifact_name, "subject name")
    digest_map = subjects[0].get("digest")
    if not isinstance(digest_map, dict):
        raise VerificationError("attestation subject digest is missing")
    _require_equal(digest_map.get("sha256"), artifact_digest, "subject SHA-256")
    _validate_predicate(
        statement.get("predicate"),
        source_commit,
        workflow_commit,
        workflow_run_id,
        workflow_run_attempt,
        platform,
        artifact_digest,
    )

    timestamps = verified.get("verifiedTimestamps")
    if not isinstance(timestamps, list) or not timestamps:
        raise VerificationError("GitHub CLI did not report a verified timestamp")
    return len(timestamps)


def _write_new(path: Path, data: bytes) -> None:
    if path.exists() or path.is_symlink():
        raise VerificationError(f"refusing to overwrite {path.name}")
    try:
        with path.open("xb") as handle:
            handle.write(data)
    except OSError as error:
        raise VerificationError(f"could not write {path.name}") from error


def _candidate_status(candidate_dir: Path) -> None:
    status_path = candidate_dir / "CANDIDATE-STATUS.json"
    _, data = _regular_file(status_path, "candidate status", 64 * 1024)
    status = _json_bytes(data, "candidate status")
    if (
        not isinstance(status, dict)
        or status.get("candidate_only") is not True
        or status.get("installable") is not False
        or status.get("release_ready") is not False
        or status.get("dependency_closure_verified") is not False
    ):
        raise VerificationError("candidate status is missing the required non-release gates")


def _write_checksums(candidate_dir: Path) -> None:
    entries: list[tuple[str, str]] = []
    total = 0
    count = 0
    for directory, subdirs, filenames in os.walk(candidate_dir, followlinks=False):
        for name in subdirs:
            if (Path(directory) / name).is_symlink():
                raise VerificationError("candidate directory contains a symlink")
        for name in filenames:
            path = Path(directory) / name
            if path.is_symlink() or not path.is_file():
                raise VerificationError("candidate directory contains a non-regular file")
            if path.name == "SHA256SUMS":
                continue
            size = path.stat().st_size
            total += size
            count += 1
            if count > MAX_CANDIDATE_FILES or total > MAX_CANDIDATE_BYTES:
                raise VerificationError("candidate directory exceeds checksum limits")
            digest = _sha256(path.read_bytes())
            relative = path.relative_to(candidate_dir).as_posix()
            entries.append((relative, digest))

    lines = [f"{digest}  {name}" for name, digest in sorted(entries)]
    sums_path = candidate_dir / "SHA256SUMS"
    temporary = candidate_dir / ".SHA256SUMS.tmp"
    if temporary.exists() or temporary.is_symlink():
        raise VerificationError("temporary checksum path already exists")
    try:
        with temporary.open("xb") as handle:
            handle.write(("\n".join(lines) + "\n").encode("ascii"))
        os.replace(temporary, sums_path)
    except OSError as error:
        try:
            temporary.unlink(missing_ok=True)
        except OSError:
            pass
        raise VerificationError("could not finalize candidate checksums") from error


def verify_attestation(
    artifact_value: str | Path,
    bundle_value: str | Path,
    source_commit_value: str,
    workflow_commit_value: str,
    output_value: str | Path,
    workflow_run_id_value: int,
    workflow_run_attempt_value: int,
    platform_value: str,
) -> dict[str, Any]:
    source_commit = _parse_sha(source_commit_value, "source commit")
    workflow_commit = _parse_sha(workflow_commit_value, "workflow commit")
    workflow_run_id = _parse_positive_int(workflow_run_id_value, "workflow run id")
    workflow_run_attempt = _parse_positive_int(
        workflow_run_attempt_value, "workflow run attempt"
    )
    if platform_value not in PLATFORMS:
        raise VerificationError("platform is not supported by the fixed candidate policy")
    artifact, artifact_bytes = _regular_file(artifact_value, "candidate executable", MAX_ARTIFACT_BYTES)
    if artifact.name != PLATFORMS[platform_value]:
        raise VerificationError("candidate executable does not match the platform")
    bundle, bundle_bytes = _regular_file(bundle_value, "Sigstore bundle", MAX_BUNDLE_BYTES)
    if artifact == bundle:
        raise VerificationError("candidate executable and bundle must be distinct files")
    bundle_json = _json_bytes(bundle_bytes, "Sigstore bundle")
    if not isinstance(bundle_json, dict) or bundle_json.get("mediaType") != BUNDLE_MEDIA_TYPE:
        raise VerificationError("candidate bundle must use Sigstore v0.3 JSON")
    candidate_dir = artifact.parent
    _candidate_status(candidate_dir)
    output = Path(output_value)
    if output.name != "verification.json" or output.parent.resolve() != candidate_dir.resolve():
        raise VerificationError("verification report must be written inside the candidate directory")
    if output.exists() or output.is_symlink():
        raise VerificationError("verification report already exists")

    gh_version = _gh_version()
    trust_root_data = _verified_trust_root()
    root_fd, root_name = tempfile.mkstemp(prefix="meshharbor-sigstore-root-", suffix=".jsonl")
    root_path = Path(root_name)
    try:
        if hasattr(os, "fchmod"):
            os.fchmod(root_fd, 0o600)
        with os.fdopen(root_fd, "wb") as handle:
            handle.write(trust_root_data)
        command = _verify_command(artifact, bundle, root_path, source_commit, workflow_commit)
        verified_output = _require_success(_run(command), "attestation verification")
        timestamp_count = _validate_cli_result(
            verified_output,
            _sha256(artifact_bytes),
            artifact.name,
            source_commit,
            workflow_commit,
            workflow_run_id,
            workflow_run_attempt,
            platform_value,
        )
    finally:
        try:
            root_path.unlink(missing_ok=True)
        except OSError:
            pass

    # Verify that the files the CLI checked did not change while the command ran.
    if _sha256(artifact.read_bytes()) != _sha256(artifact_bytes):
        raise VerificationError("candidate executable changed during verification")
    if _sha256(bundle.read_bytes()) != _sha256(bundle_bytes):
        raise VerificationError("Sigstore bundle changed during verification")

    bundle_copy = candidate_dir / "provenance.sigstore.json"
    if bundle_copy != bundle:
        if bundle_copy.exists() or bundle_copy.is_symlink():
            raise VerificationError("candidate already contains a Sigstore bundle")
        _write_new(bundle_copy, bundle_bytes)
    if _sha256(bundle_copy.read_bytes()) != _sha256(bundle_bytes):
        raise VerificationError("copied Sigstore bundle did not match verified bytes")

    report: dict[str, Any] = {
        "schema_version": 1,
        "verifier": "GitHub CLI gh attestation verify on a clean GitHub-hosted runner",
        "verifier_version": gh_version,
        "github_host": "github.com",
        "trusted_root": "obtained by gh attestation trusted-root and passed through TUF verification",
        "artifact": {"name": artifact.name, "sha256": _sha256(artifact_bytes)},
        "bundle": {"name": bundle_copy.name, "sha256": _sha256(bundle_bytes)},
        "platform": platform_value,
        "workflow_run_id": workflow_run_id,
        "workflow_run_attempt": workflow_run_attempt,
        "materials": {
            "meshharbor_source_commit": source_commit,
            "quadriflow_repository": UPSTREAM_REPOSITORY,
            "quadriflow_commit": UPSTREAM_COMMIT,
            "build_recipe_sha256": _build_recipe_sha256(),
        },
        "policy": {
            "repository": REPOSITORY,
            "repository_id": REPOSITORY_ID,
            "owner_id": OWNER_ID,
            "source_ref": SOURCE_REF,
            "source_commit": source_commit,
            "signer_workflow": f"{REPOSITORY}/{WORKFLOW_PATH}",
            "workflow_commit": workflow_commit,
            "workflow_trigger": "workflow_dispatch",
            "runner_environment": "github-hosted",
            "oidc_issuer": OIDC_ISSUER,
            "predicate_type": PREDICATE_TYPE,
        },
        "verified_timestamp_count": timestamp_count,
        "verified_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "installer_ready": False,
    }
    _write_new(output, (json.dumps(report, sort_keys=True, indent=2) + "\n").encode("utf-8"))
    _write_checksums(candidate_dir)
    return report


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("verify",))
    parser.add_argument("--artifact", required=True, help="candidate executable")
    parser.add_argument("--bundle", required=True, help="local Sigstore bundle from actions/attest")
    parser.add_argument("--source-commit", required=True, help="GitHub source commit SHA")
    parser.add_argument("--workflow-commit", required=True, help="GitHub workflow revision SHA")
    parser.add_argument("--workflow-run-id", required=True, type=int)
    parser.add_argument("--workflow-run-attempt", required=True, type=int)
    parser.add_argument("--platform", required=True, choices=tuple(PLATFORMS))
    parser.add_argument("--output", required=True, help="candidate-directory verification.json path")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        report = verify_attestation(
            args.artifact,
            args.bundle,
            args.source_commit,
            args.workflow_commit,
            args.output,
            args.workflow_run_id,
            args.workflow_run_attempt,
            args.platform,
        )
    except VerificationError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(
        f"Verified candidate executable {report['artifact']['sha256']} "
        f"with {report['verified_timestamp_count']} authenticated timestamp(s); "
        "installer readiness remains false."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
