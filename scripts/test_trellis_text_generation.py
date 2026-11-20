import io
import json
import os
import runpy
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch


ADAPTER = runpy.run_path(str(Path(__file__).resolve().parents[1] / "crates/core/src/trellis_text_generation.py"))


class TrellisTextGenerationTests(unittest.TestCase):
    def test_local_text_pipeline_exports_only_geometry(self):
        sampler = {
            "name": "FlowEulerGuidanceIntervalSampler",
            "args": {"sigma_min": 0.00001},
            "params": {"steps": 25},
        }
        config = {
            "name": "TrellisTextTo3DPipeline",
            "args": {
                "sparse_structure_sampler": sampler,
                "slat_sampler": sampler,
                "slat_normalization": {"std": [1], "mean": [0]},
            },
        }
        mesh = Mock(faces=[None] * 6)
        vertices, faces = Mock(), Mock()
        generate = Mock(return_value={
            "mesh": [SimpleNamespace(success=True, vertices=vertices, faces=faces)],
        })
        to_cuda = Mock()
        checkpoints = Mock(side_effect=lambda path: Path(path).name)
        clip = Mock()
        tokenizer = Mock()

        class UpstreamPipeline:
            def __init__(self, *, models, sparse_structure_sampler, slat_sampler,
                         slat_normalization, text_cond_model):
                self.models = models
                self._init_text_cond_model(text_cond_model)

            encode_text = Mock()
            cuda = to_cuda
            run = generate

        modules = {
            "torch": SimpleNamespace(cuda=SimpleNamespace(is_available=lambda: True)),
            "trimesh": SimpleNamespace(
                Trimesh=Mock(return_value=mesh), transformations=SimpleNamespace(rotation_matrix=Mock()),
            ),
            "trellis": SimpleNamespace(models=SimpleNamespace(from_pretrained=checkpoints)),
            "trellis.pipelines": SimpleNamespace(
                TrellisTextTo3DPipeline=UpstreamPipeline,
                samplers=SimpleNamespace(FlowEulerGuidanceIntervalSampler=Mock()),
            ),
            "transformers": SimpleNamespace(
                CLIPTextModel=SimpleNamespace(from_pretrained=clip),
                AutoTokenizer=SimpleNamespace(from_pretrained=tokenizer),
            ),
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            weights = root / "weights"
            stems = [weights / relative for relative in ADAPTER["MODEL_PATHS"].values()]
            for stem in stems:
                stem.parent.mkdir(parents=True, exist_ok=True)
                stem.with_suffix(".json").write_text("{}", encoding="utf-8")
                stem.with_suffix(".safetensors").touch()
            (weights / "model/pipeline.json").write_text(json.dumps(config), encoding="utf-8")
            output = root / "mesh.glb"
            argv = ["worker", "trellis-text-cuda", str(root / "source"), str(weights),
                    "-", str(output), "145", "4"]
            prompt = "도자기 컵\nwith a blue handle"
            with patch.dict(sys.modules, modules), patch.dict(os.environ), \
                    patch.object(sys, "path", []), patch.object(sys, "argv", argv), \
                    patch.object(sys, "stdin", SimpleNamespace(buffer=io.BytesIO(prompt.encode()))), \
                    redirect_stdout(io.StringIO()) as stdout:
                ADAPTER["main"]()
                self.assertEqual(os.environ["SPARSE_BACKEND"], "spconv")
                self.assertNotIn(prompt, stdout.getvalue())

            self.assertEqual([call.args[0] for call in checkpoints.call_args_list], list(map(str, stems)))
            clip.assert_called_once_with(str(weights / "clip"), local_files_only=True, use_safetensors=True)
            tokenizer.assert_called_once_with(str(weights / "clip"), local_files_only=True)
            to_cuda.assert_called_once_with()
            generate.assert_called_once_with(prompt, seed=145, formats=["mesh"])
            mesh.simplify_quadric_decimation.assert_called_once_with(face_count=4)
            mesh.simplify_quadric_decimation.return_value.export.assert_called_once_with(str(output))
            self.assertNotIn(prompt, argv)

            stems[-1].with_suffix(".safetensors").unlink()
            checkpoints.reset_mock()
            with patch.dict(sys.modules, modules), self.assertRaisesRegex(FileNotFoundError, "slat_dec_mesh"):
                ADAPTER["load_pipeline"](weights)
            self.assertNotIn(str(stems[-1]), [call.args[0] for call in checkpoints.call_args_list])

    def test_prompt_validation_counts_utf8_bytes(self):
        for raw in (b"  ", b"cup\0", ("가" * 1366).encode()):
            with self.subTest(length=len(raw)), \
                    patch.object(sys, "stdin", SimpleNamespace(buffer=io.BytesIO(raw))), \
                    self.assertRaises(ValueError):
                ADAPTER["read_prompt"]()
        with patch.object(sys, "stdin", SimpleNamespace(buffer=io.BytesIO(b"x" * 4096))):
            self.assertEqual(len(ADAPTER["read_prompt"]()), 4096)


if __name__ == "__main__":
    unittest.main()
