import importlib.util
from pathlib import Path
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


if __name__ == "__main__":
    unittest.main()
