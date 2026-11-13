# Architecture and implementation status

## Intended architecture

The approved product direction is a Rust application with Axum/Tokio, Clap, the official `rmcp` Rust SDK, one operation core, SQLite metadata, a file-based artifact store, and a React/TypeScript/Three.js Studio. Model-specific CUDA and MLX workers remain separable backends and may also be vendored in this monorepo. One user entry binary should own REST, CLI, MCP, and the Studio entry point. The system must not imply that an unavailable model runtime is installable or commercially cleared.

## Implemented Rust slice

- `crates/core/src/store.rs` owns SQLite assets, artifacts and job state plus immutable SHA-256-addressed files. Source import creates a new asset; simplification creates a derived asset linked back to its source.
- `crates/core/src/glb.rs` parses GLB v2 chunks, inspects metadata, and implements meshoptimizer simplification, conservative face cleanup, vertex-cache/overdraw render optimization, and required `KHR_meshopt_compression` output for supported static indexed triangle data. `crates/core/src/parts.rs` groups triangles by shared vertex-index connectivity and emits multiple primitives within the same mesh; it does not split scene nodes or retopologize.
- `crates/atlas-baker` and `crates/xatlas-sys` provide a bounded xatlas integration: one supported static mesh/primitive/material, with same-mesh rebaking of embedded base-color and metallic/roughness PNG maps. This is not high-to-low projection; unsupported textures, extensions, tangents, and multi-primitive assets are rejected. Exact vendored-source notices are retained with the xatlas crate.
- `crates/core/src/operation.rs` is the shared operation catalog and `OperationCore` used by every current interface.
- `crates/core/src/runtime.rs` manages the isolated MLX uv/Python environment and NAF conversion; `cuda_runtime.rs` manages an isolated Linux x86_64/NVIDIA PyTorch 2.6.0/CUDA 12.4 environment and runs the upstream CUDA/SDPA setup preflight. Model downloads are explicit separate actions with required confirmation fields. Hugging Face login uses the official masked CLI in a per-backend private cache, never the Studio/REST/MCP. CUDA upstream lacks a committed lock, so first setup resolves a private per-install lock rather than claiming cross-machine reproducibility.
- `crates/core/src/generation.rs` routes the shared `image.generate` flow to `auto`, MLX, or CUDA, runs inference offline, validates the resulting GLB, and imports a derived asset. No backend automatically installs packages or downloads weights during generation.
- `crates/core/build.rs` embeds both pinned upstream source snapshots plus the first-party CUDA adapter manifest in the Rust executable. Runtime setup extracts source into application data, so source paths do not depend on the build machine's checkout. This does not bundle `uv`, Python, third-party wheels, or model weights.
- `crates/meshharbor/src/main.rs` provides CLI, loopback REST, one-slot async mesh jobs, and stdio MCP in a single Rust binary. The built-in dashboard gets tokenless access only with its non-simple marker and exact same-origin browser provenance; tokenless API writes require exact `Origin`. Valid bearer-authenticated non-browser REST clients may omit `Origin` for compatibility, but any supplied `Origin` must match the loopback `Host`. The marker is not local-process authentication: same-OS-user processes can forge request headers, which is documented as part of the single-user loopback trust boundary. Served Studio HTML sets `frame-ancestors 'none'` and `X-Frame-Options: DENY`.
- `crates/meshharbor/build.rs` embeds `apps/studio/dist` into that binary when a client build exists; otherwise the root route returns a local not-built notice.
- `apps/studio/` contains a React/TypeScript/Three.js source client for importing/previewing GLBs and PNG/JPEG inputs, starting/polling simplify, cleanup, part-separation, atlas-unwrap, render-optimization, and compression jobs, MLX/CUDA runtime setup/download jobs, backend selection, image-generation jobs, and downloading results. It connects automatically without asking for a token. The source is present, but npm dependencies were unavailable offline, so the bundle and browser workflow have not been validated.
- `meshopt` 0.7.0 supplies the Rust wrapper and builds meshoptimizer's C++ implementation as a normal dependency. There is no bespoke C++ worker and no copied `glb-mesh-refiner` core in the active product tree.

## Scope boundaries and incomplete work

The current code is a tested product-core slice, not the whole planned product:

- Studio source and a loopback static-serving/embed path exist, but the npm build, embedded production bundle, and browser workflow have not been validated. There is no desktop shell.
- Image-to-3D code is wired through Studio, CLI, REST jobs, and MCP. MLX setup is Apple Silicon only; CUDA setup is Linux x86_64/NVIDIA only. Neither setup nor actual inference has been run end-to-end in this executor. Gated DINOv3 access must be obtained by the user, model downloads require explicit consent, and unresolved model/checkpoint terms remain separate from the MIT source notices. The CUDA route is experimental, with no fresh current-source GPU validation or committed complete dependency lock.
- The operation catalog exposes cleanup, connected-part separation, renderer-order optimization, required mesh compression, and bounded xatlas UV atlas rebaking. The experimental remeshing worker remains unavailable until dependency lock/build, geometry validation, and quality gates pass. Generic high-to-low texture baking is not implemented. The current synthetic tests do not establish real-asset quality or broad glTF compatibility.
- Persistent job states exist, but queued jobs do not resume after restart; native C++ simplification cannot be cancelled mid-call.
- No cross-platform build matrix, installer, release signing, validated image-to-output inference run, or real-asset visual QA has been completed.

The latest managed-runtime changes have not been compiled or tested in this executor; it has no `cargo`, `rustc`, or `rustfmt` on `PATH`. Do not describe the current build as a complete M1 release or as Windows/macOS verified.

## Simplification and UV/baking boundary

The product simplification path uses meshoptimizer with border locking and weighted `NORMAL` / `TEXCOORD_0` metrics when those accessors are compatible. It keeps original UVs rather than automatically generating a new atlas. Because the current operation does not change vertex buffers, UVs, material definitions, or embedded texture bytes, it does not need xatlas or texture rebaking. The target triangle count is a goal, not a guarantee; material/primitive boundaries and error limits can leave the output above the requested target.

The product uses a distinct `crates/atlas-baker` xatlas integration and does not include the experimental `glb-mesh-refiner` implementation. It is a separate explicit operation with a one-static-primitive/one-material contract and only embedded base-color plus metallic/roughness rebaking from the same mesh's existing UVs. It does not transfer arbitrary maps or project a high-poly source. xatlas unwrap correctness and synthetic pixel-coverage checks are not equivalent to a production PBR visual-quality test. The worker remains bounded and rejects unsupported GLBs; broader fixture and target-renderer acceptance is still required.

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

