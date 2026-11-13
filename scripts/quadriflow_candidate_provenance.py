#!/usr/bin/env python3
"""Prepare a workflow-controlled SLSA predicate for an untrusted build candidate.

This helper only parses candidate metadata and hashes regular files. It never
executes the candidate executable or its bundled dependencies.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any


REPOSITORY = "syi0808/meshharbor"
REPOSITORY_URI = "https://github.com/syi0808/meshharbor"
SOURCE_REF = "refs/heads/main"
WORKFLOW_PATH = ".github/workflows/quadriflow-candidate-build.yml"
WORKFLOW_SAN = (
    "https://github.com/syi0808/meshharbor/"
    ".github/workflows/quadriflow-candidate-build.yml@refs/heads/main"
)
UPSTREAM_REPOSITORY = "https://github.com/hjwdzh/QuadriFlow.git"
UPSTREAM_COMMIT = "810b7a0967c35b0dc85b4464e3835e26a756c967"
BUILD_TYPE = "https://slsa-framework.github.io/github-actions-buildtypes/workflow/v1"
PREDICATE_TYPE = "https://slsa.dev/provenance/v1"
PLATFORMS = {
    "linux-x86_64": "quadriflow",
    "macos-aarch64": "quadriflow",
    "windows-x86_64": "quadriflow.exe",
}
BUILD_OPTIONS = [
    "-DCMAKE_BUILD_TYPE=Release",
    "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
    "-DBUILD_FREE_LICENSE=ON",
    "-DBUILD_OPENMP=OFF",
    "-DBUILD_TBB=OFF",
    "-DBUILD_GUROBI=OFF",
    "-DBUILD_LOG=OFF",
    "-DBUILD_PERFORMANCE_TEST=OFF",
    "-DLEMON_ENABLE_GLPK=OFF",
    "-DLEMON_ENABLE_ILOG=OFF",
    "-DLEMON_ENABLE_COIN=OFF",
    "-DLEMON_ENABLE_SOPLEX=OFF",
]
WINDOWS_BUILD_OPTIONS = [
    "-DVCPKG_TARGET_TRIPLET=x64-windows-static",
    "-DCMAKE_POLICY_DEFAULT_CMP0091=NEW",
    "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded$<$<CONFIG:Debug>:Debug>",
]
MAX_CANDIDATE_FILE_BYTES = 128 * 1024 * 1024
MAX_METADATA_BYTES = 4 * 1024 * 1024
SHA1 = re.compile(r"^[0-9a-f]{40}$")


class CandidateProvenanceError(RuntimeError):
    """Raised when candidate evidence does not meet the fixed signing policy."""


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise CandidateProvenanceError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _read_regular_file(value: str | Path, label: str, maximum: int) -> tuple[Path, bytes]:
    path = Path(value)
    if path.is_symlink() or not path.is_file():
        raise CandidateProvenanceError(f"{label} must be a regular file")
    try:
        if path.stat().st_size > maximum:
            raise CandidateProvenanceError(f"{label} exceeds its size limit")
        data = path.read_bytes()
    except OSError as error:
        raise CandidateProvenanceError(f"could not read {label}") from error
    if len(data) > maximum:
        raise CandidateProvenanceError(f"{label} exceeds its size limit")
    return path, data


def _read_json(path: Path, label: str, maximum: int) -> Any:
    _, data = _read_regular_file(path, label, maximum)
    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=_unique_object)
    except CandidateProvenanceError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CandidateProvenanceError(f"{label} is not valid UTF-8 JSON") from error


def _parse_commit(value: str, label: str) -> str:
    if not isinstance(value, str) or not SHA1.fullmatch(value):
        raise CandidateProvenanceError(f"{label} must be a lowercase 40-character commit SHA")
    return value


def _parse_positive(value: int, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 1:
        raise CandidateProvenanceError(f"{label} must be a positive integer")
    return value


def _expected_build_options(platform: str) -> list[str]:
    options = list(BUILD_OPTIONS)
    if platform == "windows-x86_64":
        options.extend(WINDOWS_BUILD_OPTIONS)
    return options


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def create_predicate(
    repository_root: str | Path,
    source_commit_value: str,
    workflow_commit_value: str,
    workflow_run_id: int,
    workflow_run_attempt: int,
    platform: str,
) -> dict[str, Any]:
    source_commit = _parse_commit(source_commit_value, "source commit")
    workflow_commit = _parse_commit(workflow_commit_value, "workflow commit")
    if workflow_commit != source_commit:
        raise CandidateProvenanceError(
            "workflow and source commits must match for a main-branch manual build"
        )
    run_id = _parse_positive(workflow_run_id, "workflow run id")
    run_attempt = _parse_positive(workflow_run_attempt, "workflow run attempt")
    if platform not in PLATFORMS:
        raise CandidateProvenanceError("platform is not supported by the candidate policy")

    root = Path(repository_root)
    recipe_path, recipe_bytes = _read_regular_file(
        root / "scripts" / "quadriflow-platform-smoke.py",
        "pinned build recipe",
        MAX_METADATA_BYTES,
    )
    recipe_sha256 = _sha256(recipe_bytes)
    recipe_uri = (
        f"{REPOSITORY_URI}/blob/{source_commit}/scripts/quadriflow-platform-smoke.py"
    )
    dependencies = [
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
            "digest": {"sha256": recipe_sha256},
        },
    ]
    return {
        "buildDefinition": {
            "buildType": BUILD_TYPE,
            "externalParameters": {
                "workflow": {
                    "repository": REPOSITORY_URI,
                    "ref": SOURCE_REF,
                    "path": WORKFLOW_PATH,
                },
                "inputs": {"confirm_candidate": "true"},
            },
            "internalParameters": {
                "github": {
                    "repository_id": "1454899411",
                    "repository_owner_id": "66503450",
                    "event_name": "workflow_dispatch",
                },
                "meshharbor_platform": platform,
            },
            "resolvedDependencies": dependencies,
        },
        "runDetails": {
            "builder": {"id": WORKFLOW_SAN},
            "metadata": {
                "invocationId": (
                    f"{REPOSITORY_URI}/actions/runs/{run_id}/attempts/{run_attempt}"
                )
            },
        },
    }


def validate_candidate(
    candidate_dir_value: str | Path,
    executable_name: str,
    platform: str,
    source_commit: str,
    workflow_commit: str,
) -> None:
    if platform not in PLATFORMS or executable_name != PLATFORMS[platform]:
        raise CandidateProvenanceError("candidate executable does not match the platform")
    candidate_dir = Path(candidate_dir_value)
    if candidate_dir.is_symlink() or not candidate_dir.is_dir():
        raise CandidateProvenanceError("candidate directory must be a real directory")

    _read_regular_file(
        candidate_dir / executable_name,
        "candidate executable",
        MAX_CANDIDATE_FILE_BYTES,
    )
    status = _read_json(candidate_dir / "CANDIDATE-STATUS.json", "candidate status", 64 * 1024)
    if (
        not isinstance(status, dict)
        or status.get("schema_version") != 1
        or status.get("candidate_only") is not True
        or status.get("installable") is not False
        or status.get("release_ready") is not False
        or status.get("dependency_closure_verified") is not False
        or status.get("quadriFlow_executable_run") is not False
    ):
        raise CandidateProvenanceError("candidate status is not fail-closed")

    provenance = _read_json(
        candidate_dir / "provenance.json",
        "candidate provenance",
        MAX_METADATA_BYTES,
    )
    expected = {
        "purpose": "manual pinned-source candidate build only; QuadriFlow was not executed",
        "meshharbor_source_commit": source_commit,
        "meshharbor_workflow_commit": workflow_commit,
        "meshharbor_workflow_path": WORKFLOW_PATH,
        "build_trigger": "workflow_dispatch",
        "upstream_repository": UPSTREAM_REPOSITORY,
        "upstream_commit": UPSTREAM_COMMIT,
        "platform": platform,
        "build_mode": "Release",
        "build_options": _expected_build_options(platform),
        "smoke_test": None,
        "smoke_result": "not-run",
    }
    if not isinstance(provenance, dict):
        raise CandidateProvenanceError("candidate provenance must be an object")
    for key, value in expected.items():
        if provenance.get(key) != value:
            raise CandidateProvenanceError(
                f"candidate provenance {key} does not match the pinned policy"
            )


def prepare_candidate(
    repository_root: str | Path,
    candidate_dir: str | Path,
    executable_name: str,
    source_commit: str,
    workflow_commit: str,
    workflow_run_id: int,
    workflow_run_attempt: int,
    platform: str,
    output_value: str | Path,
) -> dict[str, Any]:
    source_sha = _parse_commit(source_commit, "source commit")
    workflow_sha = _parse_commit(workflow_commit, "workflow commit")
    validate_candidate(candidate_dir, executable_name, platform, source_sha, workflow_sha)
    predicate = create_predicate(
        repository_root,
        source_sha,
        workflow_sha,
        workflow_run_id,
        workflow_run_attempt,
        platform,
    )
    output = Path(output_value)
    if output.exists() or output.is_symlink():
        raise CandidateProvenanceError("refusing to overwrite predicate output")
    try:
        with output.open("xb") as handle:
            handle.write((json.dumps(predicate, sort_keys=True, indent=2) + "\n").encode("utf-8"))
    except OSError as error:
        raise CandidateProvenanceError("could not write predicate output") from error
    return predicate


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("prepare",))
    parser.add_argument("--repository-root", required=True)
    parser.add_argument("--candidate-dir", required=True)
    parser.add_argument("--executable", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--workflow-commit", required=True)
    parser.add_argument("--workflow-run-id", required=True, type=int)
    parser.add_argument("--workflow-run-attempt", required=True, type=int)
    parser.add_argument("--platform", required=True, choices=tuple(PLATFORMS))
    parser.add_argument("--output", required=True)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        prepare_candidate(
            args.repository_root,
            args.candidate_dir,
            args.executable,
            args.source_commit,
            args.workflow_commit,
            args.workflow_run_id,
            args.workflow_run_attempt,
            args.platform,
            args.output,
        )
    except CandidateProvenanceError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("Prepared candidate-only SLSA materials predicate; installer readiness remains false.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
