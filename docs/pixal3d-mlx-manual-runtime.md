# Pixal3D MLX: managed local setup

This is the first image-generation integration profile for Apple Silicon. MeshHarbor can provision an isolated Python 3.12 environment and, after a separate explicit action, download pinned model checkpoints. It never accepts model terms, requests account access, or handles a Hugging Face token through the Studio or API. The user's HF credential is entered only through the official interactive CLI and stored in a dedicated local cache. MeshHarbor's project license and Pixal3D source-port MIT notice do not replace model, checkpoint, and dependency terms.

## Prerequisites

- Apple Silicon macOS. This guide covers MLX only; the separate CUDA profile requires Linux x86_64 with NVIDIA.
- Rust/Cargo 1.88+ and native build tools required to build MeshHarbor.
- `curl` for the official uv release download if uv is not already installed. The runtime setup installs pinned uv 0.13.8 into MeshHarbor application data and verifies its SHA-256 before using it. macOS and Linux also need the system `tar` utility; Windows uses PowerShell `Expand-Archive`.
- Build the MeshHarbor executable from the repository with the MLX source tree present. The pinned source snapshot and adapter scripts are embedded into the executable and extracted under `<data-dir>/runtimes/pixal3d-mlx/source`; the original checkout is not needed when running that built binary.
- Around 25 GB of free disk space for the seven Pixal3D checkpoints, gated DINOv3, NAF and the isolated environment. MoGe is skipped for the fixed-camera generation profile.
- Hugging Face account access to the gated DINOv3 repository. The owner must review and accept any required terms and obtain access in Hugging Face before downloading. Do not paste the token into chat, Studio, or source code.

## Setup and model download

Use the same data directory for the server and the following CLI commands. The default on macOS is `~/Library/Application Support/MeshHarbor`; `--data-dir` / `MESHHARBOR_HOME` overrides it.

~~~sh
# Install pinned uv if missing, create managed Python 3.12, and install exact locked MLX dependencies
meshharbor runtime setup

# In a terminal on this computer, enter the HF token only in the official masked prompt.
# This writes it to MeshHarbor's private HF cache, not your ordinary HF cache.
meshharbor runtime hf-login

# First review/accept DINOv3 terms and request access in your own HF account.
# This then preflights access and explicitly downloads about 24 GB of model files.
meshharbor runtime install-models --confirm-large-download --confirm-gated-access

meshharbor doctor
~~~

`runtime install-models` checks access to the pinned gated DINOv3 repository before downloading Pixal3D's large checkpoints, so an unapproved request fails early. It pins the known source revisions, verifies Pixal3D and NAF checkpoint hashes, uses a dedicated Hugging Face cache under `<data-dir>/runtimes/pixal3d-mlx/home`, and saves NAF under `<data-dir>/runtimes/pixal3d-mlx/weights`. The isolated token is used only for the Hugging Face model request; MeshHarbor does not expose it to generation jobs. The downloader deliberately skips MoGe because the supported generation profile uses a fixed camera and passes `--no-moge`. After download, MeshHarbor creates a separate CPU-only converter environment with pinned PyTorch, safetensors, and NumPy versions, converts and verifies NAF, and leaves the MLX inference environment unchanged. If conversion fails after the model files arrive, retry with `meshharbor runtime convert-naf` or the Studio's retry button.

## Generate

Import a PNG/JPEG using Studio or `meshharbor import`, then run:

~~~sh
meshharbor generate asset_<image-id> --output ./generated.glb
~~~

Studio also exposes explicit environment setup, model-download progress, and image-generation jobs. The download action requires an acknowledgment that the user has completed the gated access process. `meshharbor runtime hf-login` remains a separate terminal command so the UI never receives a token. Setup/download progress is shown as an activity indicator; detailed package and per-file transfer output appears in the engine terminal.

Generation uses the fixed reviewed 512 profile, the local pinned Pixal3D/DINOv3/NAF files, Hugging Face offline mode, no background-removal model, and the fan hole filler. The first DINOv3 cache revision must be the pinned revision `ea8dc2863c51be0a264bab82070e3e8836b02d51`. The managed HF cache is dedicated to MeshHarbor and avoids changing/deleting the user's existing HF cache.

## Runtime source and licensing boundary

- The source-port MIT notice is preserved.
- DINOv3 has a separate agreement and gated checkpoint access. NAF and all other checkpoint/dependency terms remain separate from MeshHarbor's own license.
- This feature does not clear redistribution of model weights, third-party wheels, or commercial usage.
- No model weights or user images are shipped in this repository.
- The embedded code snapshot is not a self-contained Python distribution: setup downloads the pinned uv binary only if needed and fetches dependencies and model files when explicitly requested.
- `runtime doctor` checks local file paths only; it does not guarantee memory capacity, dependency health after setup, or successful end-to-end inference.

## Validation status

- The current executor has no `cargo`, `rustc`, or `rustfmt` on `PATH`; after the latest managed-runtime and source-bundling changes, Rust formatting, compilation, and tests remain unverified.
- This environment is not Apple Silicon macOS. Managed MLX installation, HF gated access, model download, and inference have not been exercised.
- Studio security tests can run independently. The npm package cache is absent here, so the TypeScript bundle/browser workflow may remain unverified.
