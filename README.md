# MeshHarbor

MeshHarbor is an early Rust-first, local 3D asset processing application. Its shared Rust operation core is exposed through a CLI, a loopback REST API, and the official Rust MCP SDK. It supports importing PNG/JPEG/GLB files; simplifying, cleaning, and separating a bounded class of static GLB meshes; xatlas-based UV atlas generation with same-mesh embedded PBR rebaking; meshoptimizer render-order optimization and KHR_meshopt_compression; and opt-in Pixal3D image-to-GLB generation through MLX on Apple Silicon or CUDA on Linux/NVIDIA. An experimental QuadriFlow quad-remeshing path is wired through the operation core but unavailable in the current product build. CLI, REST, MCP, and Studio expose backend status and an install request. Runtime code ignores adjacent executables; a managed sidecar can be resolved only after an in-process verifier accepts its release attestation and the executable hash matches the trusted manifest. The in-process verifier is integrated, but the checked-in manifest is empty, so status reports the backend unavailable and install/rollback requests fail closed without downloads or backend-directory writes. Unsigned temporary Actions artifacts are never installed. For its narrowly supported textured inputs, base-color and metallic/roughness maps are projected only onto the triangulated GLB preview, with strict fail-closed gates; the editable quad OBJ remains geometry-only. Generic high-to-low texture baking remains unimplemented.

This is **not yet a complete M1 product or release build**. A React/TypeScript/Three.js Studio client exists under `apps/studio`. GitHub Actions now validates the locked Studio build and launches a release-mode CLI with the bundle embedded on pinned Linux, macOS, and Windows runners. This is a startup smoke only: there is no browser/visual QA, desktop shell, installer, code signing, or published release package. Neither image-to-3D backend has been exercised end-to-end in this environment, and the CUDA route still needs a fresh supported-GPU acceptance run. Model weights are not bundled. MeshHarbor's project license has not been selected; this repository deliberately has no root `LICENSE`.

## Implemented now

- `crates/core`: GLB v2 parsing and inspection, source-immutable content-addressed artifact storage, SQLite asset/job records, operation catalog, and meshoptimizer-based simplification, conservative face cleanup, shared-index connected-part separation, vertex-cache/overdraw render optimization, KHR_meshopt_compression export, xatlas atlas UV generation with supported same-mesh texture rebaking, and a conditional QuadriFlow sidecar adapter.
- `crates/meshharbor`: one executable with CLI commands, a local Axum REST API, and stdio MCP tools, all using the same `OperationCore`.
- `apps/studio`: a source-only local Studio client for GLB and PNG/JPEG import, mesh preview, simplify/cleanup/part-separation/render-optimize/compress jobs, conditional retopology, MLX/CUDA runtime setup and download jobs, backend selection, image-generation jobs, and result download. The preview loads KHR_meshopt_compression with Three.js's local MeshoptDecoder. Python environments and checkpoint downloads are separate explicit actions. The Rust build embeds `apps/studio/dist` when present; without it, the server root serves a local “assets are not built” notice.
- `crates/core/src/runtime.rs`, `cuda_runtime.rs`, and `generation.rs`: opt-in isolated uv-managed Python runtimes, separated Hugging Face homes, user-controlled gated access, and offline image-to-GLB wrappers. The source snapshots and adapters are embedded in the Rust executable and extracted under application data during setup; no checkout-relative source path is needed. Setup bootstraps pinned official uv if absent and verifies its SHA-256 before use. The CUDA profile is Linux x86_64/NVIDIA only and runs a PyTorch CUDA/SDPA smoke test. Upstream has no CUDA lock file, so explicit setup resolves and stores a per-install lock in app data rather than claiming a committed cross-machine lock.
- `runtimes/pixal3d/{cuda,mlx}/upstream/`: byte-preserved source snapshots from the two pinned Pixal3D repositories, with per-file inventory and third-party notices. No model weights are bundled; model downloads require an explicit confirmation and gated DINOv3 access.
- `registry/models/pixal3d.json`: records the experimental, non-release status and unresolved upstream model/checkpoint terms. The MeshHarbor project license has not been selected in this repository, and model-weight commercial use remains uncleared.

