# MeshHarbor REST API

## Start the server

Start the [built executable](../README.md#quick-start) on loopback (non-loopback binds are rejected):

```sh
meshharbor serve --listen 127.0.0.1:7432
```

Open [Swagger UI](http://127.0.0.1:7432/docs) to browse the API, or download the [OpenAPI JSON](http://127.0.0.1:7432/openapi.json). Both are served locally and work offline without a token. To try a protected GET in Swagger UI, enter the API token under **Authorize**. Use a REST client for write requests.

Check the server with `curl http://127.0.0.1:7432/api/v1/health`. The health endpoint needs no token. For non-Studio clients, send `Authorization: Bearer <token>` on other API calls. When `MESHHARBOR_API_TOKEN` is unset, the engine generates a temporary token and prints it to stderr. A configured token must not be empty or whitespace-only, and its value is not written to logs.

The built-in Studio connects without a token using a dashboard marker and exact same-origin browser provenance. Tokenless writes need an exact same-origin `Origin`; safe reads may use browser `Sec-Fetch-Site: same-origin` when no `Origin` is supplied. Non-browser bearer clients can omit `Origin`, but a supplied origin must still match the loopback host. Cross-origin requests are rejected even with a bearer token. The HTTP endpoints for entering a raw Hugging Face token and installing Blender locally remain Studio-only. CLI and MCP have a browser device flow and their own local Blender installer.

Do not expose the server through a public proxy. These browser-origin checks are not authentication against processes running as the same OS user. See the [security policy](../SECURITY.md) for the local trust boundary.

## Endpoints

Paths in the tables are relative to `/api/v1`. Submit new work through its feature endpoint. An asynchronous submission returns `202 Accepted` and a job record; follow that record through `GET /jobs/{job_id}`. There is no `POST /jobs`.

| Feature | Endpoints |
| --- | --- |
| Library | `POST /assets?filename=scene.glb` uploads raw file bytes. `GET /assets` pages saved images and individual GLB versions using optional `limit`, `cursor`, `q`, `kind`, and `derived` filters. `GET /assets/{asset_id}` reads a saved version. `GET /artifacts/{artifact_id}` downloads exact bytes. |
| 3D models | `GET /models` pages one current asset per model using optional `limit`, `cursor`, `q`, and `generated` filters. `GET /models/{model_id}/versions` lists versions and the current ID. `PUT /models/{model_id}/current` selects a saved version with `{"asset_id":"..."}`. |
| Generation | `POST /generation` accepts tagged image or text input. Read or reorder waiting generation jobs at `/generation/queue`. |
| Jobs | `GET /jobs`, `GET /jobs/{job_id}`, `GET /jobs/{job_id}/steps`, `POST /jobs/{job_id}/cancel`, and `POST /jobs/{job_id}/retry` manage existing records only. |
| Runtime | `GET /runtime` lists the installed environment and model catalog. `POST /runtime/environment/setup` prepares shared Python, uv, and account tools. `POST /runtime/{backend}/install` installs a catalog backend or an optional package. `GET /runtime/{backend}/jobs` returns the six newest provisioning jobs; `DELETE /runtime/{backend}/model-files` removes managed files. QuadriFlow status and guarded install are at `GET /runtime/quadriflow` and `POST /runtime/quadriflow/install`. |
| Blender | `GET /blender` checks the connection; `GET /blender/addon` downloads the add-on. `POST /blender/send` accepts `{"asset_id":"..."}` and returns a transfer ID for `GET /blender/send/{id}`. `POST /blender/install` is Studio-only and requires Blender to be closed. Native companion routes are not public API. |

Asset and model pages return `{items,next_cursor}` newest-first. Model filters and cursors are based on the original model record. Version pages accept `limit` and `before` (a version number).

Runtime setup, installation, and model-file removal require `confirmed: true`. A base install sets up the selected backend and weights in one job. Pixal3D and TRELLIS.2 also require DINOv3 access and `confirm_gated_access: true`. Backends advertise extra packages in `optional_packages`. For the additional Pixal3D CUDA weights use `{"confirmed":true,"package":"multiview"}` on the same install endpoint. Installation requires the shared environment and saved Hugging Face credentials, which are verified before provisioning. Matching pinned files are reused from the user's HF cache; removing managed files leaves that cache untouched. Installing GeoSAM2 does not enable a Studio Repart action.

## Mesh editing

Send JSON directly to the chosen `POST /assets/{asset_id}/…` endpoint. Successful submissions return a job ID. `retopology-preflight` returns an eligibility report immediately; it does not queue a job.

| Suffix | JSON body |
| --- | --- |
| `simplify` | `{"target_triangles":4000,"max_error":0.02}` |
| `cleanup` | `{}` or `{"for_retopology":true}` |
| `voxel-remesh` | `{}` or `{"resolution":96}` |
| `unwrap-uv` | `{}` or `{"resolution":1024,"padding":8}` |
| `optimize-rendering` | `{}` or `{"overdraw_threshold":1.05}` |
| `compress` | `{}` |
| `retopology`, `retopology-preflight` | `{}` for automatic density, or `{"target_quads":1000}` |
| `retopology-lato2` | `{}` or `{"backend_id":"lato2-mlx","target_triangles":4000,"atlas_resolution":1024}` |

The retired connected-part split has no submit or retry API. Its saved assets and jobs are still readable.

## Model versions

A mesh derived from another mesh is an immutable version of the same model. A GLB import or the result of 3D generation starts a new model. A new edit advances the current version only if it was based on the current version; an edit from an older version creates a branch without replacing the current one. Restoring changes the current pointer, not any stored file. Existing data is grouped by its recorded source lineage on first open; the latest saved version becomes current. The existing asset endpoints, CLI, MCP, and job results still address individual assets.

## Generation inputs

Use `POST /api/v1/generation` with a tagged input:

```json
{
  "input": { "kind": "image", "asset_id": "asset_example" },
  "parameters": { "backend_id": "pixal3d-mlx" }
}
```

`GET /api/v1/runtime` exposes each backend's `generation.input_modes`, `parameters`, `view_modes`, and `max_views`. Pick a ready backend and a supported input mode. For TRELLIS Text Large, send `input: { "kind": "text", "prompt": "..." }` and set `parameters.backend_id` to [`trellis-text-mlx`](trellis-text-mlx.md) or [`trellis-text-cuda`](trellis-text-cuda.md). The prompt must be nonblank, at most 4,096 UTF-8 bytes, and contain no NUL characters. With `auto`, image input selects platform Pixal3D and text selects platform TRELLIS Text. An unready external backend or unsupported input mode returns HTTP 422 without creating a job.

Keep base fields such as `seed` at the top level of job parameters and put catalog fields marked `tuning: true` under `parameters.tuning`. For a ready multiview backend, send `parameters.view_input` with `mode` (`four`, `six`, or `custom`) and an ordered `references` array. Each entry contains an imported image's `asset_id`; the first must match `input.asset_id`. Presets use front/right/back/left order (then top/bottom for six). Custom entries each add a `camera` object with `azimuth_degrees`, `elevation_degrees`, `distance`, and `fov_degrees`. No client filesystem paths are accepted. The CUDA profile exposes `multiview_ready` once the separately approved snapshot is installed.

The [LATO.2 profiles](lato2-experiment.md#image-to-3d) accept one image with `backend_id: "lato2-cuda"` or `"lato2-mlx"` and an optional approximate `target_vertices` (200–5000, default 2000). They output a geometry-only GLB and do not accept text, multiview, or a texture-size override. Install either profile through the same confirmed model-install endpoint as other catalog models.

## Job history and execution

Job submissions return `202 Accepted` with `state: "running"` and `started_at` set if the engine can start them immediately. Only work that waits for the engine slot starts in `queued`.

`GET /api/v1/jobs/{job_id}/steps` returns state changes and execution stages in order. Each entry contains `id`, `state`, `stage`, `at`, `message`, and `commands`. Each command entry contains its argument vector (`argv`), an explicit working directory (`cwd`) when set, timestamps, an `outcome` when available, and the available `stdout`/`stderr` tails. Logs are limited to the most recent 16 KiB per stream before redaction; `truncated` indicates earlier output was dropped. Environment variables, stdin, and sign-in traffic are not recorded. The response is not cached or embedded in the job list. Jobs predating this feature can have no recorded steps. Logs can still contain local paths and other diagnostic information; review them before sharing.

`GET /api/v1/jobs?limit=20` lists the newest updated persisted job records (default 50, capped at 200). Studio polls with `include_active=true` to list all active jobs plus up to the requested number of terminal records. Use `before=<terminal_job_id>` to page through older terminal records. New jobs include a `started_at` timestamp once running; generation jobs also report the latest known worker `stage` when available. The CLI exposes job history through `engine jobs list` and `engine jobs show <job_id>`; MCP uses `engine_jobs`. These queries are read-only: listing or checking a saved job never resumes or restarts work.

`GET /api/v1/generation/queue` returns the IDs of waiting generation jobs in execution order. `POST` to the same endpoint with `job_id` and `swap_with` to swap two adjacent generation jobs; other operations retain their positions. A job that has started or was left behind by an interrupted engine cannot be reordered. Use `POST /api/v1/jobs/{job_id}/cancel?queued_only=true` to remove a waiting job without stopping it if it has just started.

`mesh.simplify` requires a positive `target_triangles` lower than the input triangle count. This is the maximum for the entire output GLB; if it cannot be reached within `max_error` and border constraints, the job fails without a derived asset. By default, `mesh.cleanup` removes invalid and same-winding duplicate faces, then attempts conservative, target-free simplification and reports its additional removals as `simplified_triangles_removed`. With `"for_retopology": true`, it skips simplification and also removes same-index reversed duplicates when all three edges have a single neighboring face that agrees on the winding to keep.

`mesh.voxel_remesh` accepts optional integer `resolution` (16–160, default 96) without any QuadriFlow backend. It reads one static indexed triangle primitive under an untransformed mesh node or parent chain in a GLB up to 64 MiB, with up to 100,000 vertices and 200,000 triangles. It verifies the reconstructed mesh with the same geometry validator as QuadriFlow preflight. The derived asset contains only geometry; UVs, materials, and textures are not transferred. The source remains unchanged. Validation does not certify self-intersection freedom or exact shape preservation.

`mesh.retopology_lato2` is a separate experimental job for [local LATO.2 retopology and native CPU texture rebaking](lato2-experiment.md). It accepts an approximate `target_triangles` (default 4000), not a maximum, and produces a textured triangle GLB without Blender or meshoptimizer. The target replaces the former `target_vertices` and `max_triangles` fields for this operation. If unconfigured the endpoint returns HTTP 422; if inference, topology checks, or baking fail, no derived asset is saved. It never falls back to `mesh.simplify`.

`uv.unwrap` accepts an optional `resolution` hint (64–4096, default 1024) and `padding` (0–64, default 8). `mesh.retopology` requires an [opt-in linked build](retopology-candidate.md#linked-development-build) or a signature-verified, manifest-matched managed `quadriflow` sidecar. Adjacent executables are ignored. Omit `target_quads` for automatic density or supply an approximate count (4–50,000). Reports return `requested_quads: null` for automatic density. The job produces both a geometry-only `quad_topology` OBJ artifact and a triangulated GLB preview. When the supported embedded PNG maps are present, only base-color and metallic/roughness textures are projected into that preview with distance/facing/coverage/ambiguity gates. `render.optimize` accepts `overdraw_threshold` (1.0–1.5, default 1.05). Generic high-to-low texture baking is not available. Model-install endpoints require explicit large-download and gated-access confirmation. Generation requires a ready backend and local model files; readiness does not prove quality or general GPU-memory fit.

`POST /api/v1/jobs/{job_id}/cancel` stops queued jobs immediately. Running jobs enter `cancelling` until the worker exits, then become `cancelled`. QuadriFlow, LATO.2 inference, generation, and managed runtime commands terminate their worker (process group on Unix); LATO.2 CPU rebaking and other in-process mesh work stop at the next safe checkpoint. An asset already being committed cannot be cancelled. Runtime caches and partial downloads are retained.

The REST server owns the queue for Studio, connected CLI, and MCP. On graceful shutdown it cancels queued and running jobs and waits for the active slot to drain. On startup it marks unfinished jobs from dead service owners as `failed` with `error.code: "interrupted"` and close any unfinished command log entries. Other live services' records are left alone. Legacy records with no owner are recovered on a cold start without another service.

`POST /api/v1/jobs/{job_id}/retry` accepts a failed or cancelled job with replayable inputs and returns `202 Accepted` with a new job ID, using the original asset and parameters or the previously confirmed runtime operation. It does not carry over results, errors, or logs. Text prompts are not stored, so text generation must be submitted again with a prompt. Send `{}` to retry ordinary jobs. A runtime install that can download weights requires `{"confirm_download":true}` and rechecks runtime prerequisites. Confirm that any child process orphaned by a force-kill has exited before retrying.

## Hugging Face status

The REST server checks saved Hugging Face credentials and gated DINOv3 file access asynchronously at startup and periodically thereafter. `GET /api/v1/runtime` returns the latest `model_backends[].huggingface.account` and `model_access` states for the Pixal3D profiles without contacting Hugging Face on each read. Invalid credentials and an unavailable check are distinct from verified access. Use the `mlx` or `cuda` account scope; the credentials apply to managed models using that accelerator.

- `POST /api/v1/connections/huggingface/{scope}/check` rechecks the saved account without a request body. `POST /api/v1/connections/huggingface/{scope}/model-access` rechecks gated access. Installs still validate gated access when needed.
- `POST /api/v1/connections/huggingface/{scope}/sessions` starts device sign-in. `GET` and `DELETE` on its `/{session}` subpath read or cancel the session. The owner enters the one-time code in their browser; no raw credential is returned.
- `DELETE /api/v1/connections/huggingface/{scope}` with `{"confirmed":true}` clears local credentials without revoking the remote token. `POST` to that path remains reserved for same-origin Studio token entry.
