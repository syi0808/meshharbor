import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

import quadriflow_attestation_verify as q


class CandidateVerifierTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.candidate = self.root / "candidate"
        self.candidate.mkdir()
        self.binary = self.candidate / "quadriflow"
        self.binary.write_bytes(b"synthetic executable bytes")
        (self.candidate / "CANDIDATE-STATUS.json").write_text(
            json.dumps({
                "candidate_only": True,
                "installable": False,
                "release_ready": False,
                "dependency_closure_verified": False,
            }),
            encoding="utf-8",
        )
        self.bundle = self.root / "bundle.json"
        self.bundle.write_text(
            json.dumps({"mediaType": q.BUNDLE_MEDIA_TYPE}),
            encoding="utf-8",
        )
        self.output = self.candidate / "verification.json"
        self.source_commit = "a" * 40
        self.workflow_commit = "b" * 40

    def tearDown(self):
        self.temp.cleanup()

    def cli_result(
        self,
        *,
        source_repository_id=q.REPOSITORY_ID,
        owner_id=q.OWNER_ID,
        source_ref=q.SOURCE_REF,
        source_commit=None,
        workflow_commit=None,
        runner="github-hosted",
        trigger="workflow_dispatch",
        subject_digest=None,
        predicate_type=q.PREDICATE_TYPE,
    ):
        artifact_digest = hashlib.sha256(self.binary.read_bytes()).hexdigest()
        certificate = {
            "issuer": q.OIDC_ISSUER,
            "subjectAlternativeName": q.WORKFLOW_SAN,
            "buildSignerURI": q.WORKFLOW_SAN,
            "buildSignerDigest": workflow_commit or self.workflow_commit,
            "sourceRepositoryURI": q.REPOSITORY_URI,
            "sourceRepositoryOwnerURI": q.OWNER_URI,
            "sourceRepositoryIdentifier": source_repository_id,
            "sourceRepositoryOwnerIdentifier": owner_id,
            "sourceRepositoryRef": source_ref,
            "sourceRepositoryDigest": source_commit or self.source_commit,
            "runnerEnvironment": runner,
            "buildTrigger": trigger,
        }
        return json.dumps([
            {
                "verificationResult": {
                    "signature": {"certificate": certificate},
                    "verifiedTimestamps": [{"source": "transparency-log"}],
                    "statement": {
                        "subject": [{
                            "name": "quadriflow",
                            "digest": {"sha256": subject_digest or artifact_digest},
                        }],
                        "predicateType": predicate_type,
                        "predicate": {},
                    },
                }
            }
        ])

    def fake_run(self, output=None, verify_returncode=0):
        calls = []

        def run(argv):
            calls.append(argv)
            if argv == ["gh", "version"]:
                return SimpleNamespace(returncode=0, stdout="gh version 2.96.0 (test)\n", stderr="")
            if argv == ["gh", "attestation", "trusted-root"]:
                return SimpleNamespace(
                    returncode=0,
                    stdout='{"mediaType":"application/vnd.dev.sigstore.trustedroot+json;version=0.1"}\n',
                    stderr="",
                )
            if len(argv) > 2 and argv[1:3] == ["attestation", "verify"]:
                return SimpleNamespace(
                    returncode=verify_returncode,
                    stdout=output or self.cli_result(),
                    stderr="verification failed" if verify_returncode else "",
                )
            self.fail(f"unexpected command: {argv}")
        return calls, run

    def test_fixed_policy_is_passed_and_verified_bundle_is_copied(self):
        calls, fake = self.fake_run()
        with patch.object(q, "_run", side_effect=fake):
            report = q.verify_attestation(
                self.binary,
                self.bundle,
                self.source_commit,
                self.workflow_commit,
                self.output,
            )

        self.assertEqual(len(calls), 3)
        command = calls[2]
        self.assertEqual(command[0:4], ["gh", "attestation", "verify", str(self.binary.resolve())])
        self.assertIn("--repo", command)
        self.assertIn(q.REPOSITORY, command)
        self.assertIn("--signer-workflow", command)
        self.assertIn(f"{q.REPOSITORY}/{q.WORKFLOW_PATH}", command)
        self.assertIn("--source-ref", command)
        self.assertIn(q.SOURCE_REF, command)
        self.assertIn("--custom-trusted-root", command)
        self.assertIn("--cert-oidc-issuer", command)
        self.assertIn(q.OIDC_ISSUER, command)
        self.assertIn("--predicate-type", command)
        self.assertIn(q.PREDICATE_TYPE, command)
        self.assertIn("--source-digest", command)
        self.assertIn(self.source_commit, command)
        self.assertIn("--signer-digest", command)
        self.assertIn(self.workflow_commit, command)
        self.assertIn("--deny-self-hosted-runners", command)
        self.assertEqual(report["installer_ready"], False)
        self.assertTrue((self.candidate / "provenance.sigstore.json").is_file())
        self.assertTrue(self.output.is_file())
        checksum_lines = (self.candidate / "SHA256SUMS").read_text(encoding="ascii").splitlines()
        self.assertTrue(any("provenance.sigstore.json" in line for line in checksum_lines))
        self.assertTrue(any("verification.json" in line for line in checksum_lines))

    def test_rejects_wrong_repository_id_after_cli_verification(self):
        calls, fake = self.fake_run(output=self.cli_result(source_repository_id="999"))
        with patch.object(q, "_run", side_effect=fake):
            with self.assertRaisesRegex(q.VerificationError, "repository id"):
                q.verify_attestation(
                    self.binary,
                    self.bundle,
                    self.source_commit,
                    self.workflow_commit,
                    self.output,
                )
        self.assertFalse((self.candidate / "provenance.sigstore.json").exists())

    def test_rejects_non_main_ref_wrong_source_commit_and_owner_id(self):
        for output in (
            self.cli_result(source_ref="refs/heads/feature"),
            self.cli_result(source_commit="c" * 40),
            self.cli_result(owner_id="999"),
        ):
            with self.subTest(output=output):
                calls, fake = self.fake_run(output=output)
                with patch.object(q, "_run", side_effect=fake):
                    with self.assertRaises(q.VerificationError):
                        q.verify_attestation(
                            self.binary,
                            self.bundle,
                            self.source_commit,
                            self.workflow_commit,
                            self.output,
                        )
                self.assertFalse((self.candidate / "provenance.sigstore.json").exists())

    def test_rejects_wrong_workflow_revision_and_runner(self):
        for output in (
            self.cli_result(workflow_commit="c" * 40),
            self.cli_result(runner="self-hosted"),
            self.cli_result(trigger="push"),
        ):
            with self.subTest(output=output):
                calls, fake = self.fake_run(output=output)
                with patch.object(q, "_run", side_effect=fake):
                    with self.assertRaises(q.VerificationError):
                        q.verify_attestation(
                            self.binary,
                            self.bundle,
                            self.source_commit,
                            self.workflow_commit,
                            self.output,
                        )
                self.assertFalse((self.candidate / "provenance.sigstore.json").exists())

    def test_rejects_wrong_subject_and_predicate(self):
        for output in (
            self.cli_result(subject_digest="0" * 64),
            self.cli_result(predicate_type="https://example.invalid/predicate"),
        ):
            with self.subTest(output=output):
                calls, fake = self.fake_run(output=output)
                with patch.object(q, "_run", side_effect=fake):
                    with self.assertRaises(q.VerificationError):
                        q.verify_attestation(
                            self.binary,
                            self.bundle,
                            self.source_commit,
                            self.workflow_commit,
                            self.output,
                        )
                self.assertFalse((self.candidate / "provenance.sigstore.json").exists())

    def test_cli_failure_does_not_write_bundle_or_report(self):
        calls, fake = self.fake_run(verify_returncode=1)
        with patch.object(q, "_run", side_effect=fake):
            with self.assertRaisesRegex(q.VerificationError, "verification failed"):
                q.verify_attestation(
                    self.binary,
                    self.bundle,
                    self.source_commit,
                    self.workflow_commit,
                    self.output,
                )
        self.assertFalse((self.candidate / "provenance.sigstore.json").exists())
        self.assertFalse(self.output.exists())

    def test_rejects_uninstallable_status_if_it_becomes_release_ready(self):
        (self.candidate / "CANDIDATE-STATUS.json").write_text(
            json.dumps({
                "candidate_only": True,
                "installable": True,
                "release_ready": True,
                "dependency_closure_verified": True,
            }),
            encoding="utf-8",
        )
        calls, fake = self.fake_run()
        with patch.object(q, "_run", side_effect=fake):
            with self.assertRaisesRegex(q.VerificationError, "non-release gates"):
                q.verify_attestation(
                    self.binary,
                    self.bundle,
                    self.source_commit,
                    self.workflow_commit,
                    self.output,
                )
        self.assertEqual(calls, [])

    def test_rejects_malformed_commits_before_cli(self):
        calls, fake = self.fake_run()
        with patch.object(q, "_run", side_effect=fake):
            with self.assertRaises(q.VerificationError):
                q.verify_attestation(
                    self.binary,
                    self.bundle,
                    "A" * 40,
                    self.workflow_commit,
                    self.output,
                )
        self.assertEqual(calls, [])

    def test_rejects_old_cli_version(self):
        calls = []

        def fake(argv):
            calls.append(argv)
            return SimpleNamespace(returncode=0, stdout="gh version 2.95.0 (test)\n", stderr="")

        with patch.object(q, "_run", side_effect=fake):
            with self.assertRaisesRegex(q.VerificationError, "2.96.0"):
                q.verify_attestation(
                    self.binary,
                    self.bundle,
                    self.source_commit,
                    self.workflow_commit,
                    self.output,
                )
        self.assertEqual(calls, [["gh", "version"]])


if __name__ == "__main__":
    unittest.main()
