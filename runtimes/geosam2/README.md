# Experimental GeoSAM2 guided repart

Select one rigid part with clicks on a rendered view. The worker propagates the selection across 12 views and exports the selected triangles and the remainder as two independently transformable GLB scene nodes. Both reuse the original geometry, UV, material and image data. The operation does not seal cuts, reconstruct hidden surfaces, estimate joints or create animation.

This is a command-line prototype, not a Studio operation. Use a single untransformed static mesh with one indexed triangle primitive, embedded resources, at most 500,000 triangles and a GLB under 64 MiB. Supply a trusted, matching 12-view GeoSAM2 render directory containing the original `mesh.glb`; the worker preserves that file and cannot verify that an arbitrary set of images depicts it. An unseen or unselected triangle stays in the remainder.

## Backends

- **CUDA:** official [GeoSAM2](https://github.com/VAST-AI-Research/GeoSAM2/tree/b5de23c60ab487d407b623d394a1614f9714761c) PyTorch neural layers on Linux/NVIDIA. No fallback to CPU inference. This checkout's Apple Silicon host cannot run CUDA.
- **MLX:** GeoSAM2's normal and position-map encoders, LoRA adaptations, feature fusion, prompt and mask modules, spatial memory encoder/attention, and object-pointer projections run in MLX. PyTorch on CPU still loads the verified checkpoint and runs upstream temporal bookkeeping and 3D label projection. This is not PyTorch MPS. The native neural components build on [eisneim's SAM2.1 MLX port](https://github.com/eisneim/sam2.1_mlx/tree/036f2d02a9069871fd2bf47109ff3b522cd222c4), which declares Apache-2.0 in its README. Its stock SAM2.1 weights are not used.

GeoSAM2 publishes [Apache-2.0 code and model weights](https://huggingface.co/VAST-AI/GeoSAM2); retain its [third-party notices](https://github.com/VAST-AI-Research/GeoSAM2/blob/b5de23c60ab487d407b623d394a1614f9714761c/NOTICE) and review dependency provenance before distributing an installer. No upstream code or checkpoints are bundled in MeshHarbor.

## Install From Studio

First prepare **Settings → Environment** and sign in to Hugging Face under **Connections**. Then open **Settings → Models → Repart → GeoSAM2** and choose **Install model** for the supported accelerator. One job installs pinned sources into a private environment using the shared Python toolchain, checks native dependencies and GPU access, and downloads the weights. Already verified files are reused. Install a C++ compiler first (for example, Xcode Command Line Tools on macOS). The single 616 MB checkpoint is shared by both backends and checked against its pinned SHA-256. **Delete model files** removes that checkpoint without removing the backend.

In the MeshHarbor data directory (set by `--data-dir`), the managed installs use `runtimes/geosam2-managed`. For example, on Apple Silicon, the pre-rendered sample included in upstream source can be run with:

```sh
managed=/path/to/meshharbor-data/runtimes/geosam2-managed
backend="$managed/geosam2-mlx"
PATH="$backend/.venv/bin:$PATH" HOME="$backend/home" \
  "$backend/.venv/bin/python" "$backend/worker/repart.py" --backend mlx \
  --geo-source "$backend/source" --sam2-mlx-source "$backend/sam2-mlx" \
  --checkpoint "$managed/weights/geosam2.pt" --views "$backend/source/example/sample_00" \
  --view 0 --point 512 400 --output split.glb
```

For a Linux/NVIDIA install, use `geosam2-cuda` and `--backend cuda` and omit `--sam2-mlx-source`. Studio does not install Blender, render input meshes, or execute this command.

## Run From Manual Checkouts

1. In isolated checkouts, pin GeoSAM2 to `b5de23c60ab487d407b623d394a1614f9714761c` and, for MLX only, `sam2.1_mlx` to `036f2d02a9069871fd2bf47109ff3b522cd222c4`. Only pass source you trust: it is imported as Python code.
2. Use a private Python 3.12 virtual environment. Install PyTorch and torchvision appropriate to the backend, GeoSAM2's pinned checkout's `requirements.txt`, and `OpenEXR`. On macOS add `mlx==0.31.2`; the local run used Torch 2.15.0. An available C++ compiler and Ninja are needed for the upstream CPU label-voting helper, which is compiled without OpenMP.
3. Download [the FP32 checkpoint at the pinned revision](https://huggingface.co/VAST-AI/GeoSAM2/resolve/ba92f5f50418f2fe9af1078448b63176df13b1ee/geosam2.pt) separately. The tool verifies its length and SHA-256 before a weights-only load and never downloads model files.
4. Use upstream `example/sample_00` to reproduce the probe. For a different asset, first prepare color, normal and depth views with [upstream's headless Blender renderer](https://github.com/VAST-AI-Research/GeoSAM2/tree/b5de23c60ab487d407b623d394a1614f9714761c#usage). Fresh rendering with the current macOS Blender version is not part of the validated path.

```sh
python runtimes/geosam2/repart.py --backend mlx \
  --geo-source /path/to/GeoSAM2 --sam2-mlx-source /path/to/sam2.1_mlx \
  --checkpoint /path/to/geosam2.pt --views /path/to/GeoSAM2/example/sample_00 \
  --view 0 --point 512 400 --output split.glb
```

For CUDA pass `--backend cuda` and omit `--sam2-mlx-source`. Coordinates are pixels in the 1024×1024 view, measured from the upper-left corner. Repeat `--point X Y` or add `--exclude X Y` to refine the same part. Import the result with MeshHarbor or open it in Blender to inspect the selected node. The output must not exist; failures do not replace it.

## Validation

The managed MLX install was tested from an empty MeshHarbor data directory on Apple M3 Pro. One job installed the backend and verified the downloaded checkpoint. Running the installer again reused both. Managed CUDA has not been exercised on a Linux/NVIDIA host.

On Apple M3 Pro, the initial MLX port matched upstream PyTorch MPS with mask IoU 1.000000 in the clicked view and 0.999972 in the next view. This compares one prompt/track with postprocessing disabled, not general segmentation quality.

The 12-view run also exported the same sample with an embedded checker PNG added to its existing material. It produced a selected node (37,670 triangles) and a remainder (369,987 triangles); all 407,657 source triangles were retained exactly once, with the original UVs, material bindings, and PNG bytes unchanged. The 92,343 triangles without observations stayed in the remainder. MeshHarbor imported the result, and Blender 5.1 opened it with two independently movable nodes sharing the material.

`python -m unittest discover -s runtimes/geosam2` checks exact index assignment, independent scene nodes, preservation of the embedded GLB data, and invalid/no-split cases. For the full model and licensing comparison see [Semantic repart feasibility](../../docs/semantic-repart-feasibility.md).
