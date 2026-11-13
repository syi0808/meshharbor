# Managed Pixal3D CUDA runtime

MeshHarbor now has an opt-in CUDA path for the vendored Pixal3D snapshot. It is
limited to **Linux x86_64 with a working NVIDIA driver/GPU**. Apple Silicon
continues to use the separate MLX backend. uv's cross-platform bootstrap does
not make CUDA available on macOS or Windows.

## Setup and generation

```sh
# Uses the same MESHHARBOR_HOME/--data-dir as the local server.
meshharbor runtime cuda-setup
meshharbor runtime cuda-hf-login

# Do this only after accepting/accessing the gated DINOv3 model in your account.
# The two flags are required confirmations for a roughly 25–30 GB download.
meshharbor runtime cuda-install-models \
  --confirm-large-download \
  --confirm-gated-access

meshharbor import ./input.png
meshharbor assets --limit 20  # copy the returned asset id
meshharbor generate asset_<image-id> --backend-id pixal3d-cuda --output ./result.glb
```

`cuda-setup` detects `nvidia-smi` before installing packages, then installs a
managed Python 3.12 environment with PyTorch 2.6.0/torchvision 0.21.0 from the
official CUDA 12.4 index and runs the upstream CUDA/SDPA preflight. Model files
are a separate action. MeshHarbor does not download them automatically while
setting up or generating.

The upstream CUDA repository has dependency ranges but no committed `uv.lock`.
On first explicit setup, uv resolves the pinned profile and upstream ranges and
stores an app-private per-install lock under the local data directory. That
provides repeatable installs for that data directory, but is **not** a
repository-committed, cross-machine dependency lock. The optional `preprocess`
and `camera` extras are omitted: inference uses `--no-rembg` and a fixed camera,
so it does not install the extra MoGe model.

Hugging Face login is the official interactive `hf auth login` command inside
MeshHarbor's isolated `HF_HOME`. Neither REST nor MCP accepts credentials. The
large model action first queries gated DINOv3 metadata through that isolated
account; it fails before checkpoint transfer if access is absent. Once the
pinned files are present, inference sets Hugging Face offline mode and does not
inherit ambient tokens or user Python packages.

## Studio, REST, and MCP

Studio shows separate MLX and CUDA setup panels, an automatic/platform backend
selector, and a distinct consent checkbox for the CUDA download. The same
generation option (`backend_id: "auto" | "pixal3d-mlx" | "pixal3d-cuda"`) is
accepted by the CLI, `POST /api/v1/jobs` `image.generate`, and MCP
`start_image_generation`. Runtime setup/model installation are asynchronous:

- `POST /api/v1/runtime/pixal3d-cuda/setup`
- `POST /api/v1/runtime/pixal3d-cuda/models` with both
  `confirm_large_download` and `confirm_gated_access` set to `true`
- MCP `setup_pixal3d_cuda_runtime` and `install_pixal3d_cuda_models` with the
  same confirmation fields

The result is validated as a GLB and imported as a derived local asset. Studio
can preview it; CLI can write it with `--output`; REST exposes the artifact;
MCP's `export_glb` writes only inside its configured export directory and never
overwrites an existing file.

## Limits and licensing

The CUDA and MLX source ports are MIT-licensed, but that does **not** grant the
same rights to upstream model/checkpoint weights. DINOv3 is separately gated;
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
