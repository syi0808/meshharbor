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
- `POST /api/v1/runtime/pixal3d-mlx/setup`, `/models`, `/convert-naf`; `/api/v1/runtime/pixal3d-cuda/setup` and `/models` for explicit asynchronous setup actions
- `GET /api/v1/assets?limit=50`; `POST /api/v1/assets?filename=scene.glb` with raw file bytes
- `GET /api/v1/assets/page?limit=50&cursor=<asset_id>&q=<filename>&kind=image|mesh&derived=true|false` returns `{items,next_cursor}` newest-first; filters are optional, and `next_cursor` can be sent back to load older assets
- `GET /api/v1/assets/{asset_id}`; `POST /api/v1/assets/{asset_id}/simplify`
- `GET /api/v1/assets/page?models_only=true` lists each mesh model once, returning its current asset; search, origin filters, and cursors use the original model record
- `GET /api/v1/models/{model_id}/versions?limit=20&before=<version_number>` returns numbered versions, their parent version, and the current asset ID; `PUT /api/v1/models/{model_id}/current` with `{"asset_id":"..."}` restores a version of that model
- `GET /api/v1/jobs?limit=20` (recent persisted jobs; read-only), `POST /api/v1/jobs`, `GET /api/v1/jobs/{job_id}`, `POST /api/v1/jobs/{job_id}/cancel`
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

`GET /api/v1/runtime` exposes each backend's `generation.input_modes`, `parameters`, `view_modes`, and `max_views`. Pick a ready backend and a supported input mode. For [TRELLIS Text Large](trellis-text-cuda.md), send `input: { "kind": "text", "prompt": "..." }` and `parameters.backend_id: "trellis-text-cuda"`. The prompt must be nonblank, at most 4,096 UTF-8 bytes, and contain no NUL characters. With `auto`, image input selects platform Pixal3D and text selects TRELLIS Text CUDA. An unready external backend or unsupported input mode returns HTTP 422 without creating a job. Legacy `image.generate` with `input: { "asset_id": "..." }` continues to work and shares the same generation queue.

Keep base fields such as `seed` at the top level of job parameters and put catalog fields marked `tuning: true` under `parameters.tuning`. For a ready multiview backend, send `parameters.view_input` with `mode` (`four`, `six`, or `custom`) and an ordered `references` array. Each entry contains an imported image's `asset_id`; the first must match `input.asset_id`. Presets use front/right/back/left order (then top/bottom for six). Custom entries each add a `camera` object with `azimuth_degrees`, `elevation_degrees`, `distance`, and `fov_degrees`. No client filesystem paths are accepted. The CUDA profile exposes `multiview_ready` once the separately approved snapshot is installed.

## Job history and execution

`GET /api/v1/jobs?limit=20` lists the newest updated persisted job records (default 50, capped at 200). Studio polls with `include_active=true` to list all active jobs plus up to the requested number of terminal records. Use `before=<terminal_job_id>` to page through older terminal records. New jobs include a `started_at` timestamp once running; generation jobs also report the latest known worker `stage` when available. The CLI exposes job history through `jobs list` and `jobs show <job_id>`; MCP exposes `list_jobs` and `get_job`. These queries are read-only: listing or checking a saved job never resumes or restarts work. After an unexpected engine exit, a saved active state may be stale; use the explicit recovery command only after stopping every REST/MCP process that uses that data directory.

`GET /api/v1/jobs/generation-queue` returns the IDs of waiting generation jobs in execution order. `POST` to the same endpoint with `job_id` and `swap_with` to swap two adjacent generation jobs; other operations retain their positions. A job that has started or was left behind by an interrupted engine cannot be reordered. Use `POST /api/v1/jobs/{job_id}/cancel?queued_only=true` to remove a waiting job without stopping it if it has just started.

Job requests support `mesh.simplify`, `mesh.cleanup`, `mesh.separate_parts`, `uv.unwrap`, `render.optimize`, `mesh.compress`, conditional `mesh.retopology`, `model.generate`, legacy `image.generate`, and explicit runtime jobs. `uv.unwrap` accepts an optional `resolution` hint (64–4096, default 1024) and `padding` (0–64, default 8); Studio uses the defaults. `mesh.retopology` is rejected unless a signature-verified, manifest-matched managed `quadriflow` sidecar is available; adjacent executables are ignored and this build has no verified release; it accepts optional `target_quads` (4–50,000, default 10,000) and produces both a geometry-only `quad_topology` OBJ artifact and a triangulated GLB preview. When the supported embedded PNG maps are present, only base-color and metallic/roughness textures are projected into that preview with distance/facing/coverage/ambiguity gates. `render.optimize` accepts `overdraw_threshold` (1.0–1.5, default 1.05). Generic high-to-low texture baking is not available. Model-install endpoints require explicit large-download and gated-access confirmation. The single operation slot serializes local work. Queued jobs can be cancelled immediately. Running asset jobs can be permanently stopped before their result is committed: generation terminates its worker process group, and native mesh processing stops at the next safe checkpoint without storing a result. Running runtime setup and downloads are not interruptible. A stopped job cannot be resumed. Job state is persisted. After an unexpected exit, stop every REST/MCP process using the same data directory and run `meshharbor jobs recover-interrupted --confirm-no-running-meshharbor` to mark nonterminal jobs as failed. The command does not resume work or delete assets/download caches; inspect existing assets before retrying. The flag is an operator acknowledgement; MeshHarbor also refuses recovery while a REST or MCP service holds the data-directory lock. Generation requires a ready backend and local model files; readiness does not prove quality or general GPU-memory fit.