The operations have explicit format and feature limits. Simplification, cleanup, render optimization, compression, and part separation work on bounded static indexed TRIANGLES data in embedded GLB buffers, reject unsupported animation/skin/morph/compression layouts, and keep the source asset immutable. Cleanup removes repeated-index, exactly zero-area, and same-winding duplicate faces only; it does not weld vertices or compact unused streams. Part separation is bounded to an embedded GLB up to 64 MiB total (JSON chunk up to 4 MiB), 1,000,000 POSITION vertices, 3,000,000 indices, and 256 output parts. These limits are enforced in the shared core before the CLI/API loads an oversized artifact for processing. It finds components by shared vertex indices and emits separate primitives inside the same GLB mesh; it does not make scene nodes or weld duplicate-position vertices. Render optimization uses meshoptimizer's vertex-cache and overdraw passes on opaque/masked primitives; translucent primitives are left in original order. Mesh compression emits the required `KHR_meshopt_compression` extension without an uncompressed geometry fallback, so the compressed GLB must be opened by an extension-aware viewer.

Atlas UV rebaking uses vendored MIT xatlas and supports exactly one static mesh/primitive/material with existing FLOAT UVs, embedded PNG base-color and metallic/roughness maps, and no unsupported extensions or tangent stream. It duplicates/remaps supported vertex streams, produces a new atlas, and rebakes those maps from the same mesh's prior UVs. The standalone `uv.unwrap` operation is available through REST, CLI, and MCP as a low-level capability, not as a separate Studio button. Retopology's supported textured path uses xatlas internally for its preview. This is not a generic high-to-low bake and does not handle arbitrary maps, external images, or multi-material assets. External image URIs are never fetched. The simplification triangle target is aspirational: its report gives the actual count and measured error.

QuadriFlow retopology is implemented as an experimental, sidecar-dependent operation, but it is unavailable in the current product build. Adjacent executables are ignored; runtime readiness requires a managed sidecar accepted by an in-process signature/provenance verifier and matched to the trusted manifest. The verifier is integrated, but the manifest is empty and no approved sidecar is listed. It accepts one closed static triangle GLB. For textured input it requires existing FLOAT `TEXCOORD_0`, optional `NORMAL`, and one embedded PNG base-color and metallic/roughness map; the source may not have tangent/other vertex streams, unsupported extensions, transforms, external images, or extra meshes/materials. A bounded nearest-surface projection with a 2%-of-source-bounds distance cap, facing-normal gate, at-least-99.5% coverage requirement, and opposing-shell ambiguity rejection rebakes only those two maps into the triangulated GLB preview. The separate OBJ preserves editable quad topology but contains geometry only. UVs, colors, other maps, rigs, and animation are not transferred, and this does not certify visual texture quality, self-intersection freedom, or measured surface error. The sidecar is not included or auto-installed; its source build and any package must retain applicable upstream/dependency notices. See [the retopology candidate limits](docs/retopology-candidate.md). Generic high-to-low texture baking remains unavailable.

External image URIs are never fetched. The simplification triangle target is aspirational: its report gives the actual count and measured error.

See [architecture status](docs/architecture.md), [license/dependency status](docs/licensing-status.md), and [source provenance](docs/source-provenance.md) before using or redistributing this code.

## Build and test

Requirements: Rust/Cargo 1.88 or later and a C++ toolchain for meshoptimizer and the vendored xatlas wrapper; CI pins Rust 1.88.0. To build the embedded Studio bundle reproducibly, use Node.js 24.22.3. Windows source builds also need the MSVC C++ toolchain, NASM 3.02 on `PATH`, and `CFLAGS=/std:c11` for the AWS-LC MSVC feature probe. See [the embedded Studio build/smoke guide](docs/embedded-studio-package-smoke.md).

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo run --bin meshharbor -- --help
cargo run --bin meshharbor -- doctor
```

Most tests cover synthetic GLB fixtures and the Rust API/core contracts. CI also builds and launches a release-mode MeshHarbor binary with its Studio bundle on Ubuntu 24.04, macOS 15, and Windows Server 2022; the smoke checks the local health endpoint and embedded HTML/JS/CSS assets. It is not an installer, signed release package, browser/visual QA, real-asset evaluation, or model-inference acceptance suite. The separate QuadriFlow source-build smoke uses only a synthetic cube; see [the retopology candidate notes](docs/retopology-candidate.md) for its platform limits.

The Studio client source is not part of the Rust unit tests. Run `npm ci --ignore-scripts --no-audit --no-fund` and `npm run build` in `apps/studio` before compiling MeshHarbor to embed the client. The committed npm lockfile keeps this install resolution deterministic.

## CLI

```sh
# Import and inspect
meshharbor import ./scene.glb
meshharbor assets --limit 20

