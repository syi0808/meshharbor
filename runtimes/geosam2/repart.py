"""Offline, user-guided GeoSAM2 repart for prepared 12-view assets."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import sys
import tempfile

import numpy as np

from repart_glb import Source


WEIGHTS_SHA256 = "2e391c0d9455fe92b69d61cdc94754d9b8081b9b541d09e1b6a3a55ebf6c6de0"
WEIGHTS_BYTES = 615_627_435
VIEW_COUNT = 12
IMAGE_SIZE = 1024
UPSTREAM_BACKGROUND = 999


def configure_upstream():
    import cv2
    import OpenEXR
    from torch.utils import cpp_extension

    imread = cv2.imread
    load_extension = cpp_extension.load

    def read_image(path, *args, **kwargs):
        if str(path).endswith(".exr"):
            with OpenEXR.File(str(path), header_only=True) as header:
                if len(header.parts) != 1:
                    raise ValueError("Expected one part in an EXR depth image.")
                origin, end = header.header()["dataWindow"]
                if not np.array_equal(origin, (0, 0)) or not np.array_equal(end, (IMAGE_SIZE - 1,) * 2):
                    raise ValueError("Expected a 1024px EXR depth image.")
            with OpenEXR.File(str(path), separate_channels=True) as source:
                channels = source.channels()
                channel = channels["B"] if "B" in channels else next(iter(channels.values()))
                return np.repeat(channel.pixels[..., None], 3, axis=2)
        return imread(path, *args, **kwargs)

    def load_serial_voting(name, *args, **kwargs):
        if name == "mode_ext":
            kwargs["extra_cflags"] = [flag for flag in kwargs.get("extra_cflags", []) if flag != "-fopenmp"]
        return load_extension(name, *args, **kwargs)

    cv2.imread = read_image
    cpp_extension.load = load_serial_voting


def check_views(root):
    from PIL import Image

    for prefix in ("color", "normal", "depth"):
        for index in range(VIEW_COUNT):
            extension = "exr" if prefix == "depth" else "webp"
            file = root / f"{prefix}_{index:04d}.{extension}"
            if not file.is_file() or not 0 < file.stat().st_size <= 16 * 1024 * 1024:
                raise ValueError(f"Missing or oversized prepared view: {file.name}")
            if extension == "webp":
                with Image.open(file) as picture:
                    if picture.size != (IMAGE_SIZE, IMAGE_SIZE) or picture.mode != "RGBA":
                        raise ValueError(f"Expected a 1024px RGBA image: {file.name}")
    meta = root / "meta.json"
    if not meta.is_file() or meta.stat().st_size > 32 * 1024:
        raise ValueError("Missing or oversized camera metadata.")
    metadata = json.loads(meta.read_text())
    transforms = np.asarray(metadata["transforms"], dtype=np.float64)
    translation = np.asarray(metadata["translation"], dtype=np.float64)
    if (transforms.shape != (VIEW_COUNT, 4, 4) or translation.shape != (3,)
            or not np.isfinite(transforms).all() or not np.isfinite(translation).all()
            or not np.isfinite(metadata["scaling_factor"]) or not metadata["scaling_factor"] > 0
            or not 0 < metadata["camera_angle_x"] < np.pi):
        raise ValueError("Camera metadata does not describe 12 valid normalized views.")


def worker(args, source):
    import torch
    import trimesh

    if args.backend == "cuda" and not torch.cuda.is_available():
        raise RuntimeError("CUDA was selected but is not available.")
    if args.backend == "mlx" and (platform.system(), platform.machine()) != ("Darwin", "arm64"):
        raise RuntimeError("The MLX backend requires Apple Silicon macOS.")

    check_views(args.views)
    with args.checkpoint.open("rb") as checkpoint_file:
        if os.fstat(checkpoint_file.fileno()).st_size != WEIGHTS_BYTES:
            raise ValueError("The GeoSAM2 checkpoint size does not match the pinned FP32 release.")
        digest = hashlib.file_digest(checkpoint_file, "sha256").hexdigest()
        if digest != WEIGHTS_SHA256:
            raise ValueError("The GeoSAM2 checkpoint digest does not match the pinned FP32 release.")
        checkpoint_file.seek(0)
        checkpoint = torch.load(checkpoint_file, map_location="cpu", weights_only=True)["model"]

    sys.path.insert(0, str(args.geo_source))
    if args.backend == "mlx":
        sys.path.insert(0, str(args.sam2_mlx_source))

    configure_upstream()
    import inference
    from sam2.build_sam import build_sam2_video_predictor_geosam2

    def load_ordered_mesh(path):
        if Path(path).resolve() != args.views / "mesh.glb":
            raise ValueError("Unexpected source in the GeoSAM2 renderer output.")
        triangles = source.positions[source.faces].reshape(-1, 3)
        sequential_faces = np.arange(len(triangles)).reshape(-1, 3)
        render_mesh = trimesh.Trimesh(vertices=triangles, faces=sequential_faces, process=False)
        original = trimesh.Trimesh(vertices=source.positions, faces=source.faces, process=False)
        return render_mesh, original

    inference.load_mesh_with_faces = load_ordered_mesh
    points = np.asarray(args.point + args.exclude, dtype=np.float32)
    point_labels = np.asarray([1] * len(args.point) + [0] * len(args.exclude), dtype=np.int32)
    np.random.seed(3)
    torch.manual_seed(3)
    device = "cuda" if args.backend == "cuda" else "cpu"

    with torch.inference_mode():
        predictor = build_sam2_video_predictor_geosam2(
            "configs/geosam2.yaml", device="cpu", apply_postprocessing=False,
        )
        predictor.load_state_dict(checkpoint, strict=True)
        if args.backend == "mlx":
            from mlx_backend import use_mlx

            use_mlx(predictor, checkpoint)
        else:
            predictor.to(device)
        del checkpoint
        state = predictor.init_state(video_path=str(args.views), video_id_list=[args.view])
        predictor.add_new_points_or_box(state, frame_idx=0, obj_id=1, points=points, labels=point_labels)
        mask = None
        for frame, _, logits in predictor.propagate_in_video_v2(state, start_frame_idx=0):
            if frame == 0:
                mask = (logits[0, 0].cpu().numpy() > 0).astype(np.int32)
        del state
        if mask is None or np.count_nonzero(mask) < 64:
            raise ValueError("No usable seed mask; try a different or more specific click.")
        with tempfile.TemporaryDirectory(prefix="meshharbor-repart-") as work:
            seed = Path(work) / "seed.npy"
            np.save(seed, mask, allow_pickle=False)
            prepared = inference.read_data(str(args.views), mask_path=str(seed), mask_view=args.view)
            prediction = inference.segment_with_mask_prompts(
                predictor=predictor, mask_generator=None, data=prepared,
                opposite_auto_segmentation=False, enable_postprocess=False, output_dir=work,
                start_frames=[args.view], start_to_seed_views={args.view: [args.view]},
            )
    labels = prediction["face_label"]
    if labels is None:
        raise RuntimeError("GeoSAM2 did not output face labels.")
    labels = labels.cpu().numpy()
    if not np.isin(labels, (0, 1, UPSTREAM_BACKGROUND)).all():
        raise ValueError("Unexpected labels in the single-part result.")
    return (labels == 1).astype(np.uint8), int(np.count_nonzero(labels == 0))


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=("cuda", "mlx"), required=True)
    parser.add_argument("--geo-source", type=lambda value: Path(value).resolve(), required=True)
    parser.add_argument("--sam2-mlx-source", type=lambda value: Path(value).resolve())
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--views", type=lambda value: Path(value).resolve(), required=True)
    parser.add_argument("--view", type=int, choices=range(VIEW_COUNT), default=0)
    parser.add_argument("--point", nargs=2, type=float, action="append", required=True, metavar=("X", "Y"))
    parser.add_argument("--exclude", nargs=2, type=float, action="append", default=[], metavar=("X", "Y"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.backend == "mlx" and args.sam2_mlx_source is None:
        parser.error("--sam2-mlx-source is required for MLX")
    points = args.point + args.exclude
    if len(points) > 16 or not all(0 <= coordinate < IMAGE_SIZE for point in points for coordinate in point):
        parser.error("Use 1 to 16 clicks within the 1024px image.")
    if args.output.suffix.lower() != ".glb" or args.output.exists():
        parser.error("--output must be a new .glb file")
    return args


def main():
    args = parse_args()
    try:
        source = Source(args.views / "mesh.glb")
        labels, not_observed = worker(args, source)
        binary, report = source.export(labels)
        with tempfile.NamedTemporaryFile(dir=args.output.parent, prefix=".repart-") as output:
            output.write(binary)
            output.flush()
            os.link(output.name, args.output)
    except (KeyError, IndexError, OSError, RuntimeError, ValueError) as error:
        print(f"GeoSAM2 repart failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps({"backend": args.backend, "unobserved_triangles": not_observed, **report}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
