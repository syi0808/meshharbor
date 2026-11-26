# MeshHarbor MCP workflow

MeshHarbor's stdio MCP server uses the same `OperationCore`, asset store, job
queue, and generation runtimes as the CLI and localhost REST API. The
server runs as `meshharbor mcp`; stdout is reserved for the protocol and
diagnostics go to stderr. For mesh-only work, start with `list_operations`,
`list_assets` or `import_asset`, then use `start_simplification` or
`start_mesh_processing` and poll `get_job`. No GPU runtime is required.

## Use the same engine and queue as Studio

For workflows shared with Studio or CLI `engine`, run `meshharbor serve` (or the background service), and give the MCP process its bearer token through `MESHHARBOR_API_TOKEN`. Optionally set `MESHHARBOR_SERVER` to a nondefault loopback IP and port; the default is `127.0.0.1:7432`. Use the same data directory for `serve` and `mcp` when combining connected and standalone tools. Never put secrets in tool arguments.

| Tool | `request.action` values |
| --- | --- |
| `engine_assets` | `page`, `show`, `versions`, `restore`, `export` |
| `engine_jobs` | `list`, `show`, `steps`, `submit`, `retry`, `cancel`, `queue`, `swap` |
| `engine_runtime` | `health`, `status`, `operations`, `jobs`, `setup_environment`, `install`, `remove_model_files`, `setup_pixal3d`, `install_pixal3d_models`, `convert_naf`, `install_multiview`, `check_account`, `check_model_access`, `login_start`, `login_status`, `login_cancel`, `disconnect` |
| `engine_blender` | `status`, `download_addon` |

Pass fields alongside `action` inside `request`. For example, `engine_assets` with `{"request":{"action":"versions","model_id":"asset_…"}}` lists version history, and `engine_jobs` with `{"request":{"action":"cancel","job_id":"job_…","queued_only":true}}` removes a waiting job without stopping it if it has started. Use `engine_jobs` `submit` with REST `operation`, `input`, and `parameters` to submit into Studio's queue; existing `start_*` tools still run in MCP's standalone queue. Retrying requires owner approval (`confirmed: true`) because runtime retries can download model files. Use `engine_assets` `export` with an `artifact_id` and `output_path` for exact artifact bytes. The asset and Blender download actions use the same local export directory and no-overwrite rules as the standalone export tools.

For Hugging Face, use `login_start`, then `login_status` with the returned `session` and the same `backend`. Let the owner enter the one-time code in their browser and check the session again. The tool never receives a Hugging Face token. The terminal and Studio sign-in flows remain available.

The managed setup below is for Pixal3D. For TRELLIS.2 or TripoSG, follow the
[manual runtime guide](external-generation-runtimes.md) locally, then pass
its explicit backend ID to `start_model_generation`. For text generation,
prepare TRELLIS Text for [Apple MLX](trellis-text-mlx.md) or [NVIDIA CUDA](trellis-text-cuda.md), then call
`start_model_generation` with `input: { "kind": "text", "prompt": "A ceramic cup" }`
and set `parameters.backend_id` to `trellis-text-mlx` or `trellis-text-cuda`; no image import is needed.

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
5. Hugging Face tokens are intentionally not accepted through MCP
   arguments or environment forwarding. Use the browser device flow above, or run `meshharbor runtime hf-login` for
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
   call `start_model_generation` with `input: { "kind": "image", "asset_id": "..." }`
   and an optional `parameters.backend_id` (`auto`, `pixal3d-mlx`, or `pixal3d-cuda`).
   Check `runtime_doctor` for each backend's `generation.input_modes`. The
   compatibility tool `start_image_generation` still accepts the original flat
   image arguments.
8. Poll `get_job` until the generation job succeeds. Its result contains the
   derived mesh `asset_id`.
9. Call `export_glb` with that mesh asset ID and an absolute `.glb` output path
   on the computer running the local MCP server.

## Send to Blender

1. With Blender 4.2+ closed, ask the owner to approve installing and enabling the
   add-on. Call `install_blender_addon` with `{"confirmed":true}`. It installs
   on the MCP server host, requires no REST server or token, and does not
   change Blender's files or preferences if Blender is running.
2. Open Blender and choose **N → MeshHarbor → Start Bridge** in the 3D Viewport.
   Start `meshharbor serve` on the same computer, using the same data directory
   as `meshharbor mcp`. No Studio browser tab is needed.
3. Set `MESHHARBOR_API_TOKEN` in the MCP process environment to the REST server's
   bearer token, not in a tool argument. If the server listens on a nondefault
   port, set `MESHHARBOR_BLENDER_SERVER` to its loopback address before starting
   the MCP process, including before installing the add-on. The default is
   `127.0.0.1:7432`.
4. Call `send_to_blender` with `{"asset_id":"..."}` and pass the returned ID to
   `get_blender_transfer` as `{"transfer_id":"..."}`. A `succeeded` result
   confirms the selected version was added to the current scene; the scene
   file and stored GLB remain unchanged. Meshopt-compressed GLBs are supported.

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

The [local MLX validation run](pixal3d-mlx-manual-runtime.md#validation-status)
verified the shared store and export path: a stdio MCP client used `get_job` for
a real Pixal3D job started in Studio and `export_glb` returned the same GLB bytes
as Studio. It did not separately start inference through MCP or test a fresh
model installation. CUDA still requires a Linux x86_64/NVIDIA host, its CUDA
preflight, model files, and gated DINOv3 access.
