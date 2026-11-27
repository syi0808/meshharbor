# External generation runtimes

These experimental backends can be installed in **Settings → Models** after shared tool setup in **Environment**, Hugging Face sign-in in **Connections**, and review of the model and dependency terms. The engine checks out pinned source and installs packages and model snapshots in a private runtime. TRELLIS.2 also requires DINOv3 access. You need Git, disk space for the downloads, and any system build prerequisites: Apple command-line build tools for native MLX extensions, or an NVIDIA driver and a matching CUDA toolkit/compiler for native CUDA extensions. MeshHarbor does not install system prerequisites. Use **Recent logs** on the backend card to inspect setup failures.

Existing manually provisioned backends still work and are not changed by the installer. Managed installations use `<data-dir>/runtimes/<backend>-managed`; **Delete model files** removes only the managed weights and model-transfer cache.

This page covers image-conditioned TRELLIS.2 and TripoSG. For the original TRELLIS text checkpoint, use the separate [Apple MLX](trellis-text-mlx.md) or [NVIDIA CUDA](trellis-text-cuda.md) guide.

## Choose a backend

| Backend | Host | Output | Compatible source revision |
| --- | --- | --- | --- |
| `trellis2-mlx` | Apple Silicon macOS | PBR GLB | [gtrg55/trellis2-mlx @ 60a3b50](https://github.com/gtrg55/trellis2-mlx/tree/60a3b508177250f92edc8453fd451ab804abc028) |
| `trellis2-cuda` | Linux x86-64 with NVIDIA | PBR GLB | [microsoft/TRELLIS.2 @ 75fbf01](https://github.com/microsoft/TRELLIS.2/tree/75fbf0183001ed9876c8dbb35de6b68552ee08bd) |
| `triposg-mlx` | Apple Silicon macOS | Geometry GLB | [cavi-ai/mlx-agent @ c4c04b4](https://github.com/cavi-ai/mlx-agent/tree/c4c04b4cbbdf563fa3ae931f9a584bb7f0b240d5/src/mlx_agent/resources/ports/mlx-3d/triposg) |
| `triposg-cuda` | Linux x86-64 with NVIDIA | Geometry GLB | [VAST-AI-Research/TripoSG @ fc5c409](https://github.com/VAST-AI-Research/TripoSG/tree/fc5c40990181e2a756c4e0b1c2f4d6b5202faf8c) |

- **TripoSG CUDA:** the official inference path uses FlashVDM. The [bundled upstream component terms](https://github.com/VAST-AI-Research/TripoSG/blob/fc5c40990181e2a756c4e0b1c2f4d6b5202faf8c/triposg/LICENSE) do not authorize covered use in South Korea, the EU, or the UK, even non-commercially. Do not provision or use that path there without separate permission.
- **TripoSG MLX:** uses a separate mesh extractor but [retains a HunyuanDiT notice](https://github.com/cavi-ai/mlx-agent/blob/c4c04b4cbbdf563fa3ae931f9a584bb7f0b240d5/src/mlx_agent/resources/ports/mlx-3d/triposg/NOTICE). Review the port and the original model/checkpoint terms independently; conversion does not change the rights in the weights.
- **TRELLIS.2:** both paths require [gated DINOv3](https://huggingface.co/facebook/dinov3-vitl16-pretrain-lvd1689m). CUDA PBR export uses [nvdiffrast v0.4.0](https://github.com/NVlabs/nvdiffrast/blob/v0.4.0/LICENSE.txt), whose standard terms restrict use to non-commercial research/evaluation. The MLX port runs model inference in MLX but still needs PyTorch MPS and native Metal dependencies for postprocessing. Neither adapter loads the default BRIA background-removal model.

## Input support

These integrations take images:

- [Pixal3D](https://github.com/TencentARC/Pixal3D) takes images, including posed views on the managed CUDA profile.
- [TRELLIS.2-4B](https://huggingface.co/microsoft/TRELLIS.2-4B) takes images. The separate [TRELLIS Text Large](trellis-text-cuda.md) backend is from the original TRELLIS family, not a mode of TRELLIS.2.
- [TripoSG](https://huggingface.co/VAST-AI/TripoSG) takes images. The separate [TripoSG-scribble](https://huggingface.co/VAST-AI/TripoSG-scribble) checkpoint takes both a scribble image and a text prompt; it is not the integrated checkpoint or a text-only workflow.

## Optional manual provisioning

Use the same data directory for setup and for CLI, REST, or MCP. These commands are manual setup recipes, not a tested lockfile or hardware compatibility promise. Do not run the upstream download-on-inference demos. Use only trusted local Python packages, checkouts, and checkpoints you are authorized to run.

Set `DATA_DIR` to your absolute MeshHarbor data directory, and `BACKEND` to one ID above. Set `REPOSITORY` to the corresponding GitHub clone URL and `REVISION` to the full SHA in the source link.

```sh
RUNTIME="$DATA_DIR/runtimes/$BACKEND"
mkdir -p "$RUNTIME/weights"
git clone --no-checkout "$REPOSITORY" "$RUNTIME/source"
git -C "$RUNTIME/source" checkout --detach "$REVISION"
```

For TRELLIS.2, run `git -C "$RUNTIME/source" submodule update --init --recursive` before building O-Voxel.

Create the dedicated environment with `uv venv --python 3.10 --seed "$RUNTIME/.venv"`, using 3.12 instead for TripoSG MLX. Install dependencies into that environment, not the engine's or the system Python:

- **TRELLIS.2 CUDA:** use the official [installation recipe](https://github.com/microsoft/TRELLIS.2/tree/75fbf0183001ed9876c8dbb35de6b68552ee08bd#-installation). For an existing Python 3.10 environment, install the documented CUDA 12.4 PyTorch/torchvision builds, then run the upstream installation from `source` in that environment (without `--new-env`). The adapter uses SDPA; the required native parts are CuMesh, FlexGEMM, O-Voxel and nvdiffrast. The demo renderer is not used.
- **TRELLIS.2 MLX:** follow the pinned port's [installation recipe](https://github.com/gtrg55/trellis2-mlx/tree/60a3b508177250f92edc8453fd451ab804abc028#installation) in the dedicated environment, including PyTorch/torchvision, the Metal extensions, and the local O-Voxel build. Do not use `app_mlx.py` or the upstream weight download script for this integration.
- **TripoSG CUDA:** use the [official dependency setup](https://github.com/VAST-AI-Research/TripoSG/tree/fc5c40990181e2a756c4e0b1c2f4d6b5202faf8c#-installation) in a Python 3.10 environment with a compatible CUDA PyTorch build and native `diso`. Also install `fast-simplification` for trimesh face reduction. MeshHarbor calls the pipeline directly, not the demo that installs BRIA RMBG weights.
- **TripoSG MLX:** use Python 3.12 and install `mlx==0.31.1`, `numpy`, `pillow`, `scikit-image` and `fast-simplification`. You do not need to install the entire agent application.

With the Hugging Face CLI installed separately, obtain each permitted snapshot explicitly. For TRELLIS.2, the expected layout is `weights/model` for the main model and its nested structure decoder, plus `weights/dinov3` for DINOv3:

```sh
hf download microsoft/TRELLIS.2-4B --revision af44b45f2e35a493886929c6d786e563ec68364d \
  --include 'pipeline.json' 'ckpts/*' --local-dir "$RUNTIME/weights/model"
hf download microsoft/TRELLIS-image-large --revision 25e0d31ffbebe4b5a97464dd851910efc3002d96 \
  'ckpts/ss_dec_conv3d_16l8_fp16.json' 'ckpts/ss_dec_conv3d_16l8_fp16.safetensors' \
  --local-dir "$RUNTIME/weights/model/microsoft/TRELLIS-image-large"
hf download facebook/dinov3-vitl16-pretrain-lvd1689m --revision ea8dc2863c51be0a264bab82070e3e8836b02d51 \
  config.json model.safetensors --local-dir "$RUNTIME/weights/dinov3"
```

For TripoSG, set `SNAPSHOT="$RUNTIME/weights/model"` on CUDA or `SNAPSHOT="$RUNTIME/weights/original"` on MLX. Download the authorized [image-conditioned snapshot](https://huggingface.co/VAST-AI/TripoSG):

```sh
hf download VAST-AI/TripoSG --revision 2c1c516d22d58db486a058d98d31bb6177344e06 --local-dir "$SNAPSHOT"
```

For MLX only, convert the downloaded snapshot into the required `model` folder:

```sh
"$RUNTIME/.venv/bin/python" "$RUNTIME/source/src/mlx_agent/resources/ports/mlx-3d/triposg/convert.py" \
  --hf-path "$SNAPSHOT" --mlx-path "$RUNTIME/weights/model"
```

## Activate and Generate

Only after the relevant permissions and files are in place, create a regular `$RUNTIME/activate.json` containing the full source revision and `"authorized_local_use": true`. For example, after confirming your rights:

```sh
test "$(git -C "$RUNTIME/source" rev-parse HEAD)" = "$REVISION" &&
  printf '{"source_revision":"%s","authorized_local_use":true}\n' "$REVISION" > "$RUNTIME/activate.json"
meshharbor --data-dir "$DATA_DIR" doctor
meshharbor --data-dir "$DATA_DIR" generate "$IMAGE_ASSET_ID" --backend-id "$BACKEND" --output ./generated.glb
```

The marker is the operator's acknowledgment, not proof of rights or source integrity. Readiness checks only the marker, host family and required nonempty files. MeshHarbor does not inspect Git state, verify weights, or test GPU imports at readiness time. Removing the marker disables new jobs for that backend.

Refresh model status in Studio after setup. Use the same explicit ID with REST's `POST /api/v1/generation` or MCP's `generate_model` and an image input. For images, `auto` continues to select Pixal3D. Each profile on this page accepts one source image at resolution 512 (TRELLIS.2 pipeline or TripoSG extraction grid). Prefer a centered subject on a clean background, or a PNG with a foreground alpha mask. TripoSG does not expose texture size.

Inference launches the trusted local runtime with an isolated home and Hugging Face offline settings; it does not forward login tokens or run a package installer. This is not an OS network sandbox. Leave runtime files unchanged while jobs are running. Tests use stub workers; real Metal/CUDA builds, model loading, memory fit and generated-mesh quality are not yet verified by MeshHarbor.
