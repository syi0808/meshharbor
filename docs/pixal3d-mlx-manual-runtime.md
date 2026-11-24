# Pixal3D MLX: managed local setup

This is the first image-generation integration profile for Apple Silicon. MeshHarbor can provision an isolated Python 3.12 environment and, after a separate explicit action, download pinned model checkpoints. Users accept terms, request access, and create tokens on Hugging Face. A token can be connected through local Studio or the interactive CLI and is stored in a dedicated local cache. MeshHarbor's project license and Pixal3D source-port MIT notice do not replace model, checkpoint, and dependency terms.

## Prerequisites

- Apple Silicon macOS. This guide covers MLX only; the separate CUDA profile requires Linux x86_64 with NVIDIA.
- Rust/Cargo 1.88+ and native build tools required to build MeshHarbor.
- `curl` for the official uv release download if uv is not already installed. The runtime setup installs pinned uv 0.13.8 into MeshHarbor application data and verifies its SHA-256 before using it. macOS and Linux also need the system `tar` utility; Windows uses PowerShell `Expand-Archive`.
- Build the MeshHarbor executable from the repository with the MLX source tree present. The pinned source snapshot and adapter scripts are embedded into the executable and extracted under `<data-dir>/runtimes/pixal3d-mlx/source`; the original checkout is not needed when running that built binary.
- Around 25 GB of free disk space for the seven Pixal3D checkpoints, gated DINOv3, NAF and the isolated environment. MoGe is skipped for the fixed-camera generation profile.
- Hugging Face account access to the gated DINOv3 repository. The owner must review and accept any required terms and obtain access in Hugging Face before downloading. Only enter the token in local Studio's connection form or the CLI's masked prompt, never in chat or source code.

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

`runtime install-models` checks access to the pinned gated DINOv3 checkpoint file with a HEAD request before transferring any weights. It delegates the pinned DINOv3 and Pixal3D transfers to `hf download`, preferring the browser sign-in CLI if it has been installed and otherwise using the CLI in the backend's environment. Both use the same dedicated Hugging Face cache under `<data-dir>/runtimes/pixal3d-mlx/home`, including cached files from earlier runs. MeshHarbor verifies the Pixal3D hashes and separately fetches and verifies NAF under `<data-dir>/runtimes/pixal3d-mlx/weights`. The isolated token is used for Hugging Face access checks and downloads; MeshHarbor does not expose it to generation jobs. The downloader deliberately skips MoGe because the supported generation profile uses a fixed camera and passes `--no-moge`. After download, MeshHarbor creates a separate CPU-only converter environment with pinned PyTorch, safetensors, NumPy, and packaging versions, converts and verifies NAF, and leaves the MLX inference environment unchanged. If conversion fails after the model files arrive, retry with `meshharbor runtime convert-naf` or Studio's **Finish NAF conversion** button.

## Generate

Import a PNG/JPEG using Studio or `meshharbor import`, then run:

~~~sh
meshharbor generate asset_<image-id> --output ./generated.glb
~~~

In **Settings**, set up the environment under **Models**, then select **Sign in with browser** under **Connections**. Studio uses an app-managed Hugging Face CLI to complete device authorization without changing the pinned MLX environment. Manual token entry and `meshharbor runtime hf-login` remain available. Return to **Models** and use the link on the Pixal3D MLX card to request DINOv3 access if needed. Studio checks access automatically; approve the download separately. The installer reuses matching public Pixal3D files from your regular Hugging Face Hub cache and transfers only missing files. Use **Recent logs** on the backend card to inspect the setup and transfer output.

Generation uses the fixed reviewed 512 profile, the local pinned Pixal3D/DINOv3/NAF files, Hugging Face offline mode, no background-removal model, and the fan hole filler. The first DINOv3 cache revision must be the pinned revision `ea8dc2863c51be0a264bab82070e3e8836b02d51`. The managed HF cache is dedicated to MeshHarbor and avoids changing/deleting the user's existing HF cache.

### Mesh postprocessing

