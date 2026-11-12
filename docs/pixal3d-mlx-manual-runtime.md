# Pixal3D MLX: manually provisioned runtime

This is the first image-generation integration profile for Apple Silicon. It
reuses the pinned MLX backend source at
`runtimes/pixal3d/mlx/upstream`; it does not install Python packages, log in
to Hugging Face, accept model terms, or download checkpoints on the user's
behalf. MeshHarbor's own project license and the source-port MIT notice do not
replace model, checkpoint, and dependency terms.

## Prerequisites

- Apple Silicon macOS with a working Metal/MLX environment. The upstream
  reference reports the 512 profile on 16 GB unified memory, but this is not
  a hard memory guarantee or an independent MeshHarbor benchmark.
- Rust/Cargo 1.88+ and the native build tools already required by MeshHarbor.
- uv and Python 3.12.
- Approximately 25 GB or more free disk space for Pixal3D, DINOv3, MoGe, NAF,
  and the isolated Python environment.
- Hugging Face access to the DINOv3 repository. The account owner must obtain
  access and manage their own Hugging Face authentication locally. Do not put
  an HF token in chat, source, logs, or MeshHarbor configuration.

## Prepare the pinned backend

From the MeshHarbor checkout:

~~~sh
cd runtimes/pixal3d/mlx/upstream
uv venv .venv --python 3.12
uv sync --frozen
~~~

Review and accept any required upstream model access terms in the model host's
own account flow. When you are ready, run the pinned upstream downloader
yourself. It retrieves large checkpoints, uses the local HF credential for
gated DINOv3, and verifies Pixal3D checkpoint hashes:

~~~sh
.venv/bin/python scripts/download_models.py --verify
~~~

The upstream pipeline needs the converted NAF weights. Its documented
one-time conversion requires PyTorch and safetensors:

~~~sh
.venv/bin/python -m pip install torch safetensors
.venv/bin/python -c "import torch; from safetensors.torch import save_file; save_file(torch.load('weights/naf_release.pth', map_location='cpu', weights_only=True), 'weights/naf_release.safetensors')"
~~~

This installs an additional package into the isolated runtime and is not done
by MeshHarbor automatically.

## Run the upstream smoke generation

Use a local input image. This test explicitly skips MoGe and background
removal, uses the backend's fixed-FOV and fan-hole-fill paths, and performs no
model or dependency downloads during inference:

~~~sh
.venv/bin/python generate_pixal3d.py \
  --image /absolute/path/to/input.png \
  --output /absolute/path/to/output.glb \
  --resolution 512 \
  --texture-size 1024 \
  --target-faces 100000 \
  --no-moge \
  --no-rembg \
  --hole-fill-backend fan
~~~

Without rembg, use a foreground-prepared image for best results. The face
count is a soft target, not a hard cap.

## MeshHarbor integration status

The Rust core contains an opt-in process wrapper that requires these optional
environment variables when the source is outside the checkout's default
location:

~~~sh
export MESHHARBOR_PIXAL3D_MLX_ROOT="/absolute/path/to/meshharbor/runtimes/pixal3d/mlx/upstream"
export MESHHARBOR_PIXAL3D_MLX_PYTHON="/absolute/path/to/meshharbor/runtimes/pixal3d/mlx/upstream/.venv/bin/python"
~~~

It probes only local paths and never downloads or authenticates. Its supported
profile uses local pinned Pixal3D/DINOv3/NAF checkpoints, offline Hugging Face
mode, fixed camera, no background-removal model, and the fan hole filler. If
any required local checkpoint is absent, runtime_doctor reports it as
not-ready. The pinned MLX entrypoint selects the first non-hidden DINOv3 cache
snapshot it finds, so MeshHarbor enables generation only when the selected
`HOME` cache contains exactly the pinned revision
`ea8dc2863c51be0a264bab82070e3e8836b02d51`. If the account cache has other
DINOv3 revisions, use a dedicated local `HOME` cache for this manually prepared
runtime; the application does not delete or rearrange the user's existing HF
cache.

MeshHarbor now exposes this profile through `meshharbor generate`, the
`image.generate` REST job (`POST /api/v1/jobs`), and the MCP tool
`start_image_generation`. The REST/MCP calls return a persistent job; poll
`GET /api/v1/jobs/{job_id}` and use `result.asset_id` to retrieve the derived
GLB artifact. The CLI accepts an imported image `asset_id` and optional
`--seed`, `--resolution`, `--texture-size`, `--target-faces`, and `--output`.
Only resolution 512 is enabled in this integration. MeshHarbor reports
checkpoint/path readiness, but does not validate the full Python dependency
environment or guarantee an inference run will fit available memory. The
upstream smoke command above remains useful for checking that runtime outside
MeshHarbor before submitting a long job.

The Studio exposes PNG/JPEG import, texture-size and soft face-target controls,
and persistent job polling. It enables the Generate action only when the local
path/checkpoint readiness probe passes; that probe does not verify installed
Python packages or model execution. The REST job API, MCP tool, and CLI remain
available as alternate interfaces.

## Conditions retained

- Pixal3D's source-port MIT notice is preserved.
- DINOv3 remains governed by its separate agreement and gated checkpoint
  access. NAF and all model/dependency terms remain separate from the
  repository's own license.
- This manual setup is intended for the user's local installation. It does not
  clear redistribution of weights, runtime bundles, or commercial usage.
- No model weights or user images are shipped in this repository.

## Validation status for this integration

- `cargo fmt --all -- --check` and offline Cargo metadata validation pass.
- `cargo test --workspace --locked --offline` could not reach compilation:
  this environment has no cached `meshopt` crate, and its allowed network path
  does not include crates.io. The Rust integration and Rust auth changes are
  therefore not compile- or test-verified here.
- Studio `npm run test:security` passes 4/4 tests. npm dependencies are not
  available in the local offline cache, so the TypeScript bundle and browser
  workflow were not built or exercised.
- No Pixal3D checkpoints were provisioned, so no model inference was run.
  Path readiness is not evidence that Python dependencies, memory capacity, or
  an end-to-end generation run will succeed.
