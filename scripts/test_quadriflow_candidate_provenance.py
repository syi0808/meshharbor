from __future__ import annotations

import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import quadriflow_candidate_provenance as candidate


class CandidateProvenanceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.candidate_dir = self.root / "candidate"
        self.independent_candidate_dir = self.root / "independent"
        self.candidate_dir.mkdir()
        self.independent_candidate_dir.mkdir()
        self.source_commit = "a" * 40
        self.workflow_commit = self.source_commit
        self.run_id = 12345
        self.run_attempt = 2
        self.platform = "linux-x86_64"
        self.executable = candidate.PLATFORMS[self.platform]
        (self.candidate_dir / self.executable).write_bytes(b"synthetic executable")
        (self.independent_candidate_dir / self.executable).write_bytes(b"synthetic executable")

    def tearDown(self):
        self.temp.cleanup()

    def write_status(self, directory=None, **overrides):
        status = {
            "schema_version": 1,
            "candidate_only": True,
            "installable": False,
            "release_ready": False,
            "dependency_closure_verified": False,
            "quadriFlow_executable_run": False,
        }
        status.update(overrides)
        directory = directory or self.candidate_dir
        (directory / "CANDIDATE-STATUS.json").write_text(
            json.dumps(status), encoding="utf-8"
        )

    def write_provenance(self, directory=None, replica="primary", **overrides):
        provenance = {
            "purpose": "manual pinned-source candidate build only; QuadriFlow was not executed",
            "meshharbor_source_commit": self.source_commit,
            "meshharbor_workflow_commit": self.workflow_commit,
            "meshharbor_workflow_path": candidate.WORKFLOW_PATH,
            "build_trigger": "workflow_dispatch",
            "upstream_repository": candidate.UPSTREAM_REPOSITORY,
            "upstream_commit": candidate.UPSTREAM_COMMIT,
            "platform": self.platform,
            "build_mode": "Release",
            "build_options": candidate._expected_build_options(self.platform),
            "smoke_test": None,
            "smoke_result": "not-run",
            "build_replica": replica,
        }
        provenance.update(overrides)
        directory = directory or self.candidate_dir
        (directory / "provenance.json").write_text(
            json.dumps(provenance), encoding="utf-8"
        )

    def prepare(self, output):
        return candidate.prepare_candidate(
            Path(__file__).resolve().parents[1],
            self.candidate_dir,
            self.independent_candidate_dir,
            self.executable,
            self.source_commit,
            self.workflow_commit,
            self.run_id,
            self.run_attempt,
            self.platform,
            output,
        )

    def test_predicate_binds_workflow_source_upstream_recipe_and_run(self):
        self.write_status()
        self.write_provenance(replica="primary")
        self.write_status(self.independent_candidate_dir)
        self.write_provenance(self.independent_candidate_dir, replica="independent")
        output = self.root / "predicate.json"
        predicate = self.prepare(output)

        self.assertEqual(predicate["buildDefinition"]["buildType"], candidate.BUILD_TYPE)
        self.assertEqual(
            predicate["buildDefinition"]["externalParameters"]["workflow"],
            {
                "repository": candidate.REPOSITORY_URI,
                "ref": candidate.SOURCE_REF,
                "path": candidate.WORKFLOW_PATH,
            },
        )
        self.assertEqual(
            predicate["buildDefinition"]["externalParameters"]["inputs"],
            {"confirm_candidate": "true"},
        )
        self.assertEqual(
            predicate["buildDefinition"]["internalParameters"]["meshharbor_reproducibility"],
            {
                "strategy": "independent-job-byte-identical",
                "primary_replica": "primary",
                "independent_replica": "independent",
                "independent_build_sha256": hashlib.sha256(
                    b"synthetic executable"
                ).hexdigest(),
            },
        )
        dependencies = predicate["buildDefinition"]["resolvedDependencies"]
        self.assertEqual(len(dependencies), 3)
        self.assertEqual(
            dependencies[0]["digest"], {"gitCommit": self.source_commit}
        )
        self.assertEqual(
            dependencies[1],
            {
                "uri": f"git+{candidate.UPSTREAM_REPOSITORY}@{candidate.UPSTREAM_COMMIT}",
                "digest": {"gitCommit": candidate.UPSTREAM_COMMIT},
            },
        )
        recipe = Path(__file__).resolve().parents[1] / "scripts" / "quadriflow-platform-smoke.py"
        recipe_sha = hashlib.sha256(recipe.read_bytes()).hexdigest()
        self.assertEqual(dependencies[2]["digest"], {"sha256": recipe_sha})
        self.assertEqual(
            predicate["runDetails"]["metadata"]["invocationId"],
            f"{candidate.REPOSITORY_URI}/actions/runs/{self.run_id}/attempts/{self.run_attempt}",
        )
        self.assertEqual(json.loads(output.read_text(encoding="utf-8")), predicate)

    def test_rejects_release_ready_status_and_never_overwrites(self):
        self.write_status(installable=True)
        self.write_provenance(replica="primary")
        self.write_status(self.independent_candidate_dir)
        self.write_provenance(self.independent_candidate_dir, replica="independent")
        with self.assertRaisesRegex(candidate.CandidateProvenanceError, "fail-closed"):
            self.prepare(self.root / "predicate.json")

        self.write_status()
        self.write_status(self.independent_candidate_dir)
        output = self.root / "existing.json"
        output.write_text("keep", encoding="utf-8")
        with self.assertRaisesRegex(candidate.CandidateProvenanceError, "overwrite"):
            self.prepare(output)
        self.assertEqual(output.read_text(encoding="utf-8"), "keep")

    def test_rejects_untrusted_provenance_material_changes(self):
        self.write_status()
        self.write_provenance(replica="primary", upstream_commit="c" * 40)
        self.write_status(self.independent_candidate_dir)
        self.write_provenance(self.independent_candidate_dir, replica="independent")
        with self.assertRaisesRegex(candidate.CandidateProvenanceError, "upstream_commit"):
            self.prepare(self.root / "predicate.json")

    def test_rejects_wrong_platform_executable_pair(self):
        self.write_status()
        self.write_provenance(replica="primary")
        with self.assertRaisesRegex(candidate.CandidateProvenanceError, "platform"):
            candidate.validate_candidate(
                self.candidate_dir,
                "quadriflow.exe",
                self.platform,
                self.source_commit,
                self.workflow_commit,
                "primary",
            )

    def test_rejects_a_substituted_executable_when_independent_build_differs(self):
        self.write_status()
        self.write_provenance(replica="primary")
        self.write_status(self.independent_candidate_dir)
        self.write_provenance(self.independent_candidate_dir, replica="independent")
        (self.independent_candidate_dir / self.executable).write_bytes(
            b"different executable with forged matching metadata"
        )
        with self.assertRaisesRegex(candidate.CandidateProvenanceError, "not byte-identical"):
            self.prepare(self.root / "predicate.json")

    def test_rejects_swapped_replica_evidence(self):
        self.write_status()
        self.write_provenance(replica="independent")
        self.write_status(self.independent_candidate_dir)
        self.write_provenance(self.independent_candidate_dir, replica="primary")
        with self.assertRaisesRegex(candidate.CandidateProvenanceError, "build_replica"):
            self.prepare(self.root / "predicate.json")

    def test_rejects_duplicate_json_keys_and_symlinks(self):
        status = self.candidate_dir / "CANDIDATE-STATUS.json"
        status.write_text(
            '{"schema_version":1,"candidate_only":true,"installable":false,'
            '"installable":false,"release_ready":false,'
            '"dependency_closure_verified":false,"quadriFlow_executable_run":false}',
            encoding="utf-8",
        )
        self.write_provenance(replica="primary")
        self.write_status(self.independent_candidate_dir)
        self.write_provenance(self.independent_candidate_dir, replica="independent")
        with self.assertRaisesRegex(candidate.CandidateProvenanceError, "duplicate"):
            self.prepare(self.root / "predicate.json")

        status.unlink()
        self.write_status()
        self.write_status(self.independent_candidate_dir)
        executable = self.candidate_dir / self.executable
        executable.unlink()
        executable.symlink_to(self.root / "missing-target")
        with self.assertRaisesRegex(candidate.CandidateProvenanceError, "regular file"):
            self.prepare(self.root / "predicate.json")

    def test_rejects_unexpected_workflow_revision(self):
        self.write_status()
        self.write_provenance()
        with self.assertRaisesRegex(candidate.CandidateProvenanceError, "must match"):
            candidate.create_predicate(
                Path(__file__).resolve().parents[1],
                self.source_commit,
                "b" * 40,
                self.run_id,
                self.run_attempt,
                self.platform,
                "a" * 64,
            )


if __name__ == "__main__":
    unittest.main()
