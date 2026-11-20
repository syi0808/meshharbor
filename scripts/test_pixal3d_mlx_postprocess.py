import runpy
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, call, patch


SOURCE = Path(__file__).resolve().parents[1] / "crates/core/src/pixal3d_mlx_postprocess.py"
POSTPROCESS = runpy.run_path(str(SOURCE))["_cleanup_and_simplify_mesh"]


class Pixal3dMlxPostprocessTests(unittest.TestCase):
    def test_filter_original_parts_before_simplifying_and_repair_only_afterward(self):
        for keep_largest, filter_name in (
            (False, "remove_small_components"),
            (True, "keep_largest_component"),
        ):
            with self.subTest(keep_largest=keep_largest):
                steps = Mock()
                filtered = getattr(steps, filter_name)
                steps.remove_duplicate_faces.return_value = ("dedup vertices", "dedup faces")
                filtered.return_value = ("filtered vertices", "filtered faces")
                steps.simplify.return_value = ("simplified vertices", "simplified faces")
                steps.cleanup_mesh.return_value = ("repaired vertices", "repaired faces")
                steps.orient_faces_by_adjacency.return_value = ("repaired vertices", "oriented faces")
                modules = {
                    "generate": SimpleNamespace(_cleanup_and_simplify_mesh=steps.simplify),
                    "pixalmlx": SimpleNamespace(mesh_cleanup=steps),
                }
                with patch.dict(sys.modules, modules):
                    output = POSTPROCESS(
                        "raw vertices", "raw faces", target_faces=100_000,
                        no_cleanup=False, keep_largest=keep_largest, hole_fill_backend="fan",
                    )
                self.assertEqual(output, ("repaired vertices", "oriented faces"))
                self.assertEqual(steps.mock_calls, [
                    call.remove_duplicate_faces("raw vertices", "raw faces", verbose=False),
                    getattr(call, filter_name)("dedup vertices", "dedup faces"),
                    call.simplify("filtered vertices", "filtered faces",
                                  target_faces=100_000, no_cleanup=True),
                    call.cleanup_mesh("simplified vertices", "simplified faces",
                                      min_component_area=0, do_fix_normals=False, hole_fill_backend="fan"),
                    call.orient_faces_by_adjacency("repaired vertices", "repaired faces",
                                                 verbose=False),
                ])

    def test_no_cleanup_delegates_to_the_upstream_simplifier_only(self):
        simplify = Mock(return_value=("simplified vertices", "simplified faces"))
        modules = {
            "generate": SimpleNamespace(_cleanup_and_simplify_mesh=simplify),
            "pixalmlx": None,
        }
        with patch.dict(sys.modules, modules):
            result = POSTPROCESS("raw vertices", "raw faces", target_faces=1000, no_cleanup=True)
        simplify.assert_called_once_with("raw vertices", "raw faces", target_faces=1000, no_cleanup=True)
        self.assertEqual(result, simplify.return_value)


if __name__ == "__main__":
    unittest.main()
