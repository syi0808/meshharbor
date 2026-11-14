from collections import Counter
from pathlib import Path
import re
import unittest


WORKFLOWS = Path(__file__).resolve().parents[1] / ".github" / "workflows"
WORKFLOW = WORKFLOWS / "quadriflow-candidate-build.yml"
BUILD_WORKFLOW = WORKFLOWS / "quadriflow-candidate-build-reusable.yml"
BUILD_JOB_IDS = (
    "build_linux_primary",
    "build_linux_independent",
    "build_macos_primary",
    "build_macos_independent",
    "build_windows_primary",
    "build_windows_independent",
)


def _job_block(workflow: str, name: str) -> str:
    marker = f"  {name}:\n"
    start = workflow.index(marker)
    remainder = workflow[start + len(marker):]
    next_job = re.search(r"(?m)^  [a-z][a-z0-9_-]*:\n", remainder)
    end = len(remainder) if next_job is None else next_job.start()
    return remainder[:end]


class CandidateWorkflowSecurityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.build_workflow = BUILD_WORKFLOW.read_text(encoding="utf-8")
        cls.build = "\n".join(_job_block(cls.workflow, job) for job in BUILD_JOB_IDS)
        cls.sign = _job_block(cls.workflow, "sign-candidate")
        cls.verify = _job_block(cls.workflow, "verify-candidate")

    def test_build_jobs_have_no_signing_capability_or_attestation_action(self):
        self.assertIn("contents: read", self.workflow)
        self.assertIn("contents: read", self.build_workflow)
        self.assertNotIn("id-token: write", self.build)
        self.assertNotIn("id-token: write", self.build_workflow)
        self.assertNotIn("attestations: write", self.build_workflow)
        self.assertNotIn("uses: actions/attest@", self.build_workflow)
        self.assertIn("--build-only", self.build_workflow)

    def test_signer_requires_each_platform_and_replica_from_distinct_named_jobs(self):
        entries = re.findall(
            r"(?ms)^  (build_[a-z_]+):\n.*?^    with:\n"
            r"      runner: ([^\n]+)\n"
            r"      platform: ([^\n]+)\n"
            r"      replica: ([^\n]+)$",
            self.workflow,
        )
        self.assertEqual(
            Counter((platform, replica) for _job, _runner, platform, replica in entries),
            Counter(
                {
                    ("linux-x86_64", "primary"): 1,
                    ("linux-x86_64", "independent"): 1,
                    ("macos-aarch64", "primary"): 1,
                    ("macos-aarch64", "independent"): 1,
                    ("windows-x86_64", "primary"): 1,
                    ("windows-x86_64", "independent"): 1,
                }
            ),
        )
        self.assertIn("MESHHARBOR_BUILD_REPLICA: ${{ inputs.replica }}", self.build_workflow)
        self.assertIn(
            "name: quadriflow-unverified-${{ inputs.platform }}-${{ inputs.replica }}",
            self.build_workflow,
        )
        self.assertIn(
            "artifact_id: ${{ steps.upload_candidate.outputs.artifact-id }}",
            self.build_workflow,
        )
        self.assertIn("value: ${{ jobs.build.outputs.artifact_id }}", self.build_workflow)
        for job_id in BUILD_JOB_IDS:
            self.assertIn(job_id, self.workflow)
            self.assertIn(f"needs.{job_id}.outputs.artifact_id", self.sign)
        self.assertEqual(self.sign.count("artifact-ids: ${{"), 2)
        self.assertIn("steps.select_artifact_ids.outputs.primary", self.sign)
        self.assertIn("steps.select_artifact_ids.outputs.independent", self.sign)
        self.assertIn("[[ ! \"$primary\" =~ ^[1-9][0-9]*$", self.sign)
        self.assertIn('[[ "$primary" == "$independent" ]]', self.sign)
        self.assertNotIn("quadriflow-unverified-", self.sign)
        self.assertIn("--independent-candidate-dir", self.sign)
        self.assertIn("quadriflow-candidate/primary", self.sign)
        self.assertIn("quadriflow-candidate/independent", self.sign)

    def test_fresh_signer_does_not_run_build_recipe_or_candidate_binary(self):
        self.assertIn("id-token: write", self.sign)
        self.assertIn("attestations: write", self.sign)
        self.assertIn("uses: actions/attest@", self.sign)
        self.assertIn("quadriflow_candidate_provenance.py", self.sign)
        self.assertNotIn("quadriflow-platform-smoke.py", self.sign)
        self.assertNotIn("cmake", self.sign.lower())
        self.assertNotRegex(
            self.sign,
            r"(?m)^\s*run:\s*.*\bquadriflow(?:\.exe)?\b",
        )

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
        for workflow in (self.workflow, self.build_workflow):
            lines = workflow.splitlines()
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

    def test_workflow_triggers_and_action_pins_are_fail_closed(self):
        self.assertIn("workflow_dispatch:", self.workflow)
        self.assertNotIn("pull_request:", self.workflow)
        self.assertNotIn("push:", self.workflow)
        self.assertIn("workflow_call:", self.build_workflow)
        action_refs = re.findall(
            r"(?m)^\s+uses: [^@]+@([^\s]+)",
            self.workflow + "\n" + self.build_workflow,
        )
        self.assertGreater(len(action_refs), 0)
        self.assertTrue(all(re.fullmatch(r"[0-9a-f]{40}", ref) for ref in action_refs))
        self.assertNotIn("upload-release-asset", self.workflow.lower())
        self.assertIn("no release or installer activation", self.workflow)


if __name__ == "__main__":
    unittest.main()
