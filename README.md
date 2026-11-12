# MeshHarbor

MeshHarbor is an early Rust-first, local 3D asset processing application. Its shared Rust operation core is exposed through a CLI, a loopback REST API, and the official Rust MCP SDK. It supports importing PNG/JPEG/GLB files and simplifying a limited class of static GLB meshes with meshoptimizer. An opt-in Pixal3D MLX image-to-3D path is wired through the core, CLI, REST jobs, and MCP when the user has manually prepared the local Python runtime and checkpoints.

This is **not yet a complete M1 product or release build**. A React/TypeScript/Three.js Studio client exists under `apps/studio`, and the Rust server can embed a built client bundle, but its npm dependencies are not installed and a production bundle/browser QA have not been verified here. The image-to-3D path has not been run end-to-end in this environment; managed Python/model installation, Windows/macOS release validation, and production packaging are not implemented. MeshHarbor's project license has not been selected; this repository deliberately has no root `LICENSE`.

## Implemented now

- `crates/core`: GLB v2 parsing and inspection, source-immutable content-addressed artifact storage, SQLite asset/job records, operation catalog, and meshoptimizer-based GLB simplification.
- `crates/meshharbor`: one executable with CLI commands, a local Axum REST API, and stdio MCP tools, all using the same `OperationCore`.
- `apps/studio`: a source-only local Studio client for GLB and PNG/JPEG import, mesh preview, simplify jobs, MLX image-generation jobs, and result download. Generation controls are enabled only when the runtime path/checkpoint probe passes. The Rust build embeds `apps/studio/dist` when present; without it, the server root serves a local “assets are not built” notice.
- `crates/core/src/generation.rs`: an opt-in subprocess wrapper for the reviewed 512 Pixal3D MLX profile. It checks locally provisioned checkpoints, forces Hugging Face offline mode, and never installs packages, accepts gated terms, or downloads weights.
- `runtimes/pixal3d/{cuda,mlx}/upstream/`: byte-preserved source snapshots from the two pinned Pixal3D repositories, with per-file inventory and third-party notices. CUDA is source-only; MLX has a manual local-runtime integration, not a managed installer.
- `registry/models/pixal3d.json`: both backends remain non-installable; no model weights or automatic model downloads are included.

The GLB simplifier is deliberately narrower than general-purpose glTF: one embedded GLB buffer, indexed triangle primitives, static meshes, FLOAT `POSITION`, and non-sparse accessors. It rejects animations, skins, morph targets, extension-bearing primitives, compressed mesh extensions, external buffers, and unsupported data layouts. Original vertex and binary streams, primitive/material references, and embedded texture payloads are left unchanged; only new index accessors are appended. `NORMAL` and `TEXCOORD_0` are included in the simplification attribute metric when supported. This does not re-unwrap UVs or rebake textures. External image URIs are not fetched or copied. The triangle target is aspirational: the report gives the actual count, measured meshoptimizer error, and a warning when constraints prevent reaching the target.

See [architecture status](docs/architecture.md), [license/dependency status](docs/licensing-status.md), and [source provenance](docs/source-provenance.md) before using or redistributing this code.

## Build and test

Requirements: Rust/Cargo 1.88 or later and a C++ toolchain for the meshoptimizer crate's native build.

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo run --bin meshharbor -- --help
cargo run --bin meshharbor -- doctor
```

Test coverage is focused on synthetic GLB fixtures and the Rust API/core contracts; it is not a cross-platform, real-asset, visual-quality, or model-inference acceptance suite.

The Studio client source is not part of the Rust test suite. Its npm dependencies must be installed before `npm run build` can create the bundle embedded by the Rust build. The client build and browser workflow were not verified in this environment because the required npm packages were not available in the offline cache.

## CLI

```sh
# Import and inspect
meshharbor import ./scene.glb
meshharbor assets --limit 20

# Simplify an imported asset; the source stays intact
meshharbor simplify asset_<id> --triangles 4000 --max-error 0.02 --output ./scene-simplified.glb

# Generate a GLB from an imported image; see the manual runtime setup first
meshharbor generate asset_<image-id> --output ./generated.glb
```

The app stores source and derived artifacts under its local data directory. Use `--data-dir` or `MESHHARBOR_HOME` to choose it. Before `generate`, follow [the manual Pixal3D MLX runtime setup](docs/pixal3d-mlx-manual-runtime.md). MeshHarbor does not install dependencies or fetch gated or public model weights.

## REST API

Start only on loopback (non-loopback binds are rejected):

```sh
meshharbor serve --listen 127.0.0.1:7432
```

A temporary bearer token is printed to stderr for non-dashboard API clients. The built-in Studio connects automatically without a token. Dashboard calls require a non-simple marker and exact same-origin browser provenance; tokenless writes require an exact same-origin `Origin`. Safe dashboard reads may use browser `Sec-Fetch-Site: same-origin` only when browsers omit `Origin`. Non-browser clients with a valid bearer token may omit `Origin` on writes for REST compatibility; if supplied, `Origin` must still match the loopback `Host`. Cross-origin requests are rejected even if they carry a bearer token. The marker is not a secret or per-process credential: local processes running as the same OS user can forge request headers and are inside this single-user local trust boundary. This browser-origin check is intended to block hostile websites, cross-origin forms, and other local web origins, not malicious software running as the user. The server binds only to loopback and rejects untrusted Host or mismatched Origin values. The Studio page sends `Content-Security-Policy: frame-ancestors 'none'` and `X-Frame-Options: DENY` to prevent clickjacking. Current endpoints:

- `GET /api/v1/health`, `/api/v1/runtime`, `/api/v1/operations`
- `GET /api/v1/assets?limit=50`; `POST /api/v1/assets?filename=scene.glb` with raw file bytes
- `GET /api/v1/assets/{asset_id}`; `POST /api/v1/assets/{asset_id}/simplify`
- `POST /api/v1/jobs`, `GET /api/v1/jobs/{job_id}`, `POST /api/v1/jobs/{job_id}/cancel`
- `GET /api/v1/artifacts/{artifact_id}`

Job requests support `mesh.simplify` and `image.generate` and return `202 Accepted`. The single operation slot serializes local work. Queued jobs can be cancelled; native simplification and model inference cannot currently be interrupted safely after starting. Job state is persisted, but automatic recovery of queued/running work after a process restart is not implemented. Image generation is only available when the manually provisioned MLX paths/checkpoints are present; the readiness check does not verify the full Python dependency environment or guarantee available memory.

## MCP

Run `meshharbor mcp` to start the stdio MCP server. Its output is the MCP protocol stream; diagnostics go to stderr. Available tools include `list_operations`, `list_assets`, `import_asset`, `start_simplification`, `start_image_generation`, `get_job`, `cancel_job`, and `runtime_doctor`. MCP asset import accepts base64-encoded local file bytes and does not fetch files or weights from the network.
