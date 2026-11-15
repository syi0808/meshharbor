import hashlib
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from quadriflow_artifact_id import ArtifactSelectionError, select_artifact_id
from quadriflow_candidate_native_smoke import NativeSmokeError, run_native_smoke


SOURCE_COMMIT = "a" * 40
WORKFLOW_COMMIT = "b" * 40
PLATFORM = "linux-x86_64"


def make_artifacts(root: Path, *, artifact_digest_override: str | None = None):
    artifact_dir = root / "artifact"
    artifact_dir.mkdir()
    executable_bytes = b"synthetic candidate executable bytes"
    bundle_bytes = b"synthetic sigstore bundle"
    executable = artifact_dir / "quadriflow"
    bundle = artifact_dir / "provenance.sigstore.json"
    executable.write_bytes(executable_bytes)
    bundle.write_bytes(bundle_bytes)
    report = {
        "schema_version": 1,
        "artifact": {
            "name": "quadriflow",
            "sha256": artifact_digest_override or hashlib.sha256(executable_bytes).hexdigest(),
        },
        "bundle": {
            "name": "provenance.sigstore.json",
            "sha256": hashlib.sha256(bundle_bytes).hexdigest(),
        },
        "platform": PLATFORM,
        "workflow_run_id": 123,
        "workflow_run_attempt": 1,
        "materials": {"meshharbor_source_commit": SOURCE_COMMIT},
        "policy": {
            "repository": "syi0808/meshharbor",
            "source_ref": "refs/heads/main",
            "source_commit": SOURCE_COMMIT,
            "workflow_commit": WORKFLOW_COMMIT,
            "workflow_trigger": "workflow_dispatch",
            "oidc_issuer": "https://token.actions.githubusercontent.com",
        },
        "installer_ready": False,
    }
    (artifact_dir / "verification.json").write_text(json.dumps(report), encoding="utf-8")
    return artifact_dir, executable


class ArtifactIdSelectionTests(unittest.TestCase):
    def _response(self, artifacts):
        return json.dumps({"total_count": len(artifacts), "artifacts": artifacts}).encode()

    def test_selects_one_exact_unexpired_bounded_id(self):
        response = self._response(
            [
                {"id": 8, "name": "other", "expired": False, "size_in_bytes": 10},
                {"id": 91, "name": "quadriflow-signed-linux-x86_64", "expired": False, "size_in_bytes": 20},
            ]
        )
        self.assertEqual(
            select_artifact_id(response, "quadriflow-signed-linux-x86_64"),
            "91",
        )

    def test_rejects_missing_duplicate_expired_or_oversized_artifact(self):
        valid = {"id": 91, "name": "candidate", "expired": False, "size_in_bytes": 20}
        with self.assertRaises(ArtifactSelectionError):
            select_artifact_id(self._response([]), "candidate")
        with self.assertRaises(ArtifactSelectionError):
            select_artifact_id(self._response([valid, valid]), "candidate")
        with self.assertRaises(ArtifactSelectionError):
            select_artifact_id(
                self._response([{**valid, "expired": True}]),
                "candidate",
            )
        with self.assertRaises(ArtifactSelectionError):
            select_artifact_id(
                self._response([{**valid, "size_in_bytes": 161 * 1024 * 1024}]),
                "candidate",
            )

    def test_rejects_incomplete_api_page_and_duplicate_json_keys(self):
        with self.assertRaises(ArtifactSelectionError):
            select_artifact_id(
                json.dumps({"total_count": 2, "artifacts": [{"id": 1, "name": "candidate"}]}).encode(),
                "candidate",
            )
        with self.assertRaises(ArtifactSelectionError):
            select_artifact_id(
                b'{"total_count":0,"total_count":1,"artifacts":[]}',
                "candidate",
            )


