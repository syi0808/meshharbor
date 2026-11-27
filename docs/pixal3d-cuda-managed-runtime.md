# Managed Pixal3D CUDA runtime

MeshHarbor now has an opt-in CUDA path for the vendored Pixal3D snapshot. It is
limited to **Linux x86_64 with a working NVIDIA driver/GPU**. Apple Silicon
continues to use the separate MLX backend. uv's cross-platform bootstrap does
not make CUDA available on macOS or Windows.

## Setup and generation

Start the server and set its bearer token as described in the [CLI guide](cli.md#control-the-running-engine).

```sh
meshharbor engine runtime setup-environment --confirmed
# After setup finishes, complete device authorization in your browser.
meshharbor engine connections login-start cuda
meshharbor engine connections login-status cuda "$SESSION_ID"
# Accept the gated DINOv3 terms before confirming the model download.
meshharbor engine runtime install pixal3d-cuda --confirmed --confirm-gated-access
meshharbor engine assets import ./input.png
meshharbor engine generation image "$IMAGE_ASSET_ID" --backend pixal3d-cuda
meshharbor engine jobs show "$JOB_ID"
```

The catalog installer detects `nvidia-smi` before installing packages, then installs a
managed Python 3.12 environment with PyTorch 2.6.0/torchvision 0.21.0 from the
official CUDA 12.4 index and runs the upstream CUDA/SDPA preflight. The same confirmed job provisions base model files. Generation never installs packages or downloads weights.

The upstream CUDA repository has dependency ranges but no committed `uv.lock`.
On first explicit setup, uv resolves the pinned profile and upstream ranges and
stores an app-private per-install lock under the local data directory. That
provides repeatable installs for that data directory, but is **not** a
repository-committed, cross-machine dependency lock. The optional `preprocess`
and `camera` extras are omitted: inference uses `--no-rembg` and either a fixed
single-view camera or explicit cameras for posed inputs. MoGe is not installed.

After host validation, each setup attempt removes any previous CUDA
preflight marker before changing the managed runtime. A new marker is
published atomically only after dependency sync, the PyTorch/CUDA doctor check,
and package-version verification succeed. If any later setup step fails or is
interrupted before successful publication, readiness stays false until a
successful retry; model files in the isolated Hugging Face cache are not
removed. Readiness accepts only a bounded JSON marker matching the reviewed
CUDA profile. It does not re-run the doctor or re-check the current driver.

Connect a Hugging Face token through the local Studio setup panel, or use the
official interactive `hf auth login` command inside MeshHarbor's isolated
`HF_HOME`. The Studio endpoint requires the exact same local origin and MCP
does not accept credentials. The large model action first sends a HEAD request
for the pinned DINOv3 checkpoint file and fails before transfer if access is absent.
MeshHarbor symlinks matching Pixal3D and DINOv3 files from the regular Hub cache
and uses `hf download` for files it cannot reuse. Retries reuse the same private
Hub cache. NAF is symlinked if present in the upstream Pixal3D CUDA cache and
its SHA-256 matches; otherwise it is downloaded separately and verified.
Once the pinned files are present, inference sets Hugging Face
offline mode and does not
inherit ambient tokens or user Python packages.

## Studio, REST, and MCP

Studio **Settings → Models** shows installation, downloads, file paths, and
recent logs for the Pixal3D MLX and CUDA profiles. Use **Connections** for
Hugging Face sign-in. The workbench shows model
names and automatically selects a locally ready runtime. The same
generation option (`backend_id: "auto" | "pixal3d-mlx" | "pixal3d-cuda"`) is
accepted by the CLI, `POST /api/v1/generation`, and MCP `generate_model`. REST and MCP requests use `input.kind: "image"`. Activate the base profile with `POST /api/v1/runtime/pixal3d-cuda/install` or MCP `engine_runtime` `install`. Use `package: "multiview"` on the same feature endpoint for the optional CUDA weights.

The result is validated as a GLB and stored as a new asset. After the job succeeds, Studio previews the asset and REST exposes its artifacts. CLI and MCP can export exact bytes via `engine assets export` or `engine_assets` `export`.

## Posed generation in Studio

After the base model is ready, open the Pixal3D CUDA card in **Settings → Models** and approve **Install multiview weights**. The engine prepares the separate pinned Pixal3D multiview snapshot `b0cb2e1b794cab9aa0ac38a95d794a4d9337437f`, reusing matching local Hub files. The base model and the gated DINOv3/NAF files are not reinstalled. No weights are fetched during generation.

Select **Pixal3D** in **Generate model**, then choose 4 views (front, right, back, left), 6 views (adds top and bottom), or Custom views. Presets use an assumed 20° FOV and a shared camera distance; use 2–8 custom views to enter an actual camera orientation, distance, and FOV for each. The first image defines the front frame and job lineage. Use masked, consistently framed inputs.

## Limits and licensing

The CUDA and MLX source snapshots retain their MIT port notices and
[separate third-party terms](../runtimes/pixal3d/THIRD-PARTY-NOTICES.md).
Those notices do not grant the same rights to model weights. DINOv3 is separately gated;
NAF checkpoint inference scope and other model terms remain flagged in
`registry/models/pixal3d.json`. MeshHarbor does not accept model terms for the
user. This implementation is an opt-in experimental route, not a claim that
every model license is cleared for commercial use.

Runtime readiness checks local paths and `nvidia-smi`; setup additionally runs
the upstream PyTorch CUDA/SDPA smoke test. No GPU inference was run as part of
this implementation, and current vendored CUDA changes still need fresh T4 /
supported-device end-to-end validation. Historical upstream T4 results do not
guarantee quality, latency, or fit for another GPU. The upstream docs recommend
roughly 40 GiB free disk before downloading and record visible surface
artifacts; this wrapper does not promise a hard GPU-memory cap or quality
parity with the MLX path.
