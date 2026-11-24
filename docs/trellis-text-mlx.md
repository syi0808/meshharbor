# TRELLIS Text Large on Apple MLX

`trellis-text-mlx` is MeshHarbor's experimental Apple Silicon port of the original [Microsoft TRELLIS Text Large](https://huggingface.co/microsoft/TRELLIS-text-large). It consumes the same pinned safetensors as the [CUDA profile](trellis-text-cuda.md), with no conversion step. CLIP, both flow models, occupancy decoding, and the shifted-window mesh decoder run on MLX/Metal. NumPy assembles the learned FlexiCubes surface on CPU. No PyTorch, MPS, Core ML, CUDA, DINO, or image input is required.

The backend outputs a geometry-only GLB. It uses the original CLIP context (77 tokens), default 25 + 25 flow steps, weighted dual vertices, and learned triangulation. It has no texture baking or adjustable resolution. The CLI and job inputs support a seed and a soft face target. MLX and CUDA do not share an RNG, so the same seed does not imply identical models. The worker rejects samples over 32,768 occupied cells rather than silently truncating detail.

## Install in Studio

In **Settings → Models → TRELLIS Text Large → Apple MLX**, select **Install model** and review the terms in the confirmation dialog. After you confirm, MeshHarbor prepares private Python 3.12 and downloads the pinned source and weights. No Hugging Face sign-in is required for these public weights.

## Optional manual setup

Set `DATA_DIR` to your absolute MeshHarbor data directory. Use Python 3.12, Git, and the Hugging Face CLI (`hf`). Only install reviewed dependencies and checkpoints you are authorized to use.

```sh
RUNTIME="$DATA_DIR/runtimes/trellis-text-mlx"
REVISION=a26b42aee4e31999dbb4429226b66d896d49e1d8
FLEXICUBES_REVISION=815e075a2a400d06c48d94c347674344ed6ae5c5
mkdir -p "$RUNTIME"
git clone --filter=blob:none --sparse https://github.com/amirhossein-razlighi/mlx_diffuser "$RUNTIME/source"
git -C "$RUNTIME/source" sparse-checkout set src
git -C "$RUNTIME/source" checkout --detach "$REVISION"
git clone --no-checkout https://github.com/MaxtirError/FlexiCubes.git "$RUNTIME/source/flexicubes"
git -C "$RUNTIME/source/flexicubes" checkout --detach "$FLEXICUBES_REVISION"

python3.12 -m venv "$RUNTIME/.venv"
PYTHON="$RUNTIME/.venv/bin/python"
"$PYTHON" -m pip install 'mlx==0.31.2' 'numpy==2.5.5' 'pillow==13.0.0' \
  'tokenizers==0.21.4' 'trimesh==5.2.1' 'fast-simplification==0.1.13'
"$PYTHON" -I -c 'import mlx.core as mx; assert mx.metal.is_available()'
```

With `RUNTIME` still set to the MLX directory, follow the [three pinned weight downloads](trellis-text-cuda.md#download-the-weights) from the CUDA guide. If you already have all those files, you may instead link its `weights` directory into the MLX runtime. Do not run the CUDA environment setup.

Only after reviewing the sources and permissions, create the activation marker:

```sh
test "$(git -C "$RUNTIME/source" rev-parse HEAD)" = "$REVISION" &&
  test "$(git -C "$RUNTIME/source/flexicubes" rev-parse HEAD)" = "$FLEXICUBES_REVISION" &&
  printf '{"source_revision":"%s","authorized_local_use":true}\n' "$REVISION" > "$RUNTIME/activate.json"

meshharbor --data-dir "$DATA_DIR" doctor
meshharbor --data-dir "$DATA_DIR" generate --backend-id trellis-text-mlx \
  --prompt "A blue ceramic cup with a single round handle" --output ./cup.glb
```

In Studio, refresh **Settings → Models**, then select **TRELLIS Text Large** in **New model**. REST and MCP use the same backend ID with a text input. Readiness checks the marker and nonempty file paths, not their hashes or Metal compatibility. The worker runs with Hugging Face offline settings; this is not an OS network sandbox. Removing the marker disables new jobs.

## Provenance and verification

- [mlx-diffuser at the pinned revision](https://github.com/amirhossein-razlighi/mlx_diffuser/tree/a26b42aee4e31999dbb4429226b66d896d49e1d8) provides the MLX CLIP encoder, flow transformer and sampler implementations, and sparse kernels. Its original code is CC0-1.0; see its [third-party notices](https://github.com/amirhossein-razlighi/mlx_diffuser/blob/a26b42aee4e31999dbb4429226b66d896d49e1d8/THIRD_PARTY_NOTICES.md).
- MeshHarbor adds the text conditioning pipeline and ports Microsoft's [MIT-licensed SLAT mesh decoder](../crates/core/src/trellis_text_mlx/THIRD-PARTY-LICENSES/TRELLIS-LICENSE.txt) to MLX. Its CPU inference path adapts [FlexiCubes](https://github.com/MaxtirError/FlexiCubes/tree/815e075a2a400d06c48d94c347674344ed6ae5c5), copyright 2025 NVIDIA Corporation & Affiliates, under [Apache-2.0](../crates/core/src/trellis_text_mlx/THIRD-PARTY-LICENSES/FlexiCubes-LICENSE.txt). It reads the lookup tables from the local checkout without executing their Python. Model weight terms are unchanged.
- The example ran through the MeshHarbor CLI in a clean environment without PyTorch on an Apple M3 Pro with 18 GiB unified memory. Seed 42 produced a 100,000-triangle GLB in about 174 seconds of worker time. The output was not watertight. The surface port also matched the reference FlexiCubes triangle geometry in a small weighted synthetic test; this does not establish full CUDA numerical parity, broad prompt quality, or a minimum supported memory configuration.