class CandidateNativeSmokeTests(unittest.TestCase):
    def _run(self, root: Path, **overrides):
        artifact_dir, executable = make_artifacts(
            root,
            artifact_digest_override=overrides.pop("artifact_digest_override", None),
        )
        output_dir = root / "native-output"
        args = {
            "artifact_dir": artifact_dir,
            "output_dir": output_dir,
            "platform_key": PLATFORM,
            "source_commit": SOURCE_COMMIT,
            "workflow_commit": WORKFLOW_COMMIT,
            "workflow_run_id": 123,
            "workflow_run_attempt": 1,
            "system": "Linux",
            "machine": "x86_64",
            "which": lambda _name: None,
            **overrides,
        }
        return artifact_dir, executable, output_dir, args

    def test_runs_exact_verified_bytes_with_restricted_environment(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            artifact_dir, executable, output_dir, args = self._run(root)
            calls = []

            def fake_run(command, **kwargs):
                calls.append((command, kwargs))
                output_path = Path(command[command.index("-o") + 1])
                output_path.write_text("v 0 0 0\nf 1 2 3 4\n", encoding="utf-8")
                return SimpleNamespace(returncode=0)

            with patch.dict(
                os.environ,
                {
                    "GH_TOKEN": "sentinel-gh-token",
                    "GITHUB_TOKEN": "sentinel-github-token",
                    "HF_TOKEN": "sentinel-hf-token",
                    "HUGGING_FACE_HUB_TOKEN": "sentinel-hf-hub-token",
                },
            ):
                result = run_native_smoke(**args, runner=fake_run)

            self.assertEqual(len(calls), 1)
            command, kwargs = calls[0]
            self.assertEqual(command[0], str(executable))
            self.assertEqual(
                command[1:],
                [
                    "-i",
                    command[2],
                    "-o",
                    command[4],
                    "-f",
                    "12",
                    "-seed",
                    "0",
                ],
            )
            self.assertFalse(kwargs["shell"])
            self.assertEqual(kwargs["timeout"], 180)
            self.assertFalse(kwargs["check"])
            self.assertEqual(Path(kwargs["cwd"]), output_dir)
            self.assertNotIn("GH_TOKEN", kwargs["env"])
            self.assertNotIn("GITHUB_TOKEN", kwargs["env"])
            self.assertNotIn("HF_TOKEN", kwargs["env"])
            self.assertNotIn("HUGGING_FACE_HUB_TOKEN", kwargs["env"])
            self.assertTrue((output_dir / "cube-quad.obj").is_file())
            self.assertEqual(result["verified_candidate_sha256"], result["executable_sha256_after"])
            self.assertFalse(result["installer_ready"])
            evidence = json.loads((output_dir / "native-smoke.json").read_text(encoding="utf-8"))
            self.assertEqual(evidence["status"], "passed")
            self.assertEqual(evidence["platform"], PLATFORM)
            self.assertEqual(evidence["elf_runtime_report"]["status"], "unavailable")

    def test_captures_linux_readelf_evidence_bound_to_candidate_digest(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            _artifact_dir, executable, output_dir, args = self._run(root)
            inspected = []

            def fake_inspector(command, **kwargs):
                inspected.append((command, kwargs))
                self.assertEqual(
                    hashlib.sha256(executable.read_bytes()).hexdigest(),
                    hashlib.sha256(b"synthetic candidate executable bytes").hexdigest(),
                )
                return SimpleNamespace(returncode=0, stdout=f"readelf output for {command[1]}\n")

            def fake_runner(command, **kwargs):
                output_path = Path(command[command.index("-o") + 1])
                output_path.write_text("v 0 0 0\nf 1 2 3 4\n", encoding="utf-8")
                return SimpleNamespace(returncode=0)

            args["which"] = lambda name: "/usr/bin/readelf" if name == "readelf" else None
            result = run_native_smoke(**args, runner=fake_runner, inspector=fake_inspector)
            elf_metadata = result["elf_runtime_report"]
            report_path = output_dir / elf_metadata["report_file"]
            report = json.loads(report_path.read_text(encoding="utf-8"))
            expected_digest = hashlib.sha256(b"synthetic candidate executable bytes").hexdigest()
            self.assertEqual(elf_metadata["status"], "captured")
            self.assertEqual(elf_metadata["executable_sha256"], expected_digest)
            self.assertEqual(report["executable_sha256"], expected_digest)
            self.assertEqual(len(inspected), 2)
            self.assertEqual([call[0][1] for call in inspected], ["-d", "--version-info"])
            self.assertEqual(report["dynamic_section"], "readelf output for -d\n")
            self.assertEqual(report["symbol_versions"], "readelf output for --version-info\n")

    def test_rejects_readelf_report_modified_by_candidate(self):
        with tempfile.TemporaryDirectory() as temp:
            _artifact_dir, _executable, output_dir, args = self._run(Path(temp))
            args["which"] = lambda name: "/usr/bin/readelf" if name == "readelf" else None

            def inspector(*_args, **_kwargs):
                return SimpleNamespace(returncode=0, stdout="bounded output\n")

            def mutating_run(command, **_kwargs):
                Path(command[command.index("-o") + 1]).write_text("quad output", encoding="utf-8")
                (output_dir / "elf-runtime.json").write_text("changed", encoding="utf-8")
                return SimpleNamespace(returncode=0)

            with self.assertRaisesRegex(NativeSmokeError, "ELF runtime report changed"):
                run_native_smoke(
                    **args,
                    runner=mutating_run,
                    inspector=inspector,
                )

    def test_rejects_wrong_digest_before_execution(self):
        with tempfile.TemporaryDirectory() as temp:
            _artifact_dir, _exe, _out, args = self._run(
                Path(temp),
                artifact_digest_override="0" * 64,
            )
            runner = unittest.mock.Mock()
            with self.assertRaises(NativeSmokeError):
                run_native_smoke(**args, runner=runner)
            runner.assert_not_called()

    def test_rejects_mismatched_run_or_platform_before_execution(self):
        with tempfile.TemporaryDirectory() as temp:
            _artifact_dir, _exe, _out, args = self._run(
                Path(temp),
                workflow_run_id=999,
            )
            runner = unittest.mock.Mock()
            with self.assertRaises(NativeSmokeError):
                run_native_smoke(**args, runner=runner)
            runner.assert_not_called()

        with tempfile.TemporaryDirectory() as temp:
            _artifact_dir, _exe, _out, args = self._run(
                Path(temp),
                system="Darwin",
                machine="arm64",
            )
            runner = unittest.mock.Mock()
            with self.assertRaises(NativeSmokeError):
                run_native_smoke(**args, runner=runner)
            runner.assert_not_called()

    def test_rejects_executable_modified_during_smoke(self):
        with tempfile.TemporaryDirectory() as temp:
            _artifact_dir, executable, _out, args = self._run(Path(temp))

            def mutating_run(command, **kwargs):
                Path(command[command.index("-o") + 1]).write_text("quad output", encoding="utf-8")
                executable.chmod(0o700)
                executable.write_bytes(b"changed candidate")
                return SimpleNamespace(returncode=0)

            with self.assertRaisesRegex(NativeSmokeError, "changed during native execution"):
                run_native_smoke(**args, runner=mutating_run)

    def test_rejects_nonzero_or_missing_output(self):
        with tempfile.TemporaryDirectory() as temp:
            _artifact_dir, _exe, _out, args = self._run(Path(temp))
            with self.assertRaisesRegex(NativeSmokeError, "nonzero"):
                run_native_smoke(
                    **args,
                    runner=lambda *_args, **_kwargs: SimpleNamespace(returncode=1),
                )

        with tempfile.TemporaryDirectory() as temp:
            _artifact_dir, _exe, _out, args = self._run(Path(temp))
            with self.assertRaisesRegex(NativeSmokeError, "cube output"):
                run_native_smoke(
                    **args,
                    runner=lambda *_args, **_kwargs: SimpleNamespace(returncode=0),
                )


if __name__ == "__main__":
    unittest.main()
