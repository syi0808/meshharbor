# MeshHarbor MCP workflow

MeshHarbor's stdio MCP server uses the same `OperationCore`, asset store, job
queue, and Pixal3D MLX runtime as the CLI and localhost REST API.

## Prepare and generate

1. Start the MeshHarbor MCP server with `meshharbor mcp`.
2. Use `import_asset` with a PNG/JPEG image. MCP imports are inline base64, so
   for large inputs prefer the authenticated localhost REST upload or the CLI
   `meshharbor import <path>` command.
3. Use `runtime_doctor` to inspect local capability and readiness. The managed Pixal3D MLX installer is supported only on Apple Silicon macOS; a cross-platform `uv` asset table does not imply CUDA or Windows/Linux inference support.
4. Use `setup_pixal3d_mlx_runtime` to queue the isolated environment setup.
   This bootstraps the pinned uv executable when needed, then provisions the
   locked Python environment. Poll the returned job with `get_job`.
5. Hugging Face authentication is intentionally not accepted through MCP
   arguments or environment forwarding. Run `meshharbor runtime hf-login` in a
   local terminal to authenticate inside MeshHarbor's isolated cache. The user
   must obtain/accept gated DINOv3 access before continuing.
6. Only after the owner has confirmed the large model download and gated access,
   call `install_pixal3d_mlx_models` with both confirmation fields set to true.
   This queues an approximately 24 GB download. Poll with `get_job`.
7. Call `convert_pixal3d_mlx_naf` and poll its job, then call
   `start_image_generation` with the imported `asset_id`.
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

The MCP orchestration and local export path are code-integrated, but the full
Pixal3D inference chain has not been demonstrated end to end. It requires an
Apple Silicon Mac, MLX dependencies, downloaded model files, and the owner's
gated DINOv3 access. In this executor, Rust compilation and tests were not run
because `cargo`, `rustc`, and `rustfmt` were unavailable. Run
`cargo test --workspace --locked` on a permitted Rust development environment
before treating this patch as build-verified.
