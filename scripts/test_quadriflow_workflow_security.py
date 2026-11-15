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


def _artifact_id_download_steps(job: str) -> list[str]:
    starts = list(re.finditer(r"(?m)^      - name: ", job))
    steps = []
    for index, start in enumerate(starts):
        end = starts[index + 1].start() if index + 1 < len(starts) else len(job)
        step = job[start.start():end]
        if (
            "uses: actions/download-artifact@" in step
            and "artifact-ids: ${{" in step
        ):
            steps.append(step)
    return steps


class CandidateWorkflowSecurityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.build_workflow = BUILD_WORKFLOW.read_text(encoding="utf-8")
        cls.build = "\n".join(_job_block(cls.workflow, job) for job in BUILD_JOB_IDS)
        cls.sign = _job_block(cls.workflow, "sign-candidate")
        cls.pre_sign_gate = _job_block(cls.workflow, "pre-sign-gate")
        cls.verify = _job_block(cls.workflow, "verify-candidate")
        cls.native = _job_block(cls.workflow, "native-smoke")

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


    def test_id_based_artifact_downloads_flatten_to_expected_roots(self):
        for label, job, expected_count in (
            ("pre-sign gate", self.pre_sign_gate, 6),
            ("signer", self.sign, 2),
            ("native smoke", self.native, 1),
        ):
            with self.subTest(job=label):
                downloads = _artifact_id_download_steps(job)
                self.assertEqual(len(downloads), expected_count)
                self.assertTrue(
                    all("merge-multiple: true" in step for step in downloads),
                    f"{label} must extract artifact members into the configured path",
                )

    def test_signing_waits_for_read_only_all_platform_pair_gate(self):
        gate = self.pre_sign_gate
        self.assertIn("contents: read", gate)
        self.assertNotIn("id-token: write", gate)
        self.assertNotIn("attestations: write", gate)
        self.assertNotIn("uses: actions/attest@", gate)
        self.assertIn("quadriflow_candidate_provenance.py prepare", gate)
        self.assertEqual(gate.count("artifact-ids: ${{"), 6)
        self.assertIn("len(set(artifact_ids))", gate)
        self.assertIn('re.fullmatch(r"[1-9][0-9]*", value)', gate)

        for job_id in BUILD_JOB_IDS:
            self.assertIn(job_id, gate)
            self.assertIn(f"needs.{job_id}.outputs.artifact_id", gate)
        for platform in ("linux-x86_64", "macos-aarch64", "windows-x86_64"):
            self.assertIn(f"--platform {platform}", gate)
        for provenance_arg in (
            "--source-commit",
            "--workflow-commit",
            "--workflow-run-id",
            "--workflow-run-attempt",
            "--independent-candidate-dir",
        ):
            self.assertIn(provenance_arg, gate)

        self.assertIn("- pre-sign-gate", self.sign)
        self.assertIn("--independent-candidate-dir", self.sign)
        self.assertIn("quadriflow_candidate_provenance.py", self.sign)
        self.assertNotRegex(
            gate,
            r"(?m)^\s*run:\s*.*\bquadriflow(?:\.exe)?\b",
        )

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

    def test_native_smoke_uses_same_immutable_signed_bytes_without_signing_capability(self):
        self.assertIn("needs: verify-candidate", self.native)
        self.assertIn("runs-on: ${{ matrix.runner }}", self.native)
        self.assertIn("actions: read", self.native)
        self.assertIn("attestations: read", self.native)
        self.assertIn("contents: read", self.native)
        self.assertNotIn("id-token: write", self.native)
        self.assertNotIn("attestations: write", self.native)
        self.assertIn("linux-x86_64", self.native)
        self.assertIn("macos-aarch64", self.native)
        self.assertIn("windows-x86_64", self.native)
        self.assertIn("gh api", self.native)
        self.assertIn("quadriflow_artifact_id.py", self.native)
        self.assertIn(
            "artifact-ids: ${{ steps.select_signed_artifact.outputs.artifact_id }}",
            self.native,
        )
        self.assertIn("merge-multiple: true", self.native)
        self.assertIn("quadriflow_attestation_verify.py verify", self.native)
        self.assertIn("quadriflow_candidate_native_smoke.py", self.native)
        self.assertIn("cargo test -p meshharbor-core --locked --test quadriflow_output", self.native)
        self.assertIn("quadriflow-native-smoke-${{ matrix.platform }}", self.native)
        self.assertIn("path: ${{ runner.temp }}/quadriflow-native-smoke/*.json", self.native)
        self.assertNotIn("binutils", self.native.lower())
        self.assertNotIn("--build-only", self.native)

        run_marker = "      - name: Run the already-verified candidate with a minimal environment\n"
        run_start = self.native.index(run_marker)
        run_rest = self.native[run_start + len(run_marker):]
        next_step = re.search(r"(?m)^      - name: ", run_rest)
        run_step = run_rest[: next_step.start()] if next_step else run_rest
        self.assertNotIn("GH_TOKEN:", run_step)
        self.assertNotIn("GITHUB_TOKEN:", run_step)

    def test_setup_python_cache_input_omitted_when_caching_is_disabled(self):
        for workflow in (self.workflow, self.build_workflow):
            lines = workflow.splitlines()
            for index, line in enumerate(lines):
                if "uses: actions/setup-python@" not in line:
                    continue
                following = lines[index + 1:index + 6]
                self.assertNotRegex(
                    "\n".join(following),
                    r"(?im)^\s*cache:\s*false\s*$",
                    "setup-python rejects a boolean false cache input; omit it to disable caching",
                )

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
