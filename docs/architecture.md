# Architecture and implementation status

## Intended architecture

The approved product direction is a Rust application with Axum/Tokio, Clap, the official `rmcp` Rust SDK, one operation core, SQLite metadata, a file-based artifact store, and a React/TypeScript/Three.js Studio. Model-specific CUDA and MLX workers remain separable backends and may also be vendored in this monorepo. One user entry binary should own REST, CLI, MCP, and the Studio entry point. The system must not imply that an unavailable model runtime is installable or commercially cleared.

## Implemented Rust slice

- `crates/core/src/store.rs` owns SQLite assets, artifacts and job state plus immutable SHA-256-addressed files. Source import creates a new asset; simplification creates a derived asset linked back to its source.
- `crates/core/src/glb.rs` parses GLB v2 chunks, inspects basic metadata, reads a conservative subset of static indexed triangle primitives, calls meshoptimizer simplification, then appends a new index accessor and writes a distinct GLB. Vertex attributes and the original GLB payload are not rewritten.
- `crates/core/src/operation.rs` is the shared operation catalog and `OperationCore` used by every current interface.
- `crates/core/src/runtime.rs` manages the isolated MLX uv/Python environment and NAF conversion; `cuda_runtime.rs` manages an isolated Linux x86_64/NVIDIA PyTorch 2.6.0/CUDA 12.4 environment and runs the upstream CUDA/SDPA setup preflight. Model downloads are explicit separate actions with required confirmation fields. Hugging Face login uses the official masked CLI in a per-backend private cache, never the Studio/REST/MCP. CUDA upstream lacks a committed lock, so first setup resolves a private per-install lock rather than claiming cross-machine reproducibility.
- `crates/core/src/generation.rs` routes the shared `image.generate` flow to `auto`, MLX, or CUDA, runs inference offline, validates the resulting GLB, and imports a derived asset. No backend automatically installs packages or downloads weights during generation.
- `crates/core/build.rs` embeds both pinned upstream source snapshots plus the first-party CUDA adapter manifest in the Rust executable. Runtime setup extracts source into application data, so source paths do not depend on the build machine's checkout. This does not bundle `uv`, Python, third-party wheels, or model weights.
- `crates/meshharbor/src/main.rs` provides CLI, loopback REST, one-slot async mesh jobs, and stdio MCP in a single Rust binary. The built-in dashboard gets tokenless access only with its non-simple marker and exact same-origin browser provenance; tokenless API writes require exact `Origin`. Valid bearer-authenticated non-browser REST clients may omit `Origin` for compatibility, but any supplied `Origin` must match the loopback `Host`. The marker is not local-process authentication: same-OS-user processes can forge request headers, which is documented as part of the single-user loopback trust boundary. Served Studio HTML sets `frame-ancestors 'none'` and `X-Frame-Options: DENY`.
- `crates/meshharbor/build.rs` embeds `apps/studio/dist` into that binary when a client build exists; otherwise the root route returns a local not-built notice.
- `apps/studio/` contains a React/TypeScript/Three.js source client for importing/previewing GLBs and PNG/JPEG inputs, starting/polling simplification, MLX/CUDA runtime setup/download jobs, backend selection, image-generation jobs, and downloading results. It connects automatically without asking for a token. The source is present, but npm dependencies were unavailable offline, so the bundle and browser workflow have not been validated.
- `meshopt` 0.7.0 supplies the Rust wrapper and builds meshoptimizer's C++ implementation as a normal dependency. There is no bespoke C++ worker and no copied `glb-mesh-refiner` core in the active product tree.

## Scope boundaries and incomplete work

The current code is a tested product-core slice, not the whole planned product:

- Studio source and a loopback static-serving/embed path exist, but the npm build, embedded production bundle, and browser workflow have not been validated. There is no desktop shell.
- Image-to-3D code is wired through Studio, CLI, REST jobs, and MCP. MLX setup is Apple Silicon only; CUDA setup is Linux x86_64/NVIDIA only. Neither setup nor actual inference has been run end-to-end in this executor. Gated DINOv3 access must be obtained by the user, model downloads require explicit consent, and unresolved model/checkpoint terms remain separate from the MIT source notices. The CUDA route is experimental, with no fresh current-source GPU validation or committed complete dependency lock.
- No cleanup, part separation, retopology, renderer-order optimization, mesh/texture compression, UV unwrap, or texture bake operation is implemented. The operation catalog marks those unavailable.
- Persistent job states exist, but queued jobs do not resume after restart; native C++ simplification cannot be cancelled mid-call.
- No cross-platform build matrix, installer, release signing, validated image-to-output inference run, or real-asset visual QA has been completed.

The latest managed-runtime changes have not been compiled or tested in this executor; it has no `cargo`, `rustc`, or `rustfmt` on `PATH`. Do not describe the current build as a complete M1 release or as Windows/macOS verified.

## Simplification and UV/baking boundary

The product simplification path uses meshoptimizer with border locking and weighted `NORMAL` / `TEXCOORD_0` metrics when those accessors are compatible. It keeps original UVs rather than automatically generating a new atlas. Because the current operation does not change vertex buffers, UVs, material definitions, or embedded texture bytes, it does not need xatlas or texture rebaking. The target triangle count is a goal, not a guarantee; material/primitive boundaries and error limits can leave the output above the requested target.

The older `glb-mesh-refiner` atlas baker was reviewed but is not part of the product path. It is an experimental one-primitive/one-material implementation with a narrow base-color plus metallic-roughness texture contract, no general normal/AO/emissive transfer, and no independent PBR render-equivalence test. xatlas UV generation alone does not establish a correct texture rebake. If future work adopts it, it needs a separate explicit opt-in, its own licensing notices, input/output validation, and broader visual tests.

## Planned structure

```text
apps/studio/               # React / TypeScript / Three.js source; npm build/browser QA pending
crates/meshharbor/          # one CLI/API/MCP/Studio-serving binary
crates/core/                # operation core, assets, jobs, storage
native/                     # only independently validated C/C++ dependencies
runtimes/pixal3d/cuda/       # pinned source plus managed Linux/NVIDIA CUDA runtime manifest
runtimes/pixal3d/mlx/        # pinned source, embedded by core and used by managed setup
registry/models/             # explicit installability and license status
docs/pixal3d-source-inventory.json # pinned upstream file hashes and exact exclusions
```

Optional future integrations must preserve build/runtime boundaries and licensing; adding a source tree does not make its dependency and model path installable.