# Simplify an imported asset; the source stays intact
meshharbor simplify asset_<id> --triangles 4000 --max-error 0.02 --output ./scene-simplified.glb

# Conservative cleanup, connected-part primitives, atlas UV rebake, render optimization, or required KHR_meshopt compression
meshharbor process-mesh asset_<id> --operation mesh.cleanup --output ./scene-cleaned.glb
meshharbor process-mesh asset_<id> --operation mesh.separate_parts --output ./scene-parts.glb
meshharbor process-mesh asset_<id> --operation uv.unwrap --output ./scene-atlas.glb
meshharbor process-mesh asset_<id> --operation render.optimize --output ./scene-render-optimized.glb
meshharbor process-mesh asset_<id> --operation mesh.compress --output ./scene-compressed.glb

# Experimental retopology is currently unavailable; adjacent binaries are ignored.
meshharbor process-mesh asset_<id> --operation mesh.retopology --target-quads 10000 --output ./quad-preview.glb --quad-output ./quad-topology.obj


# If MeshHarbor exited unexpectedly, first stop every REST/MCP process using this data directory.
# The command also refuses to run while a REST or MCP service holds the shared data-directory lock.
meshharbor jobs recover-interrupted --confirm-no-running-meshharbor

# Optional Apple Silicon MLX runtime setup (uv is installed automatically if missing)
meshharbor runtime setup
meshharbor runtime hf-login
meshharbor runtime install-models --confirm-large-download --confirm-gated-access
# Generate after the setup, gated access, and download steps complete
meshharbor generate asset_<image-id> --backend-id pixal3d-mlx --output ./generated.glb

