#!/usr/bin/env python3
"""Build pinned QuadriFlow and validate one synthetic cube per CI platform."""

from __future__ import annotations

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


def main() -> int:
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
            if os.name == "nt":
                vcpkg_root = os.environ.get("VCPKG_INSTALLATION_ROOT") or os.environ.get("VCPKG_ROOT")
                if not vcpkg_root:
                    raise RuntimeError("Windows runner did not provide VCPKG_INSTALLATION_ROOT")
                cmake_command.extend(
                    [
                        f"-DCMAKE_TOOLCHAIN_FILE={Path(vcpkg_root) / 'scripts' / 'buildsystems' / 'vcpkg.cmake'}",
                        "-DVCPKG_TARGET_TRIPLET=x64-windows-static",
                    ]
                )
            run(cmake_command, timeout=600)
            run(["cmake", "--build", str(build_dir), "--config", "Release", "--parallel", "2"], timeout=2400)

            executable_candidates = [
                build_dir / executable_name,
                build_dir / "Release" / executable_name,
            ]
            executable = next((candidate for candidate in executable_candidates if candidate.is_file()), None)
            if executable is None:
                raise RuntimeError("built QuadriFlow executable was not found in the expected CMake output paths")

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
    }
    fixture = repository_root / "crates" / "core" / "tests" / "fixtures" / "quadriflow_cube_triangles.obj"
    provenance = {
        "purpose": "isolated CI build and smoke test only; not a product release",
        "upstream_repository": QUADRIFLOW_REPOSITORY,
        "upstream_commit": QUADRIFLOW_COMMIT,
        "platform": platform_name,
        "machine": platform.machine(),
        "platform_release": platform.release(),
        "tool_versions": tool_versions,
        "build_dependency_versions": dependency_versions,
        "build_options": BUILD_OPTIONS,
        "build_mode": "Release",
        "fixture": "crates/core/tests/fixtures/quadriflow_cube_triangles.obj (synthetic cube; no user model)",
        "synthetic_fixture_sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
        "smoke_test": "crates/core/tests/quadriflow_output.rs::validates_quad_output_and_builds_a_glb_preview",
        "smoke_result": "passed",
        "license_note": (
            "Upstream LICENSE.txt is included. BUILD_FREE_LICENSE=ON requests Eigen's MPL2-only "
            "code path; this is not a complete dependency-license clearance or a redistribution approval."
        ),
        "artifact_note": "Unsigned temporary CI evidence; do not treat as a trusted installer or product binary.",
    }
    provenance_path = artifact_dir / "provenance.json"
    provenance_path.write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")

    checksums = []
    for path in sorted(artifact_dir.iterdir()):
        if path.name == "SHA256SUMS" or not path.is_file():
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        checksums.append(f"{digest}  {path.name}")
    (artifact_dir / "SHA256SUMS").write_text("\n".join(checksums) + "\n", encoding="ascii")
    print(f"\nArtifact evidence ready at {artifact_dir}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
