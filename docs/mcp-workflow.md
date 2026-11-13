# MeshHarbor MCP workflow

MeshHarbor's stdio MCP server uses the same `OperationCore`, asset store, job
queue, and Pixal3D MLX/CUDA runtimes as the CLI and localhost REST API.

## Prepare and generate

1. Start the MeshHarbor MCP server with `meshharbor mcp`.
2. Use `import_asset` with a PNG/JPEG image. MCP imports are inline base64, so
   for large inputs prefer the authenticated localhost REST upload or the CLI
   `meshharbor import <path>` command.
3. Use `runtime_doctor` to inspect local capability and readiness. MLX setup is
   Apple Silicon/macOS only; CUDA setup is Linux x86_64/NVIDIA only.
4. Choose `setup_pixal3d_mlx_runtime` or `setup_pixal3d_cuda_runtime`. Both
   bootstrap pinned uv if needed. CUDA setup also resolves upstream's loose
   dependency ranges into a private per-install lock and runs the upstream
   PyTorch CUDA/SDPA smoke test. It does not download model weights. Poll the
   returned job with `get_job`.
5. Hugging Face authentication is intentionally not accepted through MCP
   arguments or environment forwarding. Run `meshharbor runtime hf-login` for
   MLX or `meshharbor runtime cuda-hf-login` for CUDA in a local terminal. The
   credential stays in a per-backend isolated cache. The user must obtain and
   accept gated DINOv3 access before continuing.
6. Only after the owner has confirmed the large model download and gated access,
   call `install_pixal3d_mlx_models` or `install_pixal3d_cuda_models` with both
   confirmation fields set to true. MLX downloads about 24 GB; CUDA downloads
   roughly 25–30 GB and requests about 40 GiB free disk space. Each action
   checks existing gated DINOv3 access before fetching checkpoints. Poll with
   `get_job`.
7. For MLX, call `convert_pixal3d_mlx_naf` and poll its job. For either backend,
   call `start_image_generation` with the imported `asset_id` and optional
   `backend_id` (`auto`, `pixal3d-mlx`, or `pixal3d-cuda`).
8. Poll `get_job` until `image.generate` succeeds. Its result contains the
   derived mesh `asset_id`.
9. Call `export_glb` with that mesh asset ID and an absolute `.glb` output path
   on the computer running the local MCP server.

## Export safeguards

- `export_glb` accepts only stored mesh assets whose artifact is a binary GLB.
- The destination parent directory must be inside `MESHHARBOR_EXPORT_DIR`; by
  default this is `<MeshHarbor data directory>/exports`. The export root itself
  is created automatically; if you specify a nested folder, create that folder
  first.
- The canonical-parent check rejects traversal and existing symlink aliases that
  resolve outside the export root. It is not a sandbox against a hostile local
  process running as the same OS account and racing filesystem changes; the MCP
  server shares that account's local trust boundary. The owner may set
  `MESHHARBOR_EXPORT_DIR` to an intentional export folder before starting it.
- The path must be absolute and end in `.glb` (case-insensitive).
- Exports are limited to 512 MiB.
- The destination is created exclusively. If a file or symlink already exists
  at that name, export fails without overwriting it.
- The returned path is on the MCP server host, not necessarily the MCP client.

The MCP server is a local process running with the OS account's permissions.
An MCP client should only offer export to an owner-selected location. The tool
does not accept credentials or transmit generated files over the network.

## Current verification boundary

The orchestration and local export path are code-integrated, but neither Pixal3D
inference chain has been demonstrated end to end. MLX requires Apple Silicon,
downloaded model files, and the owner's gated DINOv3 access. CUDA requires Linux
x86_64/NVIDIA, its CUDA preflight, model files, and gated DINOv3 access. In this
executor, Rust compilation and GPU inference were not run because `cargo`,
`rustc`, `rustfmt`, and a CUDA device were unavailable. Run
`cargo test --workspace --locked` on a permitted Rust development environment
before treating this patch as build-verified.
