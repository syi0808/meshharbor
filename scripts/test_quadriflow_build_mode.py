import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("quadriflow-platform-smoke.py")
SPEC = importlib.util.spec_from_file_location("quadriflow_platform_smoke", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load QuadriFlow build helper")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class BuildModeTests(unittest.TestCase):
    def test_build_only_is_explicit(self):
        self.assertTrue(MODULE.build_parser().parse_args(["--build-only"]).build_only)

    def test_existing_smoke_default_is_unchanged(self):
        self.assertFalse(MODULE.build_parser().parse_args([]).build_only)

    def test_checksum_manifest_includes_nested_notice_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            artifact_dir = Path(temporary)
            notice = artifact_dir / "third-party-notices" / "upstream" / "LICENSE"
            notice.parent.mkdir(parents=True)
            notice.write_text("upstream notice\n", encoding="utf-8")
            (artifact_dir / "provenance.json").write_text("{}\n", encoding="utf-8")

            MODULE.write_checksum_manifest(artifact_dir)

            lines = (artifact_dir / "SHA256SUMS").read_text(encoding="ascii").splitlines()
            expected = hashlib.sha256(notice.read_bytes()).hexdigest()
            self.assertIn(f"{expected}  third-party-notices/upstream/LICENSE", lines)


if __name__ == "__main__":
    unittest.main()
