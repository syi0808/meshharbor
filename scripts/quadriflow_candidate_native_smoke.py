#!/usr/bin/env python3
"""Run a previously verified, signed QuadriFlow executable on a native runner."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform as host_platform
import re
import shutil
import stat
import subprocess
import sys
from pathlib import Path
from typing import Any, Callable

MAX_REPORT_BYTES = 2 * 1024 * 1024
MAX_EXECUTABLE_BYTES = 128 * 1024 * 1024
MAX_BUNDLE_BYTES = 4 * 1024 * 1024
MAX_OUTPUT_BYTES = 64 * 1024 * 1024
MAX_ELF_REPORT_BYTES = 1024 * 1024
SMOKE_TIMEOUT_SECONDS = 180
SHA256 = re.compile(r"^[0-9a-f]{64}$")
SHA1 = re.compile(r"^[0-9a-f]{40}$")
REPOSITORY = "syi0808/meshharbor"
SOURCE_REF = "refs/heads/main"
OIDC_ISSUER = "https://token.actions.githubusercontent.com"
PLATFORMS = {
    "linux-x86_64": ("Linux", {"x86_64", "amd64"}, "quadriflow"),
    "macos-aarch64": ("Darwin", {"arm64", "aarch64"}, "quadriflow"),
    "windows-x86_64": ("Windows", {"amd64", "x86_64"}, "quadriflow.exe"),
}


class NativeSmokeError(RuntimeError):
    """Raised when a verified candidate is not safe to exercise natively."""


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise NativeSmokeError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _read_report(path: Path) -> dict[str, Any]:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise NativeSmokeError("verification report is missing") from error
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_size <= 0
        or metadata.st_size > MAX_REPORT_BYTES
    ):
        raise NativeSmokeError("verification report must be a bounded regular file")
    try:
        value = json.loads(path.read_bytes(), object_pairs_hook=_unique_object)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise NativeSmokeError("verification report is not valid JSON") from error
    if not isinstance(value, dict):
        raise NativeSmokeError("verification report must be a JSON object")
    return value


def _sha256_file(path: Path, max_bytes: int, label: str) -> str:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise NativeSmokeError(f"{label} is missing") from error
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size <= 0 or metadata.st_size > max_bytes:
        raise NativeSmokeError(f"{label} must be a bounded regular file")
    digest = hashlib.sha256()
    size = 0
    try:
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                size += len(chunk)
                if size > max_bytes:
                    raise NativeSmokeError(f"{label} exceeded its size limit while reading")
                digest.update(chunk)
    except OSError as error:
        raise NativeSmokeError(f"{label} could not be read") from error
    if size != metadata.st_size:
        raise NativeSmokeError(f"{label} changed while it was being hashed")
    return digest.hexdigest()


def _require_digest(value: Any, label: str) -> str:
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        raise NativeSmokeError(f"{label} is not a lowercase SHA-256 digest")
    return value


def _verify_report_context(
    report: dict[str, Any],
    *,
    platform_key: str,
    executable_name: str,
    source_commit: str,
    workflow_commit: str,
    workflow_run_id: int,
    workflow_run_attempt: int,
) -> tuple[str, str]:
    if type(report.get("schema_version")) is not int or report.get("schema_version") != 1:
        raise NativeSmokeError("verification report has an unsupported schema")
    if report.get("platform") != platform_key:
        raise NativeSmokeError("verification report platform does not match the native runner")
    if type(report.get("workflow_run_id")) is not int or report.get("workflow_run_id") != workflow_run_id:
        raise NativeSmokeError("verification report belongs to a different workflow run")
    if type(report.get("workflow_run_attempt")) is not int or report.get("workflow_run_attempt") != workflow_run_attempt:
        raise NativeSmokeError("verification report belongs to a different workflow attempt")

    artifact = report.get("artifact")
    bundle = report.get("bundle")
    materials = report.get("materials")
    policy = report.get("policy")
    if not all(isinstance(value, dict) for value in (artifact, bundle, materials, policy)):
        raise NativeSmokeError("verification report is missing required evidence objects")
    if artifact.get("name") != executable_name:
        raise NativeSmokeError("verification report names a different executable")
    if bundle.get("name") != "provenance.sigstore.json":
        raise NativeSmokeError("verification report names a different signature bundle")
    if materials.get("meshharbor_source_commit") != source_commit:
        raise NativeSmokeError("verification report has a different source commit")
    if policy.get("repository") != REPOSITORY or policy.get("source_ref") != SOURCE_REF:
        raise NativeSmokeError("verification report repository or ref does not match pinned policy")
    if (
        not SHA1.fullmatch(source_commit)
        or not SHA1.fullmatch(workflow_commit)
        or policy.get("source_commit") != source_commit
        or policy.get("workflow_commit") != workflow_commit
    ):
        raise NativeSmokeError("verification report commits do not match this workflow")
    if policy.get("workflow_trigger") != "workflow_dispatch":
        raise NativeSmokeError("verification report has an unexpected workflow trigger")
    if policy.get("oidc_issuer") != OIDC_ISSUER:
        raise NativeSmokeError("verification report has an unexpected OIDC issuer")
    if workflow_run_id <= 0 or workflow_run_attempt <= 0:
        raise NativeSmokeError("workflow run identity must be positive")
    if report.get("installer_ready") is not False:
        raise NativeSmokeError("candidate evidence must not claim installer readiness")

    return (
        _require_digest(artifact.get("sha256"), "verified executable digest"),
        _require_digest(bundle.get("sha256"), "verified bundle digest"),
    )


def _safe_child_environment(system: str) -> dict[str, str]:
    if system == "Windows":
        allowed = {
            "path",
            "systemroot",
            "windir",
            "comspec",
            "pathext",
            "temp",
            "tmp",
            "userprofile",
        }
    else:
        allowed = {"path", "home", "tmpdir", "temp", "tmp", "lang", "lc_all", "tz"}
    return {
        key: value
        for key, value in os.environ.items()
        if key.lower() in allowed
    }


def _capture_linux_elf_report(
    executable: Path,
    expected_digest: str,
    output_dir: Path,
    *,
    inspector: Callable[..., Any],
    which: Callable[[str], str | None],
) -> dict[str, Any]:
    readelf = which("readelf")
    if readelf is None:
        return {
            "status": "unavailable",
            "tool": "readelf",
            "reason": "readelf is not present on this runner; no tool was installed",
        }

    sections: dict[str, str] = {}
    for label, option in (("dynamic_section", "-d"), ("symbol_versions", "--version-info")):
        try:
            completed = inspector(
                [readelf, option, str(executable)],
                cwd=str(output_dir),
                env=_safe_child_environment("Linux"),
                timeout=30,
                check=False,
                shell=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
        except subprocess.TimeoutExpired as error:
            raise NativeSmokeError(f"readelf {option} timed out") from error
        except OSError as error:
            raise NativeSmokeError(f"readelf {option} could not be started") from error
        output = getattr(completed, "stdout", None)
        if getattr(completed, "returncode", None) != 0 or not isinstance(output, str):
            raise NativeSmokeError(f"readelf {option} failed on the native candidate")
        if len(output.encode("utf-8")) > MAX_ELF_REPORT_BYTES:
            raise NativeSmokeError(f"readelf {option} output exceeded its size limit")
        sections[label] = output

    if (
        _sha256_file(executable, MAX_EXECUTABLE_BYTES, "candidate after ELF inspection")
        != expected_digest
    ):
        raise NativeSmokeError("candidate executable bytes changed during ELF inspection")

    report = {
        "schema_version": 1,
        "status": "captured",
        "tool": "readelf",
        "executable_sha256": expected_digest,
        "dynamic_section": sections["dynamic_section"],
        "symbol_versions": sections["symbol_versions"],
    }
    report_path = output_dir / "elf-runtime.json"
    try:
        with report_path.open("x", encoding="utf-8", newline="\n") as stream:
            json.dump(report, stream, sort_keys=True, indent=2)
            stream.write("\n")
    except OSError as error:
        raise NativeSmokeError("ELF runtime report could not be written") from error
    report_digest = _sha256_file(report_path, MAX_ELF_REPORT_BYTES * 3, "ELF runtime report")
    return {
        "status": "captured",
        "tool": "readelf",
        "executable_sha256": expected_digest,
        "report_file": report_path.name,
        "report_sha256": report_digest,
    }


def run_native_smoke(
    *,
    artifact_dir: Path,
    output_dir: Path,
    platform_key: str,
    source_commit: str,
    workflow_commit: str,
    workflow_run_id: int,
    workflow_run_attempt: int,
    runner: Callable[..., Any] = subprocess.run,
    system: str | None = None,
    machine: str | None = None,
    inspector: Callable[..., Any] | None = None,
    which: Callable[[str], str | None] = shutil.which,
) -> dict[str, Any]:
    if platform_key not in PLATFORMS:
        raise NativeSmokeError("unsupported candidate platform")
    expected_system, expected_machines, executable_name = PLATFORMS[platform_key]
    actual_system = system or host_platform.system()
    actual_machine = (machine or host_platform.machine()).lower()
    if actual_system != expected_system or actual_machine not in expected_machines:
        raise NativeSmokeError("candidate platform does not match the native runner architecture")

    try:
        directory_info = artifact_dir.lstat()
    except OSError as error:
        raise NativeSmokeError("signed artifact directory is missing") from error
    if not stat.S_ISDIR(directory_info.st_mode):
        raise NativeSmokeError("signed artifact path must be a real directory")

    executable = artifact_dir / executable_name
    report_path = artifact_dir / "verification.json"
    bundle_path = artifact_dir / "provenance.sigstore.json"
    report = _read_report(report_path)
    expected_executable_digest, expected_bundle_digest = _verify_report_context(
        report,
        platform_key=platform_key,
        executable_name=executable_name,
        source_commit=source_commit,
        workflow_commit=workflow_commit,
        workflow_run_id=workflow_run_id,
        workflow_run_attempt=workflow_run_attempt,
    )
    executable_digest_before = _sha256_file(
        executable,
        MAX_EXECUTABLE_BYTES,
        "signed candidate executable",
    )
    if executable_digest_before != expected_executable_digest:
        raise NativeSmokeError("candidate bytes do not match the verified signed digest")
    report_digest_before = _sha256_file(report_path, MAX_REPORT_BYTES, "verification report")
    bundle_digest_before = _sha256_file(bundle_path, MAX_BUNDLE_BYTES, "Sigstore bundle")
    if bundle_digest_before != expected_bundle_digest:
        raise NativeSmokeError("Sigstore bundle changed after verification")

    fixture = (
        Path(__file__).resolve().parents[1]
        / "crates"
        / "core"
        / "tests"
        / "fixtures"
        / "quadriflow_cube_triangles.obj"
    )
    fixture_digest = _sha256_file(fixture, 1024 * 1024, "synthetic cube fixture")

    if output_dir.exists() or output_dir.is_symlink():
        raise NativeSmokeError("native smoke output directory must not already exist")
    try:
        parent_info = output_dir.parent.lstat()
        if not stat.S_ISDIR(parent_info.st_mode):
            raise NativeSmokeError("native smoke output parent must be a real directory")
        output_dir.mkdir(mode=0o700)
    except OSError as error:
        raise NativeSmokeError("native smoke output directory could not be created exclusively") from error

    output_obj = output_dir / "cube-quad.obj"
    elf_report = {"status": "not-applicable"}
    if actual_system == "Linux":
        elf_report = _capture_linux_elf_report(
            executable,
            expected_executable_digest,
            output_dir,
            inspector=inspector or subprocess.run,
            which=which,
        )
    if actual_system != "Windows":
        # Artifact transfer may normalize Unix modes; restore only owner execute/read.
        executable.chmod(stat.S_IRUSR | stat.S_IXUSR)

    command = [
        str(executable),
        "-i",
        str(fixture),
        "-o",
        str(output_obj),
        "-f",
        "12",
        "-seed",
        "0",
    ]
    try:
        completed = runner(
            command,
            cwd=str(output_dir),
            env=_safe_child_environment(actual_system),
            timeout=SMOKE_TIMEOUT_SECONDS,
            check=False,
            shell=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except subprocess.TimeoutExpired as error:
        raise NativeSmokeError("native QuadriFlow smoke timed out") from error
    except OSError as error:
        raise NativeSmokeError("native QuadriFlow executable could not be started") from error
    if getattr(completed, "returncode", None) != 0:
        raise NativeSmokeError("native QuadriFlow smoke returned a nonzero exit code")

    output_digest = _sha256_file(output_obj, MAX_OUTPUT_BYTES, "QuadriFlow cube output")
    executable_digest_after = _sha256_file(
        executable,
        MAX_EXECUTABLE_BYTES,
        "signed candidate executable after native execution",
    )
    if executable_digest_after != expected_executable_digest:
        raise NativeSmokeError("candidate executable bytes changed during native execution")
    if (
        _sha256_file(report_path, MAX_REPORT_BYTES, "verification report after execution")
        != report_digest_before
    ):
        raise NativeSmokeError("verification report changed during native execution")
    if (
        _sha256_file(bundle_path, MAX_BUNDLE_BYTES, "Sigstore bundle after execution")
        != bundle_digest_before
    ):
        raise NativeSmokeError("Sigstore bundle changed during native execution")
    if (
        _sha256_file(fixture, 1024 * 1024, "synthetic cube fixture after execution")
        != fixture_digest
    ):
        raise NativeSmokeError("synthetic cube fixture changed during native execution")
    if elf_report.get("status") == "captured":
        elf_report_path = output_dir / str(elf_report["report_file"])
        if (
            _sha256_file(
                elf_report_path,
                MAX_ELF_REPORT_BYTES * 3,
                "ELF runtime report after execution",
            )
            != elf_report["report_sha256"]
        ):
            raise NativeSmokeError("ELF runtime report changed during native execution")

    result = {
        "schema_version": 1,
        "status": "passed",
        "candidate_executed": True,
        "platform": platform_key,
        "workflow_run_id": workflow_run_id,
        "workflow_run_attempt": workflow_run_attempt,
        "verified_candidate_sha256": expected_executable_digest,
        "executable_sha256_before": executable_digest_before,
        "executable_sha256_after": executable_digest_after,
        "sigstore_bundle_sha256": bundle_digest_before,
        "fixture_sha256": fixture_digest,
        "quad_obj_sha256": output_digest,
        "elf_runtime_report": elf_report,
        "installer_ready": False,
    }
    try:
        report_output = output_dir / "native-smoke.json"
        with report_output.open("x", encoding="utf-8", newline="\n") as stream:
            json.dump(result, stream, sort_keys=True, indent=2)
            stream.write("\n")
    except OSError as error:
        raise NativeSmokeError("native smoke evidence could not be written") from error
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-dir", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--platform", required=True, choices=tuple(PLATFORMS))
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--workflow-commit", required=True)
    parser.add_argument("--workflow-run-id", required=True, type=int)
    parser.add_argument("--workflow-run-attempt", required=True, type=int)
    args = parser.parse_args(argv)
    try:
        report = run_native_smoke(
            artifact_dir=args.artifact_dir,
            output_dir=args.output_dir,
            platform_key=args.platform,
            source_commit=args.source_commit,
            workflow_commit=args.workflow_commit,
            workflow_run_id=args.workflow_run_id,
            workflow_run_attempt=args.workflow_run_attempt,
        )
    except NativeSmokeError as error:
        print(f"native smoke failed: {error}", file=sys.stderr)
        return 2
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
