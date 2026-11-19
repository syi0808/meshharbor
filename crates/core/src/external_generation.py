import argparse
import os
import sys
from pathlib import Path
from unittest.mock import patch


def stage(name):
    print(f"=== {name} ===", flush=True)


def foreground(path, background, padding):
    from PIL import Image, ImageOps

    with Image.open(path) as source:
        image = ImageOps.exif_transpose(source).convert("RGBA")
    image.thumbnail((1024, 1024), Image.Resampling.LANCZOS)
    bounds = image.getchannel("A").getbbox()
    if bounds is None:
        raise ValueError("The reference image is fully transparent.")
    image = image.crop(bounds)
    width, height = image.size
    edge = max(width, height)
    side = edge + 2 * round(edge * padding)
    canvas = Image.new("RGBA", (side, side), background)
    canvas.alpha_composite(image, ((side - width) // 2, (side - height) // 2))
    return canvas.convert("RGB")


def local_dinov3(snapshot):
    def resolve(repo_id, filename, **_options):
        if repo_id != "facebook/dinov3-vitl16-pretrain-lvd1689m" or filename not in (
            "config.json",
            "model.safetensors",
        ):
            raise ValueError(f"Unexpected TRELLIS.2 model reference: {repo_id}/{filename}")
        return str(snapshot / filename)

    return resolve


def trellis_model(weights, mlx):
    from trellis2.pipelines import rembg

    model = str(weights / "model")
    dino = weights / "dinov3"
    with patch.object(rembg, "BiRefNet", return_value=None):
        if mlx:
            from mlx_backend.pipeline import create_mlx_pipeline

            with patch("huggingface_hub.hf_hub_download", side_effect=local_dinov3(dino)):
                return create_mlx_pipeline(model)

        from trellis2.modules import image_feature_extractor
        from trellis2.pipelines import Trellis2ImageTo3DPipeline

        extractor = image_feature_extractor.DinoV3FeatureExtractor
        with patch.object(
            image_feature_extractor,
            "DinoV3FeatureExtractor",
            side_effect=lambda **_options: extractor(model_name=str(dino)),
        ):
            pipeline = Trellis2ImageTo3DPipeline.from_pretrained(model)
        pipeline.cuda()
        return pipeline


def generate_trellis(args):
    import torch
    from o_voxel import postprocess

    mlx = args.backend == "trellis2-mlx"
    if not (torch.backends.mps.is_available() if mlx else torch.cuda.is_available()):
        raise RuntimeError("The configured PyTorch build cannot use the required GPU.")
    image = foreground(args.image, "black", 0)
    pipeline = trellis_model(args.weights, mlx)
    stage("generate_mesh")
    mesh = pipeline.run(
        image, seed=args.seed, pipeline_type=str(args.resolution), preprocess_image=False
    )[0]
    stage("pbr_export")
    if mlx:
        export = {"texture_baking_mode": "dense_mps", "alpha_mode": "OPAQUE", "mesh_repair_mode": "fast"}
    else:
        mesh.simplify(16777216)
        export = {"remesh": True, "remesh_band": 1, "remesh_project": 0}
    glb = postprocess.to_glb(
        vertices=mesh.vertices,
        faces=mesh.faces,
        attr_volume=mesh.attrs,
        coords=mesh.coords,
        attr_layout=mesh.layout,
        voxel_size=mesh.voxel_size,
        aabb=[[-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]],
        decimation_target=args.target_faces,
        texture_size=args.texture_size,
        **export,
    )
    glb.export(str(args.output))


def generate_triposg_cuda(args):
    import torch
    from triposg.pipelines.pipeline_triposg import TripoSGPipeline

    if not torch.cuda.is_available():
        raise RuntimeError("The configured PyTorch build cannot use CUDA.")
    image = foreground(args.image, "white", 0.1)
    pipeline = TripoSGPipeline.from_pretrained(
        str(args.weights / "model"), local_files_only=True, use_safetensors=True
    ).to("cuda", torch.float16)
    stage("generate_mesh")
    with torch.inference_mode():
        result = pipeline(
            image=image,
            generator=torch.Generator(device=pipeline.device).manual_seed(args.seed),
            flash_octree_depth=args.resolution.bit_length() - 1,
        )
    stage("mesh_export")
    mesh = result.meshes[0]
    if len(mesh.faces) > args.target_faces:
        mesh = mesh.simplify_quadric_decimation(face_count=args.target_faces)
    mesh.export(str(args.output))


def generate_triposg_mlx(args):
    sys.path.insert(0, str(args.source / "src/mlx_agent/resources/ports/mlx-3d"))
    from triposg import pipeline

    image = args.output.parent / "conditioning.png"
    foreground(args.image, "white", 0.1).save(image)
    model = pipeline.load(args.weights / "model")
    stage("generate_mesh")
    pipeline.generate(
        model, image, args.output, seed=args.seed,
        resolution=args.resolution, max_faces=args.target_faces,
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("backend", choices=("trellis2-cuda", "trellis2-mlx", "triposg-cuda", "triposg-mlx"))
    for parameter in ("source", "weights", "image", "output"):
        parser.add_argument(parameter, type=Path)
    for parameter in ("seed", "resolution", "texture_size", "target_faces"):
        parser.add_argument(parameter, type=int)
    args = parser.parse_args()
    stage("initialize_model")
    if args.backend == "triposg-mlx":
        generate_triposg_mlx(args)
        return
    sys.path.insert(0, str(args.source))
    if args.backend == "triposg-cuda":
        generate_triposg_cuda(args)
        return
    os.environ["ATTN_BACKEND"] = "sdpa"
    generate_trellis(args)


if __name__ == "__main__":
    main()
