#!/usr/bin/env python3
"""Exercise deterministic package assembly and the real Rust USTAR installer parser."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "scripts" / "fixtures" / "quadriflow-package-roundtrip"
PACKAGER = ROOT / "scripts" / "quadriflow_ustar_package.py"
RUST_TEST = "quadriflow_installer::tests::package_recipe_round_trips_through_installer_parser"
SYNTHETIC_COMMON_INPUTS = {
    "THIRD-PARTY-NOTICES.md": "synthetic-notices.txt",
    "sbom.spdx.json": "synthetic-sbom.json",
    "provenance.json": "synthetic-provenance.json",
    "provenance.sigstore.json": "synthetic-signature.json",
}


class UstarPackageRoundTripTests(unittest.TestCase):
    def _prepare_payload_dir(self, parent: Path, platform: str) -> Path:
        payload_dir = parent / "payload"
        payload_dir.mkdir()
        executable_name = "quadriflow.exe" if platform == "windows-x86_64" else "quadriflow"
        inputs = {executable_name: "synthetic-executable.bin", **SYNTHETIC_COMMON_INPUTS}
        for output_name, fixture_name in inputs.items():
            shutil.copyfile(FIXTURES / fixture_name, payload_dir / output_name)
        return payload_dir

    def test_deterministic_archive_is_accepted_by_rust_installer(self):
        with tempfile.TemporaryDirectory(prefix="meshharbor-qf-ustar-roundtrip-") as temporary:
            temp_root = Path(temporary)
            cargo = shutil.which("cargo")
            self.assertIsNotNone(cargo, "cargo must be available for the Rust parser round-trip")
            for platform in ("linux-x86_64", "macos-aarch64", "windows-x86_64"):
                with self.subTest(platform=platform):
                    platform_root = temp_root / platform
                    platform_root.mkdir()
                    payload_dir = self._prepare_payload_dir(platform_root, platform)
                    archive_paths = [platform_root / "first.tar", platform_root / "second.tar"]
                    reports = []
                    for archive_path in archive_paths:
                        completed = subprocess.run(
                            [
                                sys.executable,
                                str(PACKAGER),
                                "--payload-dir",
                                str(payload_dir),
                                "--platform",
                                platform,
                                "--output",
                                str(archive_path),
                            ],
                            cwd=ROOT,
                            check=True,
                            capture_output=True,
                            text=True,
                        )
                        reports.append(json.loads(completed.stdout))

                    self.assertEqual(archive_paths[0].read_bytes(), archive_paths[1].read_bytes())
                    self.assertEqual(reports[0], reports[1])
                    self.assertFalse(reports[0]["installable"])
                    self.assertFalse(reports[0]["release_ready"])
                    self.assertFalse(reports[0]["dependency_closure_verified"])
                    self.assertEqual(
                        reports[0]["sha256"],
                        hashlib.sha256(archive_paths[0].read_bytes()).hexdigest(),
                    )
                    executable_name = "quadriflow.exe" if platform == "windows-x86_64" else "quadriflow"
                    expected_payloads = [
                        f"bin/{executable_name}",
                        "THIRD-PARTY-NOTICES.md",
                        "sbom.spdx.json",
                        "provenance.json",
                        "provenance.sigstore.json",
                    ]
                    self.assertEqual(
                        [payload["path"] for payload in reports[0]["payloads"]],
                        expected_payloads,
                    )
                    expected_members = ["bin", *expected_payloads]
                    expected_modes = [0o755, 0o755, 0o644, 0o644, 0o644, 0o644]
                    with tarfile.open(archive_paths[0], "r:") as archive:
                        members = archive.getmembers()
                        self.assertEqual([member.name for member in members], expected_members)
                        for member, mode in zip(members, expected_modes, strict=True):
                            self.assertIn(member.type, (tarfile.DIRTYPE, tarfile.REGTYPE))
                            self.assertEqual(member.mode, mode)
                            self.assertEqual(member.uid, 0)
                            self.assertEqual(member.gid, 0)
                            self.assertEqual(member.mtime, 0)
                            self.assertEqual(member.uname, "")
                            self.assertEqual(member.gname, "")

                    environment = os.environ.copy()
                    environment["MESHHARBOR_TEST_QUADRIFLOW_PACKAGE_ARCHIVE"] = str(archive_paths[0])
                    environment["MESHHARBOR_TEST_QUADRIFLOW_PACKAGE_PLATFORM"] = platform
                    result = subprocess.run(
                        [
                            str(cargo),
                            "test",
                            "--locked",
                            "-p",
                            "meshharbor-core",
                            "--lib",
                            RUST_TEST,
                            "--",
                            "--ignored",
                            "--exact",
                        ],
                        cwd=ROOT,
                        env=environment,
                        check=False,
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_rejects_unexpected_payload_and_existing_output(self):
        with tempfile.TemporaryDirectory(prefix="meshharbor-qf-ustar-invalid-") as temporary:
            temp_root = Path(temporary)
            payload_dir = self._prepare_payload_dir(temp_root, "linux-x86_64")
            (payload_dir / "extra.txt").write_text("unexpected", encoding="utf-8")
            output = temp_root / "package.tar"
            rejected = subprocess.run(
                [
                    sys.executable,
                    str(PACKAGER),
                    "--payload-dir",
                    str(payload_dir),
                    "--platform",
                    "linux-x86_64",
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(rejected.returncode, 2)
            self.assertFalse(output.exists())

            (payload_dir / "extra.txt").unlink()
            output.write_bytes(b"do not overwrite")
            rejected_existing = subprocess.run(
                [
                    sys.executable,
                    str(PACKAGER),
                    "--payload-dir",
                    str(payload_dir),
                    "--platform",
                    "linux-x86_64",
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(rejected_existing.returncode, 2)
            self.assertEqual(output.read_bytes(), b"do not overwrite")

    def test_rejects_symlink_payload(self):
        with tempfile.TemporaryDirectory(prefix="meshharbor-qf-ustar-symlink-") as temporary:
            temp_root = Path(temporary)
            payload_dir = self._prepare_payload_dir(temp_root, "linux-x86_64")
            executable = payload_dir / "quadriflow"
            executable.unlink()
            try:
                executable.symlink_to(FIXTURES / "synthetic-executable.bin")
            except (OSError, NotImplementedError) as error:
                self.skipTest(f"symlinks are unavailable in this environment: {error}")

            output = temp_root / "package.tar"
            rejected = subprocess.run(
                [
                    sys.executable,
                    str(PACKAGER),
                    "--payload-dir",
                    str(payload_dir),
                    "--platform",
                    "linux-x86_64",
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(rejected.returncode, 2)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
