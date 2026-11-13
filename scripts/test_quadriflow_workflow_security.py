from pathlib import Path
import re
import unittest


WORKFLOW = (
    Path(__file__).resolve().parents[1]
    / ".github"
    / "workflows"
    / "quadriflow-candidate-build.yml"
)


def _job_block(workflow: str, name: str) -> str:
    marker = f"  {name}:\n"
    start = workflow.index(marker)
    remainder = workflow[start + len(marker):]
    next_job = re.search(r"(?m)^  [a-z][a-z0-9-]*:\n", remainder)
    end = len(remainder) if next_job is None else next_job.start()
    return remainder[:end]


class CandidateWorkflowSecurityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.build = _job_block(cls.workflow, "build-candidate")
        cls.sign = _job_block(cls.workflow, "sign-candidate")
        cls.verify = _job_block(cls.workflow, "verify-candidate")

    def test_build_job_has_no_signing_capability_or_attestation_action(self):
        self.assertIn("contents: read", self.build)
        self.assertNotIn("id-token: write", self.build)
        self.assertNotIn("attestations: write", self.build)
        self.assertNotIn("uses: actions/attest@", self.build)
        self.assertIn("--build-only", self.build)

    def test_fresh_signer_does_not_run_build_recipe_or_candidate_binary(self):
        self.assertIn("id-token: write", self.sign)
        self.assertIn("attestations: write", self.sign)
        self.assertIn("uses: actions/attest@", self.sign)
        self.assertIn("quadriflow_candidate_provenance.py", self.sign)
        self.assertNotIn("quadriflow-platform-smoke.py", self.sign)
        self.assertNotIn("cmake", self.sign.lower())
        self.assertIn("without executing it", self.sign.lower())

    def test_fresh_verifier_has_read_only_attestation_permission(self):
        self.assertIn("attestations: read", self.verify)
        self.assertNotIn("id-token: write", self.verify)
        self.assertNotIn("attestations: write", self.verify)
        self.assertNotIn("uses: actions/attest@", self.verify)
        self.assertIn("quadriflow_attestation_verify.py", self.verify)
        self.assertIn("--workflow-run-id", self.verify)
        self.assertIn("--workflow-run-attempt", self.verify)
        self.assertIn("--platform", self.verify)

    def test_run_block_scalars_have_indented_command_bodies(self):
        lines = self.workflow.splitlines()
        for index, line in enumerate(lines):
            match = re.match(r"^( *)run:\s*[|>][+-]?\s*$", line)
            if match is None:
                continue
            body = next(
                (candidate for candidate in lines[index + 1:] if candidate.strip()),
                "",
            )
            body_indent = len(body) - len(body.lstrip())
            self.assertGreater(
                body_indent,
                len(match.group(1)),
                f"block scalar at line {index + 1} has no indented body",
            )

    def test_candidate_workflow_is_manual_and_all_actions_are_commit_pinned(self):
        self.assertIn("workflow_dispatch:", self.workflow)
        self.assertNotIn("pull_request:", self.workflow)
        self.assertNotIn("push:", self.workflow)
        action_refs = re.findall(r"(?m)^\s+uses: [^@]+@([^\s]+)", self.workflow)
        self.assertGreater(len(action_refs), 0)
        self.assertTrue(all(re.fullmatch(r"[0-9a-f]{40}", ref) for ref in action_refs))
        self.assertNotIn("upload-release-asset", self.workflow.lower())
        self.assertIn("no release or installer activation", self.workflow)


if __name__ == "__main__":
    unittest.main()
