#!/usr/bin/env python3
"""Build pinned QuadriFlow; optionally run the approved synthetic-cube smoke test."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


QUADRIFLOW_COMMIT = "810b7a0967c35b0dc85b4464e3835e26a756c967"
QUADRIFLOW_REPOSITORY = "https://github.com/hjwdzh/QuadriFlow.git"
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


def write_checksum_manifest(artifact_dir: Path) -> None:
    entries = []
    for path in sorted(artifact_dir.rglob("*")):
        if path.is_symlink():
            raise RuntimeError("artifact directory contains a symlink")
        if path.is_dir():
            continue
        if not path.is_file():
            raise RuntimeError("artifact directory contains a non-regular file")
        if path == artifact_dir / "SHA256SUMS":
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        relative = path.relative_to(artifact_dir).as_posix()
        entries.append(f"{digest}  {relative}")
    (artifact_dir / "SHA256SUMS").write_text("\n".join(entries) + "\n", encoding="ascii")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--build-only",
        action="store_true",
        help="build and collect candidate evidence without executing QuadriFlow or running its smoke test",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    repository_root = Path(__file__).resolve().parents[1]
    temp_root = Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir()))
    work_root = temp_root / "quadriflow-platform-smoke"
    source_dir = work_root / "upstream"
    build_dir = work_root / "build"
    output_dir = work_root / "output"
    artifact_dir = work_root / "artifact"
    output_obj = output_dir / "cube-quad.obj"
    log_path = artifact_dir / "build-and-smoke.log"
    platform_name = os.environ.get("RUNNER_OS", platform.system())
    candidate_platform = os.environ.get("MESHHARBOR_PLATFORM", platform_name)
    build_replica = os.environ.get("MESHHARBOR_BUILD_REPLICA", "single")
    if build_replica not in {"single", "primary", "independent"}:
        raise RuntimeError("MESHHARBOR_BUILD_REPLICA is not supported")
    executable_name = "quadriflow.exe" if os.name == "nt" else "quadriflow"

    if work_root.exists():
        shutil.rmtree(work_root)
    output_dir.mkdir(parents=True)
    artifact_dir.mkdir(parents=True)

    with log_path.open("w", encoding="utf-8") as log:
        def run(
            command: list[str],
            *,
            cwd: Path | None = None,
            env: dict[str, str] | None = None,
            timeout: int = 1800,
        ) -> str:
            rendered = subprocess.list2cmdline(command) if os.name == "nt" else " ".join(command)
            print(f"+ {rendered}", flush=True)
            log.write(f"\n$ {rendered}\n")
            log.flush()
            completed = subprocess.run(
                command,
                cwd=cwd,
                env=env,
                check=False,
                timeout=timeout,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            print(completed.stdout, end="", flush=True)
            log.write(completed.stdout)
            log.flush()
            if completed.returncode != 0:
                raise subprocess.CalledProcessError(
                    completed.returncode, command, output=completed.stdout
                )
            return completed.stdout

        def version(command: list[str]) -> str:
            try:
                completed = subprocess.run(
                    command,
                    check=False,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                )
            except OSError as error:
                return f"unavailable: {error}"
            output = completed.stdout.strip()
            if completed.returncode != 0:
                return f"unavailable (exit {completed.returncode}): {output}"
            return output

        try:
            run(["git", "init", str(source_dir)])
            run(["git", "-C", str(source_dir), "remote", "add", "origin", QUADRIFLOW_REPOSITORY])
            run(["git", "-C", str(source_dir), "fetch", "--depth=1", "origin", QUADRIFLOW_COMMIT])
            run(["git", "-C", str(source_dir), "checkout", "--detach", "FETCH_HEAD"])
            resolved_commit = run(["git", "-C", str(source_dir), "rev-parse", "HEAD"]).strip()
            if resolved_commit != QUADRIFLOW_COMMIT:
                raise RuntimeError(f"expected upstream {QUADRIFLOW_COMMIT}, got {resolved_commit}")

            license_file = source_dir / "LICENSE.txt"
            if not license_file.is_file():
                raise RuntimeError("pinned upstream LICENSE.txt was not found")
            shutil.copy2(license_file, artifact_dir / "QuadriFlow-LICENSE.txt")
            notices_dir = artifact_dir / "third-party-notices"
            for candidate in source_dir.rglob("*"):
                if not candidate.is_file():
                    continue
                upper_name = candidate.name.upper()
                if upper_name.startswith(("LICENSE", "COPYING", "NOTICE")):
                    relative = candidate.relative_to(source_dir)
                    destination = notices_dir / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(candidate, destination)

            cmake_command = ["cmake", "-S", str(source_dir), "-B", str(build_dir), *BUILD_OPTIONS]
            platform_build_options: list[str] = []
            if os.name == "nt":
                vcpkg_root = os.environ.get("VCPKG_INSTALLATION_ROOT") or os.environ.get("VCPKG_ROOT")
                if not vcpkg_root:
                    raise RuntimeError("Windows runner did not provide VCPKG_INSTALLATION_ROOT")
                platform_build_options.extend(
                    [
                        f"-DCMAKE_TOOLCHAIN_FILE={Path(vcpkg_root) / 'scripts' / 'buildsystems' / 'vcpkg.cmake'}",
                        "-DVCPKG_TARGET_TRIPLET=x64-windows-static",
                        "-DCMAKE_POLICY_DEFAULT_CMP0091=NEW",
                        "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded$<$<CONFIG:Debug>:Debug>",
                    ]
                )
                cmake_command.extend(platform_build_options)
            # Normalize common timestamp/locale sources; signer still requires
            # byte-identical output from the separately scheduled runner.
            reproducible_env = os.environ.copy()
            reproducible_env.update(
                {
                    "SOURCE_DATE_EPOCH": "0",
                    "TZ": "UTC",
                    "LC_ALL": "C",
                    "LANG": "C",
                    "ZERO_AR_DATE": "1",
                }
            )
            run(cmake_command, env=reproducible_env, timeout=600)
            run(
                ["cmake", "--build", str(build_dir), "--config", "Release", "--parallel", "2"],
                env=reproducible_env,
                timeout=2400,
            )

            executable_candidates = [
                build_dir / executable_name,
                build_dir / "Release" / executable_name,
            ]
            executable = next((candidate for candidate in executable_candidates if candidate.is_file()), None)
            if executable is None:
                raise RuntimeError("built QuadriFlow executable was not found in the expected CMake output paths")

            if not args.build_only:
                fixture = repository_root / "crates" / "core" / "tests" / "fixtures" / "quadriflow_cube_triangles.obj"
                if not fixture.is_file():
                    raise RuntimeError(f"synthetic cube fixture is missing: {fixture}")
                run(
                    [str(executable), "-i", str(fixture), "-o", str(output_obj), "-f", "12", "-seed", "0"],
                    timeout=180,
                )
                if not output_obj.is_file() or output_obj.stat().st_size == 0:
                    raise RuntimeError("QuadriFlow did not produce a nonempty cube OBJ")
    
                env = os.environ.copy()
                env["MESHHARBOR_QUADRIFLOW_OBJ_OUTPUT"] = str(output_obj)
                run(
                    [
                        "cargo",
                        "test",
                        "-p",
                        "meshharbor-core",
                        "--locked",
                        "--test",
                        "quadriflow_output",
                        "--",
                        "--ignored",
                        "--nocapture",
                    ],
                    cwd=repository_root,
                    env=env,
                    timeout=2400,
                )
        except Exception as error:  # Preserve diagnostic output as an Actions artifact.
            log.write(f"\nERROR: {type(error).__name__}: {error}\n")
            log.flush()
            raise

    binary_path = artifact_dir / executable_name
    shutil.copy2(executable, binary_path)
    if platform_name == "Windows":
        dependency_versions = version(
            [
                os.path.join(os.environ.get("VCPKG_INSTALLATION_ROOT", "C:\\vcpkg"), "vcpkg.exe"),
                "list",
            ]
        )
    elif platform_name == "macOS":
        dependency_versions = version(["brew", "list", "--versions", "eigen", "boost"])
    else:
        dependency_versions = version(["dpkg-query", "--show", "libeigen3-dev", "libboost-dev"])
    tool_versions = {
        "python": platform.python_version(),
        "cmake": version(["cmake", "--version"]).splitlines()[0],
        "rustc": version(["rustc", "--version"]),
        "cargo": version(["cargo", "--version"]),
        "git": version(["git", "--version"]),
        "gh": version(["gh", "version"]),
    }
    fixture = repository_root / "crates" / "core" / "tests" / "fixtures" / "quadriflow_cube_triangles.obj"
    provenance = {
        "purpose": (
            "manual pinned-source candidate build only; QuadriFlow was not executed"
            if args.build_only
            else "isolated CI build and smoke test only; not a product release"
        ),
        "meshharbor_source_commit": os.environ.get("MESHHARBOR_SOURCE_COMMIT"),
        "meshharbor_workflow_commit": os.environ.get("MESHHARBOR_WORKFLOW_COMMIT"),
        "meshharbor_workflow_path": os.environ.get("MESHHARBOR_WORKFLOW_PATH"),
        "build_trigger": os.environ.get("MESHHARBOR_BUILD_TRIGGER"),
        "upstream_repository": QUADRIFLOW_REPOSITORY,
        "upstream_commit": QUADRIFLOW_COMMIT,
        "platform": candidate_platform,
        "build_replica": build_replica,
        "machine": platform.machine(),
        "platform_release": platform.release(),
        "tool_versions": tool_versions,
        "build_dependency_versions": dependency_versions,
        "build_options": [*BUILD_OPTIONS, *(option for option in platform_build_options if not option.startswith("-DCMAKE_TOOLCHAIN_FILE="))],
        "build_mode": "Release",
        "fixture": "crates/core/tests/fixtures/quadriflow_cube_triangles.obj (synthetic cube; no user model)",
        "synthetic_fixture_sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
        "smoke_test": (
            None
            if args.build_only
            else "crates/core/tests/quadriflow_output.rs::validates_quad_output_and_builds_a_glb_preview"
        ),
        "smoke_result": "not-run" if args.build_only else "passed",
        "license_note": (
            "Upstream LICENSE.txt is included. BUILD_FREE_LICENSE=ON requests Eigen's MPL2-only "
            "code path; this is not a complete dependency-license clearance or a redistribution approval."
        ),
        "artifact_note": (
            "Temporary candidate only; a separate clean GitHub-hosted verifier job checks binary identity, "
            "not release approval or license clearance."
            if args.build_only
            else "Unsigned temporary CI evidence; do not treat as a trusted installer or product binary."
        ),
    }
    provenance_path = artifact_dir / "provenance.json"
    provenance_path.write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    candidate_status = {
        "schema_version": 1,
        "candidate_only": True,
        "installable": False,
        "release_ready": False,
        "dependency_closure_verified": False,
        "license_clearance": "not reviewed",
        "quadriFlow_executable_run": not args.build_only,
        "note": (
            "This temporary evidence is not a product package or an installer input. "
            "The product manifest remains empty."
        ),
    }
    (artifact_dir / "CANDIDATE-STATUS.json").write_text(
        json.dumps(candidate_status, indent=2) + "\n", encoding="utf-8"
    )

    write_checksum_manifest(artifact_dir)
    print(f"\nArtifact evidence ready at {artifact_dir}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
