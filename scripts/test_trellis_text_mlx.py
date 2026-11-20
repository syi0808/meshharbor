import importlib.util
import runpy
import tempfile
import unittest
from pathlib import Path


RUNTIME = Path(__file__).resolve().parents[1] / "crates/core/src/trellis_text_mlx"


@unittest.skipUnless(importlib.util.find_spec("numpy"), "NumPy is required for surface tests")
class TrellisTextMlxSurfaceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        import numpy as np

        cls.np = np
        cls.surface = runpy.run_path(str(RUNTIME / "surface.py"))

    def test_isolated_negative_corner_builds_a_closed_outward_mesh(self):
        np = self.np
        duals = np.full((256, 4, 7), -1, dtype=np.int32)
        checks = np.zeros((256, 5), dtype=np.int32)
        for corner in range(8):
            incident = np.flatnonzero((self.surface["EDGES"] == corner).any(axis=1))
            duals[1 << corner, 0, :3] = incident
        cells = np.array([[1, 1, 1]], dtype=np.int32)
        features = np.zeros((1, 53), dtype=np.float32)
        features[:, :8] = 1
        features[:, 0] = -0.5
        verts, faces = self.surface["extract_surface"](cells, features, 4, (duals, checks))
        self.assertEqual((verts.shape, faces.shape), ((8, 3), (12, 3)))
        triangles = verts[faces]
        signed_volume = np.sum(triangles[:, 0] * np.cross(triangles[:, 1], triangles[:, 2])) / 6
        self.assertGreater(signed_volume, 0)
        edges = np.sort(faces[:, [0, 1, 1, 2, 2, 0]].reshape(-1, 2), axis=1)
        _, counts = np.unique(edges, axis=0, return_counts=True)
        self.assertTrue((counts == 2).all())

        features[:, 8:11] = 0.5
        features[:, 32:44] = np.linspace(-1, 1, 12)
        changed, _ = self.surface["extract_surface"](cells, features, 4, (duals, checks))
        self.assertFalse(np.allclose(verts, changed))

    def test_table_loader_reads_literals_without_executing_upstream_code(self):
        np = self.np
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "tables.py"
            path.write_text(
                f"dmc_table = {np.full((256, 4, 7), -1).tolist()}\n"
                f"check_table = {np.zeros((256, 5), dtype=int).tolist()}\n"
                "raise RuntimeError('do not execute the table file')\n",
                encoding="utf-8",
            )
            dual, check = self.surface["load_tables"](path)
            self.assertEqual((dual.shape, check.shape), ((256, 4, 7), (256, 5)))
            path.write_text("dmc_table = []", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "Unexpected FlexiCubes table size"):
                self.surface["load_tables"](path)


if __name__ == "__main__":
    unittest.main()
