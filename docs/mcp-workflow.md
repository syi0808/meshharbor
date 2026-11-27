# MeshHarbor MCP workflow

Run `meshharbor serve` (or the background service), then start the stdio adapter with `meshharbor mcp`. Give the MCP process `MESHHARBOR_API_TOKEN` and, for a nondefault port, `MESHHARBOR_SERVER` (loopback only). Never put credentials in a tool argument. MCP talks to the same REST server and queue as Studio; it does not open another asset store or start its own scheduler.

| Tool | Capability |
| --- | --- |
| `import_engine_asset` | Upload PNG, JPEG, or GLB bytes as `filename` and `bytes_base64`. Use CLI `engine assets import` for large local files. |
| `engine_assets` | `page`, `show`, `export` exact artifact bytes. |
| `engine_models` | `list`, `versions`, `restore` the selected model version. |
| `generate_model` | Generate from typed `input` and backend `parameters`; see [generation inputs](rest-api.md#generation-inputs). |
| `engine_mesh` | `simplify`, `cleanup`, `voxel_remesh`, `unwrap_uv`, `optimize_rendering`, `compress`, `retopology`, `retopology_lato2`, `preflight`. |
| `engine_generation_queue` | `list` or `swap` adjacent waiting generation jobs. |
| `engine_jobs` | `list`, `show`, `steps`, `cancel`, `retry` an existing job. |
| `engine_runtime` | `health`, `status`, `operations`, `jobs`, `setup_environment`, `install`, `remove_model_files`, `quadriflow`, `install_quadriflow`. |
| `engine_connections` | `check_account`, `check_model_access`, `login_start`, `login_status`, `login_cancel`, `disconnect` for the `mlx` or `cuda` account scope. |
| `engine_blender` | `status`, `download_addon`, `send`, `transfer`. |

For grouped tools, pass `{"request":{"action":"…", ...}}`. For instance:

- Optimize a stored mesh: `engine_mesh` with `{"request":{"action":"simplify","asset_id":"asset_…","options":{"target_triangles":4000}}}`.
- Generate from a stored image: `generate_model` with `{"input":{"kind":"image","asset_id":"asset_…"},"parameters":{"backend_id":"auto"}}`.
- Poll either result: `engine_jobs` with `{"request":{"action":"show","job_id":"job_…"}}`. Submit work through the corresponding feature tool, not through Jobs.

For runtime setup, use `engine_runtime` with `setup_environment` and owner confirmation; then use `engine_connections` to start device sign-in. Read the one-time code with `login_status` and ask the owner to complete authorization in their browser. Install through `engine_runtime` with `action: "install"`, a catalog `backend`, `confirmed: true`, and applicable gated-access confirmation. The optional Pixal3D CUDA package uses `package: "multiview"`. For install retries use `confirm_download: true` only with the owner's approval.

### Exports and Blender

Find the output asset through the completed job's `result.asset_id`, then use `engine_assets` `show` to obtain an artifact ID. Use `export` with that `artifact_id` and a new absolute `output_path` inside `MESHHARBOR_EXPORT_DIR`. The default export root is `<MCP --data-dir>/exports`; it can differ from the engine's data directory. Files and symlinks are never overwritten. The limit is 512 MiB, and the resulting path is on the MCP host.

For Blender, the separate `install_blender_addon` tool requires `confirmed: true` from the owner, installs locally, and refuses to change Blender preferences while Blender is running. Alternatively save the add-on with `engine_blender` `download_addon`. After starting the bridge in Blender, use `send` with an `asset_id` and `transfer` with the returned `transfer_id`. The transfer does not overwrite the stored GLB or save the scene.
