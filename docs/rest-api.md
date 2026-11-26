# MeshHarbor REST API

## Start the server

Start the [built executable](../README.md#quick-start) on loopback (non-loopback binds are rejected):

```sh
meshharbor serve --listen 127.0.0.1:7432
```

Check the server with `curl http://127.0.0.1:7432/api/v1/health`. The health endpoint needs no token. For non-Studio clients, send `Authorization: Bearer <token>` on other API calls. When `MESHHARBOR_API_TOKEN` is unset, the engine generates a temporary token and prints it to stderr. A configured token must not be empty or whitespace-only, and its value is not written to logs.

The built-in Studio connects without a token using a dashboard marker and exact same-origin browser provenance. Tokenless writes need an exact same-origin `Origin`; safe reads may use browser `Sec-Fetch-Site: same-origin` when no `Origin` is supplied. Certain runtime connection and model-management routes are Studio-only. Non-browser bearer clients can omit `Origin`, but a supplied origin must still match the loopback host. Cross-origin requests are rejected even with a bearer token.

Do not expose the server through a public proxy. These browser-origin checks are not authentication against processes running as the same OS user. See the [security policy](../SECURITY.md) for the local trust boundary.

## Endpoints

Selected endpoints:

- `GET /api/v1/health`, `/api/v1/runtime`, `/api/v1/operations`
- Studio-only: `POST /api/v1/runtime/environment/setup` with `{"confirmed":true}` queues shared uv, Python 3.10/3.12, and Hugging Face tool setup without installing any model. Check `environment.ready` in runtime status.
- `POST /api/v1/runtime/pixal3d-mlx/setup`, `/models`, `/convert-naf`; `/api/v1/runtime/pixal3d-cuda/setup` and `/models` for explicit asynchronous setup actions
- `GET /api/v1/runtime/{backend}/jobs` returns the six most recent setup or install jobs for that backend. The runtime status includes model file paths.
- Studio-only: `POST /api/v1/runtime/{backend}/install` with `confirmed: true` queues a managed backend-and-weights job for Pixal3D, GeoSAM2, TRELLIS.2, TripoSG, or TRELLIS Text. The shared environment and saved Hugging Face credentials must be present before queuing; the worker verifies sign-in before provisioning or downloads. Pixal3D and TRELLIS.2 also require DINOv3 access and `confirm_gated_access: true`. In runtime status, `model_catalog[].category` distinguishes `repart` from `generation`. GeoSAM2 files do not enable a Studio repart operation.
- Studio-only: `DELETE /api/v1/runtime/{backend}/model-files` with `confirmed: true` removes managed model files. For either GeoSAM2 backend this removes the one shared checkpoint and preserves both environments.
- `GET /api/v1/assets?limit=50`; `POST /api/v1/assets?filename=scene.glb` with raw file bytes
- `GET /api/v1/assets/page?limit=50&cursor=<asset_id>&q=<filename>&kind=image|mesh&derived=true|false` returns `{items,next_cursor}` newest-first; filters are optional, and `next_cursor` can be sent back to load older assets
- `GET /api/v1/assets/{asset_id}`; `POST /api/v1/assets/{asset_id}/simplify`
- `GET /api/v1/assets/page?models_only=true` lists each mesh model once, returning its current asset; search, origin filters, and cursors use the original model record
- `GET /api/v1/models/{model_id}/versions?limit=20&before=<version_number>` returns numbered versions, their parent version, and the current asset ID; `PUT /api/v1/models/{model_id}/current` with `{"asset_id":"..."}` restores a version of that model
- `GET /api/v1/jobs?limit=20` (recent persisted jobs; read-only), `POST /api/v1/jobs`, `GET /api/v1/jobs/{job_id}`, `POST /api/v1/jobs/{job_id}/cancel`
- `GET /api/v1/jobs/{job_id}/steps` (persisted execution history and command logs)
- `POST /api/v1/jobs/{job_id}/retry` (start a new attempt from a replayable failed or stopped job)
- `GET /api/v1/artifacts/{artifact_id}`

## Model versions

A mesh derived from another mesh is an immutable version of the same model. A GLB import or the result of 3D generation starts a new model. A new edit advances the current version only if it was based on the current version; an edit from an older version creates a branch without replacing the current one. Restoring changes the current pointer, not any stored file. Existing data is grouped by its recorded source lineage on first open; the latest saved version becomes current. The existing asset endpoints, CLI, MCP, and job results still address individual assets.

## Generation inputs

Use `POST /api/v1/jobs` with `operation: "model.generate"` and a tagged input:

```json
{
  "operation": "model.generate",
  "input": { "kind": "image", "asset_id": "asset_example" },
  "parameters": { "backend_id": "pixal3d-mlx" }
}
```

`GET /api/v1/runtime` exposes each backend's `generation.input_modes`, `parameters`, `view_modes`, and `max_views`. Pick a ready backend and a supported input mode. For TRELLIS Text Large, send `input: { "kind": "text", "prompt": "..." }` and set `parameters.backend_id` to [`trellis-text-mlx`](trellis-text-mlx.md) or [`trellis-text-cuda`](trellis-text-cuda.md). The prompt must be nonblank, at most 4,096 UTF-8 bytes, and contain no NUL characters. With `auto`, image input selects platform Pixal3D and text selects platform TRELLIS Text. An unready external backend or unsupported input mode returns HTTP 422 without creating a job. Legacy `image.generate` with `input: { "asset_id": "..." }` continues to work and shares the same generation queue.

Keep base fields such as `seed` at the top level of job parameters and put catalog fields marked `tuning: true` under `parameters.tuning`. For a ready multiview backend, send `parameters.view_input` with `mode` (`four`, `six`, or `custom`) and an ordered `references` array. Each entry contains an imported image's `asset_id`; the first must match `input.asset_id`. Presets use front/right/back/left order (then top/bottom for six). Custom entries each add a `camera` object with `azimuth_degrees`, `elevation_degrees`, `distance`, and `fov_degrees`. No client filesystem paths are accepted. The CUDA profile exposes `multiview_ready` once the separately approved snapshot is installed.

## Job history and execution

Job submissions return `202 Accepted` with `state: "running"` and `started_at` set if the engine can start them immediately. Only work that waits for the engine slot starts in `queued`.

`GET /api/v1/jobs/{job_id}/steps` returns state changes and execution stages in order. Each entry contains `id`, `state`, `stage`, `at`, `message`, and `commands`. Each command entry contains its argument vector (`argv`), an explicit working directory (`cwd`) when set, timestamps, an `outcome` when available, and the available `stdout`/`stderr` tails. Logs are limited to the most recent 16 KiB per stream before redaction; `truncated` indicates earlier output was dropped. Environment variables, stdin, and sign-in traffic are not recorded. The response is not cached or embedded in the job list. Jobs predating this feature can have no recorded steps. Logs can still contain local paths and other diagnostic information; review them before sharing.

`GET /api/v1/jobs?limit=20` lists the newest updated persisted job records (default 50, capped at 200). Studio polls with `include_active=true` to list all active jobs plus up to the requested number of terminal records. Use `before=<terminal_job_id>` to page through older terminal records. New jobs include a `started_at` timestamp once running; generation jobs also report the latest known worker `stage` when available. The CLI exposes job history through `jobs list` and `jobs show <job_id>`; MCP exposes `list_jobs` and `get_job`. These queries are read-only: listing or checking a saved job never resumes or restarts work.

`GET /api/v1/jobs/generation-queue` returns the IDs of waiting generation jobs in execution order. `POST` to the same endpoint with `job_id` and `swap_with` to swap two adjacent generation jobs; other operations retain their positions. A job that has started or was left behind by an interrupted engine cannot be reordered. Use `POST /api/v1/jobs/{job_id}/cancel?queued_only=true` to remove a waiting job without stopping it if it has just started.

Job requests support `mesh.simplify`, `mesh.cleanup`, `mesh.voxel_remesh`, `uv.unwrap`, `render.optimize`, `mesh.compress`, conditional `mesh.retopology`, `model.generate`, legacy `image.generate`, and explicit runtime jobs. The retired `mesh.separate_parts` operation cannot be submitted or retried; its recorded jobs and assets remain readable.

`mesh.simplify` requires a positive `target_triangles` lower than the input triangle count. This is the maximum for the entire output GLB; if it cannot be reached within `max_error` and border constraints, the job fails without a derived asset. By default, `mesh.cleanup` removes invalid and same-winding duplicate faces, then attempts conservative, target-free simplification and reports its additional removals as `simplified_triangles_removed`. With `"for_retopology": true`, it skips simplification and also removes same-index reversed duplicates when all three edges have a single neighboring face that agrees on the winding to keep.

`mesh.voxel_remesh` accepts optional integer `resolution` (16–160, default 96) without any QuadriFlow backend. It reads one static indexed triangle primitive under an untransformed mesh node or parent chain in a GLB up to 64 MiB, with up to 100,000 vertices and 200,000 triangles. It verifies the reconstructed mesh with the same geometry validator as QuadriFlow preflight. The derived asset contains only geometry; UVs, materials, and textures are not transferred. The source remains unchanged. Validation does not certify self-intersection freedom or exact shape preservation.

`uv.unwrap` accepts an optional `resolution` hint (64–4096, default 1024) and `padding` (0–64, default 8); Studio uses the defaults. `mesh.retopology` requires an [opt-in linked build](retopology-candidate.md#linked-development-build) or a signature-verified, manifest-matched managed `quadriflow` sidecar. Adjacent executables are ignored. Omit `target_quads` for automatic density or supply an approximate count (4–50,000). Reports return `requested_quads: null` for automatic density. The job produces both a geometry-only `quad_topology` OBJ artifact and a triangulated GLB preview. When the supported embedded PNG maps are present, only base-color and metallic/roughness textures are projected into that preview with distance/facing/coverage/ambiguity gates. `render.optimize` accepts `overdraw_threshold` (1.0–1.5, default 1.05). Generic high-to-low texture baking is not available. Model-install endpoints require explicit large-download and gated-access confirmation. Generation requires a ready backend and local model files; readiness does not prove quality or general GPU-memory fit.

`POST /api/v1/jobs/{job_id}/cancel` stops queued jobs immediately. Running jobs enter `cancelling` until the worker exits, then become `cancelled`. QuadriFlow, generation, and managed runtime commands terminate their worker (process group on Unix); other mesh processing stops at the next safe checkpoint. An asset already being committed cannot be cancelled. Runtime caches and partial downloads are retained.

REST and MCP request cancellation of their own queued and running jobs on graceful shutdown and wait for the active slot to drain. On startup they mark unfinished jobs from dead service owners as `failed` with `error.code: "interrupted"` and close any unfinished command log entries. Other live services' records are left alone. Legacy records with no owner are recovered on a cold start without another service.

`POST /api/v1/jobs/{job_id}/retry` accepts a failed or cancelled job with replayable inputs and returns `202 Accepted` with a new job ID, using the original asset and parameters or the previously confirmed runtime operation. It does not carry over results, errors, or logs. Text prompts are not stored, so text generation must be submitted again with a prompt. Runtime retry can download model files; confirm that any child process orphaned by a force-kill has exited before retrying.

## Hugging Face status

The REST server checks saved Hugging Face credentials and gated DINOv3 file access asynchronously at startup and periodically thereafter. `GET /api/v1/runtime` returns the latest `model_backends[].huggingface.account` and `model_access` states for the Pixal3D profiles without contacting Hugging Face on each read. Invalid credentials and an unavailable check are distinct from verified access. The same-origin Studio recheck and sign-in endpoints update this snapshot, and installs still validate gated access when needed.