# Optional Linux x86_64/NVIDIA CUDA runtime
meshharbor runtime cuda-setup
meshharbor runtime cuda-hf-login
meshharbor runtime cuda-install-models --confirm-large-download --confirm-gated-access
meshharbor generate asset_<image-id> --backend-id pixal3d-cuda --output ./generated.glb
```

The app stores source assets, managed Python environments, and isolated Hugging Face caches under its local data directory. By default that is `%LOCALAPPDATA%\MeshHarbor` on Windows (falling back to `%USERPROFILE%\AppData\Local\MeshHarbor`), `~/Library/Application Support/MeshHarbor` on macOS, and `$XDG_DATA_HOME/meshharbor` or `~/.local/share/meshharbor` on Linux. Empty environment-variable values are ignored. Set `MESHHARBOR_HOME` to override the location. Use the same `--data-dir` or `MESHHARBOR_HOME` value for `serve` and the corresponding `*-hf-login` command. Review and accept any model-host terms in your own account before downloading. CUDA is limited to Linux x86_64/NVIDIA, requests roughly 25–30 GB of weights, uses a fixed camera and does not install MoGe/rembg. MLX retains its separate conversion step and profile. See [the Pixal3D MLX guide](docs/pixal3d-mlx-manual-runtime.md) and [CUDA guide](docs/pixal3d-cuda-managed-runtime.md).

## REST API

Start only on loopback (non-loopback binds are rejected):

```sh
meshharbor serve --listen 127.0.0.1:7432
```

When `MESHHARBOR_API_TOKEN` is unset, a per-run temporary bearer token is generated and printed to stderr for non-dashboard API clients. A configured token must not be empty or whitespace-only, and its value is not written to logs. The built-in Studio connects automatically without a token. Dashboard calls require a non-simple marker and exact same-origin browser provenance; tokenless writes require an exact same-origin `Origin`. Safe dashboard reads may use browser `Sec-Fetch-Site: same-origin` only when browsers omit `Origin`. Non-browser clients with a valid bearer token may omit `Origin` on writes for REST compatibility; if supplied, `Origin` must still match the loopback `Host`. Cross-origin requests are rejected even if they carry a bearer token. The marker is not a secret or per-process credential: local processes running as the same OS user can forge request headers and are inside this single-user local trust boundary. This browser-origin check is intended to block hostile websites, cross-origin forms, and other local web origins, not malicious software running as the user. The server binds only to loopback and rejects untrusted Host or mismatched Origin values. The Studio page sends `Content-Security-Policy: frame-ancestors 'none'` and `X-Frame-Options: DENY` to prevent clickjacking. Current endpoints:

- `GET /api/v1/health`, `/api/v1/runtime`, `/api/v1/operations`
- `POST /api/v1/runtime/pixal3d-mlx/setup`, `/models`, `/convert-naf`; `/api/v1/runtime/pixal3d-cuda/setup` and `/models` for explicit asynchronous setup actions
- `GET /api/v1/assets?limit=50`; `POST /api/v1/assets?filename=scene.glb` with raw file bytes
- `GET /api/v1/assets/{asset_id}`; `POST /api/v1/assets/{asset_id}/simplify`
- `POST /api/v1/jobs`, `GET /api/v1/jobs/{job_id}`, `POST /api/v1/jobs/{job_id}/cancel`
- `GET /api/v1/artifacts/{artifact_id}`

Job requests support `mesh.simplify`, `mesh.cleanup`, `mesh.separate_parts`, `uv.unwrap`, `render.optimize`, `mesh.compress`, conditional `mesh.retopology`, `image.generate` (with optional `backend_id: "auto" | "pixal3d-mlx" | "pixal3d-cuda"`), and explicit runtime jobs. The low-level `uv.unwrap` operation is available through REST, CLI, and MCP but is not a standalone Studio tool. It accepts optional `resolution` (64–4096, default 1024) and `padding` (0–64, default 8). `mesh.retopology` is rejected unless a signature-verified, manifest-matched managed `quadriflow` sidecar is available; adjacent executables are ignored and this build has no verified release; it accepts optional `target_quads` (4–50,000, default 10,000) and produces both a geometry-only `quad_topology` OBJ artifact and a triangulated GLB preview. When the supported embedded PNG maps are present, only base-color and metallic/roughness textures are projected into that preview with distance/facing/coverage/ambiguity gates. `render.optimize` accepts `overdraw_threshold` (1.0–1.5, default 1.05). Generic high-to-low texture baking is not available. Model-install endpoints require explicit large-download and gated-access confirmation. The single operation slot serializes local work. Queued jobs can be cancelled; native simplification, runtime installation, downloads, and model inference cannot currently be interrupted safely after starting. Job state is persisted. After an unexpected exit, stop every REST/MCP process using the same data directory and run `meshharbor jobs recover-interrupted --confirm-no-running-meshharbor` to mark nonterminal jobs as failed. The command does not resume work or delete assets/download caches; inspect existing assets before retrying. The flag is an operator acknowledgement and MeshHarbor cannot verify that other processes are stopped. Image generation requires a ready backend and local model files; readiness does not prove quality or general GPU-memory fit.
## MCP

Run `meshharbor mcp` to start the stdio MCP server. Its output is the MCP protocol stream; diagnostics go to stderr. Tools include `list_operations`, `list_assets`, `import_asset`, `start_simplification`, `start_mesh_processing`, `start_image_generation`, MLX and CUDA runtime setup/model-download actions, `get_job`, `cancel_job`, `runtime_doctor`, `export_glb`, and `export_quad_obj`. `mesh.retopology` requires a signature-verified, manifest-matched managed QuadriFlow sidecar, which is unavailable in this build; adjacent executables are ignored; its OBJ export writes only inside the configured export directory and refuses existing files. MCP asset import accepts base64-encoded local file bytes and does not fetch files or weights. Model tools require explicit large-download and gated-access confirmations. Hugging Face credentials are never accepted by MCP; authenticate through the corresponding `meshharbor runtime *-hf-login` command in a local terminal. `export_glb` writes only inside the configured export directory (defaulting to the app data `exports` folder), refuses existing files, and does not transmit the GLB to the MCP client. See [the MCP workflow guide](docs/mcp-workflow.md).


