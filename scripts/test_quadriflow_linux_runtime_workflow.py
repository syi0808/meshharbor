"""Check that the stock Linux QA re-verifies staged original bytes."""

import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/quadriflow-linux-runtime-qa.yml"
PAYLOADS = ("quadriflow", "provenance.sigstore.json", "CANDIDATE-STATUS.json")


class LinuxRuntimeWorkflowTests(unittest.TestCase):
    def setUp(self):
        self.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_fresh_verifier_and_runtime_share_staging_directory(self):
        self.assertEqual(self.workflow.count("/quadriflow-fixed-linux-reverified"), 3)
        self.assertIn('--output "$CANDIDATE_DIR/verification.json"', self.workflow)
        self.assertNotIn("quadriflow-linux-reverification.json", self.workflow)
        self.assertLess(
            self.workflow.index("Stage exact payload"),
            self.workflow.index("quadriflow_attestation_verify.py verify"),
        )
        self.assertLess(
            self.workflow.index("quadriflow_attestation_verify.py verify"),
            self.workflow.index("docker pull"),
        )

    @unittest.skipUnless(shutil.which("bash"), "requires Bash")
    def test_staging_does_not_reuse_historical_report(self):
        match = re.search(
            r"      - name: Stage exact payload[^\n]*\n"
            r"(?:(?!      - name:)[^\n]*\n)*?"
            r"        run: \|\n((?:          [^\n]*\n)+)",
            self.workflow,
        )
        self.assertIsNotNone(match)
        staging_command = textwrap.dedent(match[1])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            download = root / "downloaded"
            stage = root / "staged"
            download.mkdir()
            for name in PAYLOADS:
                (download / name).write_bytes(name.encode("ascii"))
            report = download / "verification.json"
            report.write_text("historical report", encoding="ascii")
            env = dict(os.environ, DOWNLOADED_DIR=str(download), CANDIDATE_DIR=str(stage))
            subprocess.run(["bash", "-c", staging_command], env=env, check=True)
            self.assertCountEqual([path.name for path in stage.iterdir()], PAYLOADS)
            for name in PAYLOADS:
                self.assertEqual((stage / name).read_bytes(), (download / name).read_bytes())
            self.assertEqual(report.read_text(encoding="ascii"), "historical report")
            self.assertNotEqual(
                subprocess.run(
                    ["bash", "-c", staging_command], env=env, capture_output=True
                ).returncode,
                0,
            )
