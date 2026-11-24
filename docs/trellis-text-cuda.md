# TRELLIS Text Large on CUDA

`trellis-text-cuda` is an experimental text-to-3D backend for Linux x86_64 with an NVIDIA GPU. Install it under **Settings → Models → TRELLIS Text Large → NVIDIA CUDA**, or follow the manual recipe below. It runs the original Microsoft TRELLIS text pipeline, not TRELLIS.2. The adapter exports a geometry-only GLB with a configurable seed and soft face target. It does not bake textures or expose a resolution control. The CLIP encoder truncates input to its 77-token context.

## Upstream and terms

- Microsoft publishes the [TRELLIS v1 source](https://github.com/microsoft/TRELLIS/tree/193e0502b5cb3eb5997ec457d5340e95d09b158e#license) and [TRELLIS-text-large weights](https://huggingface.co/microsoft/TRELLIS-text-large) under MIT, subject to the source repository's separately licensed components. The geometry adapter uses two shared decoders from [TRELLIS-image-large](https://huggingface.co/microsoft/TRELLIS-image-large); those do not require an input image or DINOv3.
- Mesh extraction uses TRELLIS's pinned [FlexiCubes fork](https://github.com/MaxtirError/FlexiCubes/blob/815e075a2a400d06c48d94c347674344ed6ae5c5/LICENSE.txt) and [NVIDIA Kaolin](https://github.com/NVIDIAGameWorks/kaolin/blob/master/LICENSE), both Apache-2.0. This path does not load the Gaussian decoder or the `nvdiffrast` texture baker.
- The local text encoder uses [OpenAI CLIP ViT-L/14](https://huggingface.co/openai/clip-vit-large-patch14); [OpenAI's CLIP code](https://github.com/openai/CLIP/blob/main/LICENSE) is MIT. Review the checkpoint's terms and the separate terms for CUDA and Python dependencies before use or redistribution. MeshHarbor includes only its adapter, not any of these components or weights.
- On Apple Silicon, use [MeshHarbor's native MLX port](trellis-text-mlx.md) with the same checkpoints. TRELLIS.2 MLX is a different, image-conditioned model.

## Optional manual setup

Set `DATA_DIR` to the absolute data directory you will use for MeshHarbor. These instructions use Python 3.10, an NVIDIA driver compatible with the CUDA 12.1 wheels, Git, and the Hugging Face CLI (`hf`). This is a setup recipe based on the pinned source, not a complete dependency lock or a tested GPU compatibility matrix. Obtain and run only trusted packages and checkpoints you are authorized to use.

```sh
RUNTIME="$DATA_DIR/runtimes/trellis-text-cuda"
REVISION=193e0502b5cb3eb5997ec457d5340e95d09b158e
mkdir -p "$RUNTIME/weights"
git clone --no-checkout https://github.com/microsoft/TRELLIS.git "$RUNTIME/source"
git -C "$RUNTIME/source" checkout --detach "$REVISION"
git -C "$RUNTIME/source" submodule update --init --recursive
python3.10 -m venv "$RUNTIME/.venv"
PYTHON="$RUNTIME/.venv/bin/python"

"$PYTHON" -m pip install torch==2.4.0 torchvision==0.19.0 xformers==0.0.27.post2 \
  --index-url https://download.pytorch.org/whl/cu121
"$PYTHON" -m pip install 'numpy<2' scipy pillow 'opencv-python-headless==4.11.0.86' \
  easydict tqdm plyfile 'open3d==0.19.0' 'rembg[cpu]==2.0.65' \
  'trimesh==4.9.0' 'fast-simplification==0.1.13' 'transformers==4.48.3' \
  safetensors 'spconv-cu120==2.3.6' \
  'git+https://github.com/EasternJournalist/utils3d.git@9a4eb15e4021b67b12c460c7057d642626897ec8'
"$PYTHON" -m pip install kaolin==0.16.0 \
  --find-links https://nvidia-kaolin.s3.us-east-2.amazonaws.com/torch-2.4.0_cu121.html
"$PYTHON" -m pip check
```

## Download the weights

Download the pinned text model, shared decoders, and CLIP encoder before running inference. This layout works for both CUDA and MLX.

```sh
hf download microsoft/TRELLIS-text-large --revision 4aad9f4a110329a410974d7f41ce5333a9a1fc87 \
  pipeline.json \
  ckpts/ss_flow_txt_dit_L_16l8_fp16.json ckpts/ss_flow_txt_dit_L_16l8_fp16.safetensors \
  ckpts/slat_flow_txt_dit_L_64l8p2_fp16.json ckpts/slat_flow_txt_dit_L_64l8p2_fp16.safetensors \
  --local-dir "$RUNTIME/weights/model"
hf download microsoft/TRELLIS-image-large --revision 25e0d31ffbebe4b5a97464dd851910efc3002d96 \
  ckpts/ss_dec_conv3d_16l8_fp16.json ckpts/ss_dec_conv3d_16l8_fp16.safetensors \
  ckpts/slat_dec_mesh_swin8_B_64l8m256c_fp16.json ckpts/slat_dec_mesh_swin8_B_64l8m256c_fp16.safetensors \
  --local-dir "$RUNTIME/weights/decoders"
hf download openai/clip-vit-large-patch14 --revision 32bd64288804d66eefd0ccbe215aa642df71cc41 \
  config.json model.safetensors merges.txt special_tokens_map.json tokenizer.json tokenizer_config.json vocab.json \
  --local-dir "$RUNTIME/weights/clip"
```

## Activate and generate

First check whether Python can see the GPU and import the pinned runtime. Then, only if you have permission for your intended use, create the activation marker:

```sh
(cd "$RUNTIME/source" && ATTN_BACKEND=xformers SPARSE_BACKEND=spconv SPCONV_ALGO=native \
  "$PYTHON" -c 'import torch, kaolin, spconv.pytorch, xformers.ops, trellis; assert torch.cuda.is_available()') &&
test "$(git -C "$RUNTIME/source" rev-parse HEAD)" = "$REVISION" &&
  printf '{"source_revision":"%s","authorized_local_use":true}\n' "$REVISION" > "$RUNTIME/activate.json"

meshharbor --data-dir "$DATA_DIR" doctor
meshharbor --data-dir "$DATA_DIR" generate --backend-id trellis-text-cuda \
  --prompt "A ceramic cup" --output ./cup.glb
```

In Studio, refresh **Settings → Models**, then select **TRELLIS Text Large** in **New model**. For REST or MCP, use `model.generate` or `start_model_generation` with `input: { "kind": "text", "prompt": "A ceramic cup" }`. No image asset is required.

Readiness checks the host, marker, and file layout only; it does not verify file hashes or run CUDA. MeshHarbor launches the trusted runtime with Hugging Face offline settings, not an OS network sandbox. It does not store prompts in job records or pass them in worker arguments; the CLI's own `--prompt` can still appear in shell history or process arguments. Remove the marker to disable new jobs. Only stub workers have been tested in MeshHarbor; real CUDA inference, memory fit, and output quality still need verification.
