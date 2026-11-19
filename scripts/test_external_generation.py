import runpy
import sys
import unittest
from contextlib import nullcontext
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch


ADAPTER = runpy.run_path(str(Path(__file__).resolve().parents[1] / "crates/core/src/external_generation.py"))
BACKBONE = "facebook/dinov3-vitl16-pretrain-lvd1689m"


class ExternalGenerationTests(unittest.TestCase):
    def test_trellis_cuda_loads_only_local_model_and_dinov3_without_bria(self):
        weights = Path("/weights")
        dino = Mock()
        bria = Mock()
        rembg = SimpleNamespace(BiRefNet=bria)
        features = SimpleNamespace(DinoV3FeatureExtractor=dino)
        pipeline = Mock()

        def load(model):
            self.assertEqual(model, "/weights/model")
            self.assertIsNone(rembg.BiRefNet(model_name="briaai/RMBG-2.0"))
            features.DinoV3FeatureExtractor(model_name=BACKBONE)
            return pipeline

        with patch.dict(sys.modules, {
            "trellis2.pipelines": SimpleNamespace(
                rembg=rembg,
                Trellis2ImageTo3DPipeline=SimpleNamespace(from_pretrained=load),
            ),
            "trellis2.modules": SimpleNamespace(image_feature_extractor=features),
        }):
            self.assertIs(ADAPTER["trellis_model"](weights, False), pipeline)
        bria.assert_not_called()
        dino.assert_called_once_with(model_name="/weights/dinov3")
        pipeline.cuda.assert_called_once_with()

    def test_trellis_mlx_resolves_only_expected_dinov3_files_and_does_not_load_bria(self):
        bria = Mock()
        download = Mock()
        rembg = SimpleNamespace(BiRefNet=bria)
        hub = SimpleNamespace(hf_hub_download=download)
        pipeline = Mock()

        def load(model):
            self.assertEqual(model, "/weights/model")
            self.assertIsNone(rembg.BiRefNet(model_name="briaai/RMBG-2.0"))
            for name in ("config.json", "model.safetensors"):
                self.assertEqual(hub.hf_hub_download(BACKBONE, name), f"/weights/dinov3/{name}")
            with self.assertRaisesRegex(ValueError, "Unexpected TRELLIS"):
                hub.hf_hub_download(BACKBONE, "model.py")
            return pipeline

        with patch.dict(sys.modules, {
            "trellis2.pipelines": SimpleNamespace(rembg=rembg),
            "huggingface_hub": hub,
            "mlx_backend.pipeline": SimpleNamespace(create_mlx_pipeline=load),
        }):
            self.assertIs(ADAPTER["trellis_model"](Path("/weights"), True), pipeline)
        bria.assert_not_called()
        download.assert_not_called()
        self.assertIs(hub.hf_hub_download, download)

    def test_triposg_mlx_forwards_geometry_options_to_the_local_port(self):
        adapter = ADAPTER["generate_triposg_mlx"]
        prepared = Mock()
        pipeline = SimpleNamespace(load=Mock(), generate=Mock())
        args = SimpleNamespace(
            source=Path("/source"), weights=Path("/weights"), image=Path("/image.png"),
            output=Path("/job/result.glb"), seed=145, resolution=512, target_faces=8000,
        )
        with patch.dict(sys.modules, {"triposg": SimpleNamespace(pipeline=pipeline)}), \
                patch.object(sys, "path", []), \
                patch.dict(adapter.__globals__, {"foreground": Mock(return_value=prepared), "stage": Mock()}):
            adapter(args)
            self.assertEqual(sys.path, ["/source/src/mlx_agent/resources/ports/mlx-3d"])
        prepared.save.assert_called_once_with(Path("/job/conditioning.png"))
        pipeline.load.assert_called_once_with(Path("/weights/model"))
        pipeline.generate.assert_called_once_with(
            pipeline.load.return_value, Path("/job/conditioning.png"), args.output,
            seed=145, resolution=512, max_faces=8000,
        )

    def test_triposg_cuda_loads_local_safetensors_and_exports_geometry(self):
        adapter = ADAPTER["generate_triposg_cuda"]
        mesh = Mock(faces=[None] * 5)
        pipeline = Mock(return_value=SimpleNamespace(meshes=[mesh]))
        load = Mock()
        load.return_value.to.return_value = pipeline
        gpu = Mock(return_value=True)
        generator = Mock()
        args = SimpleNamespace(
            weights=Path("/weights"), image=Path("/image.png"),
            output=Path("/job/result.glb"), seed=145, resolution=512, target_faces=4,
        )
        prepare = Mock()
        with patch.dict(sys.modules, {
            "torch": SimpleNamespace(
                cuda=SimpleNamespace(is_available=gpu), float16="float16",
                inference_mode=nullcontext, Generator=generator,
            ),
            "triposg.pipelines.pipeline_triposg": SimpleNamespace(
                TripoSGPipeline=SimpleNamespace(from_pretrained=load),
            ),
        }), patch.dict(adapter.__globals__, {"foreground": prepare, "stage": Mock()}):
            adapter(args)
        load.assert_called_once_with("/weights/model", local_files_only=True, use_safetensors=True)
        load.return_value.to.assert_called_once_with("cuda", "float16")
        generator.assert_called_once_with(device=pipeline.device)
        generator.return_value.manual_seed.assert_called_once_with(145)
        pipeline.assert_called_once_with(
            image=prepare.return_value, generator=generator.return_value.manual_seed.return_value,
            flash_octree_depth=9,
        )
        mesh.simplify_quadric_decimation.assert_called_once_with(face_count=4)
        mesh.simplify_quadric_decimation.return_value.export.assert_called_once_with("/job/result.glb")


if __name__ == "__main__":
    unittest.main()