[Official Pixal3D export](https://github.com/TencentARC/Pixal3D/blob/master/inference.py) uses CUDA O-Voxel remeshing and simplification. The managed MLX path does not implement that CUDA remesher. It reuses the pinned MLX runtime's component filter, simplifier, edge repair, hole filling, and winding correction. The MeshHarbor overlay filters only the original disconnected components, runs simplification before splitting non-manifold edges, and then repairs the result. It does not delete newly separated parts as debris or rerun simplification on split edges. Model sampling, texture baking, and the upstream source snapshot are unchanged.

The face target remains soft because final hole filling can add triangles. In a replay of the same 512-profile decoded shoe mesh at a 100,000-face target, the previous order produced 10,824 edge-connected components. The new order produced 7 (126,685 faces), counting coincident vertex positions together to avoid counting seams as disconnected. A separate end-to-end CLI run on the Apple M3 Pro produced the same face and component counts and a GLB with embedded base color and metallic/roughness textures. This verifies one input, not CUDA remeshing parity.

## Runtime source and licensing boundary

- The source-port MIT notice is preserved.
- DINOv3 has a separate agreement and gated checkpoint access. NAF and all other checkpoint/dependency terms remain separate from MeshHarbor's own license.
- This feature does not clear redistribution of model weights, third-party wheels, or commercial usage.
- No model weights or user images are shipped in this repository.
- The embedded code snapshot is not a self-contained Python distribution: setup downloads the pinned uv binary only if needed and fetches dependencies and model files when explicitly requested.
- `runtime doctor` checks local file paths only; it does not guarantee memory capacity, dependency health after setup, or successful end-to-end inference.

## Validation status

- On 2026-11-18, CLI and embedded Studio setup both succeeded on an Apple M3 Pro (18 GiB) running macOS 26.2. With no `uv` on PATH and a new data directory, Studio's setup job bootstrapped uv 0.13.8, installed Python 3.12.16 and the locked dependencies, and passed a Metal GPU calculation.
- Disk space did not permit a fresh transfer of all checkpoints. The test reused APFS clones of the pinned Pixal3D and DINOv3 weights in the isolated data directory and an existing authorized Hugging Face login. The managed downloader fetched the omitted Pixal3D and DINOv3 JSON files and the NAF checkpoint over the network. It verified SHA-256 for all seven Pixal3D checkpoints and the NAF file. The interactive sign-in and full clean model download were not tested.
- Fresh NAF conversion originally failed because `safetensors.torch` imports `packaging`, which was not installed in the separate converter environment. After pinning it, both the CLI retry and Studio's **Finish NAF conversion** completed without redownloading model weights.
- Studio imported a transparent 512×512 robot PNG and ran the actual Pixal3D MLX worker offline for **5 min 53 s** (geometry profile 512, texture size 512, seed 42, soft target 20,000 faces). The resulting GLB rendered in Studio with 20,152 triangles, one material, and two nonuniform embedded 512×512 PNG maps (base color and metallic/roughness). The CLI read the same persisted job. The MCP server read that Studio job and exported a GLB byte-identical to Studio's download.
- Studio optimized the generated GLB from 20,152 to 10,021 triangles, switched between Original and Result, and exported both. The Original export matched the generated file byte for byte. The material definitions, bindings, UVs, normals, and embedded textures were unchanged. The simpler GLB grew from 2.36 to 2.42 MiB; a separate CLI `mesh.compress` run reduced the generated GLB to 2.01 MiB and requires a meshopt-capable viewer.
- CLI cleanup and render optimization ran on the same GLB. A follow-up resolved two reproduced failures: seam-aware part separation finds 1,482 components and emits 256 primitives without losing triangles, and standalone `uv.unwrap` rebakes both textures into a 2,080×2,076 atlas after excluding only 99 zero-area faces. Pixal3D generation's own bake is a separate path.
- This verifies one Mac and one input. It is not a claim about visual quality across inputs, full cold installation, CUDA, or delivery of a desktop installer. QuadriFlow retopology is unavailable in the tested build because no approved signed sidecar is published.
