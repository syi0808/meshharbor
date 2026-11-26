# MeshHarbor CLI

The examples use the built `meshharbor` executable on `PATH`. From a source checkout, substitute `cargo run --locked --bin meshharbor --` or use the executable in `target/debug` after building.

## Control the running engine

Use `meshharbor engine` to operate on the same server and job queue as Studio, even when its browser tab is closed. Start `meshharbor serve` or the background service first, then set `MESHHARBOR_API_TOKEN` in the terminal environment to the server's bearer token. Set `MESHHARBOR_SERVER` or pass `--server 127.0.0.1:PORT` for a nondefault loopback address. The default is `127.0.0.1:7432`. `engine runtime health` does not need a token; all other `engine` commands do. The running server selects the data directory for these commands.

```sh
meshharbor engine assets page --models-only --query castle --limit 20
meshharbor engine assets show "$ASSET_ID"
meshharbor engine assets versions "$MODEL_ID"
meshharbor engine assets restore "$MODEL_ID" "$ASSET_ID"
meshharbor engine assets export "$ARTIFACT_ID" ./existing-version.glb

meshharbor engine jobs submit model.generate --input '{"kind":"image","asset_id":"asset_REPLACE_ME"}' \
  --parameters '{"backend_id":"pixal3d-mlx","tuning":{"ss_steps":16}}'
meshharbor engine jobs list --include-active
meshharbor engine jobs steps "$JOB_ID"
meshharbor engine jobs queue
meshharbor engine jobs swap "$JOB_ID" "$OTHER_JOB_ID"
meshharbor engine jobs cancel "$JOB_ID" --queued-only
meshharbor engine jobs retry "$JOB_ID" --confirmed
```

`assets page` also accepts `--cursor`, `--kind`, and `--derived true|false`. `jobs list --before JOB_ID` loads older terminal records. `jobs submit` returns immediately and accepts the same input and parameters as the [REST job API](rest-api.md#job-history-and-execution), including [posed multiview generation](rest-api.md#generation-inputs). Existing `generate`, `simplify`, and `process-mesh` commands still run directly; use `engine jobs submit` when the job must share Studio's queue.

```sh
meshharbor engine runtime status
meshharbor engine runtime jobs pixal3d-mlx
meshharbor engine runtime setup-environment --confirmed
meshharbor engine runtime install pixal3d-mlx --confirmed --confirm-gated-access
meshharbor engine runtime remove-model-files pixal3d-mlx --confirmed
meshharbor engine runtime install-multiview --confirm-large-download
meshharbor engine runtime check-account pixal3d-mlx
meshharbor engine runtime check-model-access pixal3d-mlx
meshharbor engine runtime login-start pixal3d-mlx
meshharbor engine runtime login-status pixal3d-mlx "$SESSION_ID"
meshharbor engine runtime login-cancel pixal3d-mlx "$SESSION_ID"
meshharbor engine runtime disconnect pixal3d-mlx --confirmed
meshharbor engine blender status
meshharbor engine blender download-addon ./meshharbor_bridge.py
```

Use `login-status` to read the one-time code, enter it yourself on the Hugging Face device authorization page, and check again for `signed_in`. Do not paste Hugging Face tokens into command lines; the existing interactive `runtime hf-login`/`cuda-hf-login` commands remain available for token entry.

## Background service

With a native binary on macOS, Linux with systemd, or Windows, you can run Studio and its REST API under your account without an open terminal. Use a binary in a permanent location, for example one installed with `cargo install --locked --path crates/meshharbor --bin meshharbor`. Stop any foreground server or container using the same port before starting the service. Run without `sudo` or Administrator elevation.

```sh
meshharbor service install   # Register automatic startup; does not start it now
meshharbor service start
meshharbor service status    # Exits nonzero if the service is not running
curl --fail http://127.0.0.1:7432/api/v1/health
meshharbor service stop      # Stop now, but keep automatic startup enabled
meshharbor service uninstall # Stop and remove the service; keep assets, jobs and models
```

`start` asks the OS to launch the service; check `/api/v1/health` for API readiness (use `curl.exe` in Windows PowerShell). The OS can restart the service if it exits unexpectedly. This does not change how stdio MCP is launched by its client.

- **macOS:** The `io.meshharbor.serve` LaunchAgent starts when you log in to your desktop session, not before login. Logs are in `~/Library/Logs/MeshHarbor/serve.log`.
- **Linux:** The `meshharbor.service` systemd user unit starts with your user manager. To start at boot and stay running even after you log out, enable lingering for your account with `loginctl enable-linger "$USER"` (subject to system policy). Read logs with `journalctl --user -u meshharbor.service`.
- **Windows:** Windows PowerShell and Task Scheduler register a task for the current account. It starts at login, not before login, and saves no account password. After a failure, Task Scheduler can retry up to 10 times at one-minute intervals. Logs are at `%LOCALAPPDATA%\MeshHarbor\service\serve.log`.

By default, the service uses the same data directory as the CLI and listens only on `127.0.0.1:7432`. To change either setting, uninstall the old registration first, then run for example `meshharbor --data-dir /path/to/data service install --listen 127.0.0.1:7440`. The service saves the absolute paths of the binary and data directory. If you move the binary, reinstall the service.

The service uses the OS service manager's environment, not the invoking shell's `PATH` or exported variables. Studio does not need an API token. For direct REST automation, see [REST API authentication](rest-api.md#start-the-server); if no token is configured in the service environment, a temporary token appears in the service logs on each start. Do not share those logs.

## Uninstall MeshHarbor

Run as the same user who installed MeshHarbor, from outside its data directory.
Back up any models or assets you want to keep, and close any foreground
`serve` process, MCP client, or other MeshHarbor command. The uninstaller
stops and unregisters the background service for this executable and data
directory, if installed.

```sh
meshharbor uninstall           # Preview only
meshharbor uninstall --confirm # Permanently delete the listed installation and data
```

Use the same `--data-dir` (before `uninstall`) or `MESHHARBOR_HOME` you used to
run MeshHarbor. The command removes that one dedicated data directory,
including stored assets, job history, managed models and runtimes, isolated
credentials, and any exports in its `exports` folder. It refuses an
unrecognized directory or unexpected top-level files. Move such files out and
preview again. On macOS and Windows it also removes the dedicated
`serve.log`; Linux's shared systemd journal is left alone.

The command removes only the executable you ran; on Windows a helper deletes
it after exit. For a Cargo installation, use
`meshharbor uninstall --confirm --keep-binary` followed by
`cargo uninstall meshharbor` so Cargo also updates its install records.

Model or export directories outside the selected data directory, shared caches
referenced by symlinks, the source checkout, the Blender add-on, MCP client
configuration, and browser site data are not removed. Clean up those
integrations in their respective apps if needed. For Docker, stop the
container and remove the image and the `meshharbor-data` volume separately
if you also want to delete its data.

## Mesh processing

```sh
# Import a GLB, then set ASSET_ID to the asset ID printed by the import.
meshharbor import ./scene.glb
meshharbor assets --limit 20
ASSET_ID=asset_REPLACE_ME

# Read persisted job history without resuming or modifying jobs
meshharbor jobs list --limit 20
meshharbor jobs show job_REPLACE_ME

# Simplify an imported asset; the source stays intact
meshharbor simplify "$ASSET_ID" --target-triangles 4000 --max-error 0.02 --output ./scene-simplified.glb

# Conservative cleanup, atlas UV rebake, render optimization, or required KHR_meshopt compression
meshharbor process-mesh "$ASSET_ID" --operation mesh.cleanup --output ./scene-cleaned.glb
# Remove duplicates for retopology without simplifying the mesh.
meshharbor process-mesh "$ASSET_ID" --operation mesh.cleanup --for-retopology
# Rebuild a closed geometry-only surface (grid resolution 16–160, default 96)
meshharbor process-mesh "$ASSET_ID" --operation mesh.voxel_remesh --voxel-resolution 96 --output ./scene-voxel.glb
meshharbor process-mesh "$ASSET_ID" --operation uv.unwrap --output ./scene-atlas.glb
# Adjust atlas resolution (64–4,096; default 1,024) and chart padding (0–64; default 8)
meshharbor process-mesh "$ASSET_ID" --operation uv.unwrap --resolution 2048 --padding 12 --output ./scene-atlas-hd.glb
meshharbor process-mesh "$ASSET_ID" --operation render.optimize --output ./scene-render-optimized.glb
# Optional overdraw threshold (1.0–1.5; default 1.05)
meshharbor process-mesh "$ASSET_ID" --operation render.optimize --overdraw-threshold 1.2 --output ./scene-render-tuned.glb
meshharbor process-mesh "$ASSET_ID" --operation mesh.compress --output ./scene-compressed.glb
```

Mesh processing targets supported static indexed triangle GLBs and creates a new asset. Simplification fails without creating an asset if it cannot honor the triangle limit. Compressed GLBs require a viewer that supports `KHR_meshopt_compression`; they do not include an uncompressed geometry fallback. See [architecture and operation limits](architecture.md#simplification-and-uvbaking-boundary).

## Send to Blender

With Blender 4.2+ closed, run `meshharbor blender install` to install and enable
the bundled add-on for the next Blender launch. Installation needs neither a
server nor a token. If Blender is running, quit it and retry; its settings are
not changed. In Blender, press **N** in the 3D Viewport and choose
**MeshHarbor → Start Bridge**.

To transfer an asset, run `meshharbor serve` on the same computer. The Studio
browser does not need to be open. Set `MESHHARBOR_API_TOKEN` in the sending
process to the server's bearer token (the configured value or the temporary token
printed by the server). Use the asset ID in that server's data directory:

```sh
meshharbor blender send "$ASSET_ID"
meshharbor blender send "$ASSET_ID" --no-wait
meshharbor blender transfer-status "$TRANSFER_ID"
```

By default, `send` waits for Blender to confirm the import. With `--no-wait`,
it returns an ID for `transfer-status`. For a nondefault listen address, pass
`--server 127.0.0.1:PORT` or set `MESHHARBOR_BLENDER_SERVER` for both installation
and transfer. Only loopback IP addresses are accepted (default `127.0.0.1:7432`).
Standard and MeshHarbor meshopt-compressed GLBs use the same bridge. The source
GLB and existing scene file are not overwritten.

## Image-to-3D

Run `meshharbor doctor` to check local backend readiness. Follow the [Pixal3D MLX](pixal3d-mlx-manual-runtime.md) or [CUDA](pixal3d-cuda-managed-runtime.md) guide for setup and gated model downloads. After setup, import your image and substitute its asset ID:

```sh
meshharbor import ./reference.png
IMAGE_ASSET_ID=asset_REPLACE_ME
meshharbor generate "$IMAGE_ASSET_ID" --backend-id pixal3d-mlx --output ./generated.glb
```

Use `pixal3d-cuda` for the Linux/NVIDIA profile.

Use `--tuning '{"ss_steps":16}'` for catalog settings supported by the selected backend. The CUDA profile also accepts `--view-input` with a JSON object containing the ordered pose references; the positional image asset ID must match the first reference. See the [input format and platform requirements](rest-api.md#generation-inputs).

TRELLIS.2 and TripoSG also expose experimental Apple MLX and NVIDIA CUDA profiles through CLI, REST, MCP, and Studio. They require [manual provisioning and authorization](external-generation-runtimes.md); only first-party adapters are included. TRELLIS.2 produces textured GLB, while TripoSG produces geometry-only GLB. Neither model has been qualified with real GPU inference in MeshHarbor.

## Text-to-3D

After manually provisioning TRELLIS Text Large on [Apple Silicon](trellis-text-mlx.md) or [Linux/NVIDIA](trellis-text-cuda.md), pass a prompt instead of an image asset ID. The experimental profiles output a geometry-only GLB. With `auto`, Apple Silicon uses `trellis-text-mlx` and Linux/NVIDIA uses `trellis-text-cuda`.

```sh
meshharbor generate --prompt "A ceramic cup" --output ./cup.glb
```

## File and output limits

- Import and inspection accept GLBs up to 512 MiB and PNG/JPEG images up to 16 MiB and 16 megapixels. Import checks image structure and dimensions but does not decode pixels. Generation verifies stored image sizes and hashes before starting a worker. External image URIs in GLBs are not fetched.
- Create parent directories before exporting. `--output` and `--quad-output` reject existing files and symlinks. A failed write may leave a partial new file; inspect or remove it before retrying.
- [QuadriFlow retopology](retopology-candidate.md) is unavailable in this build. When an approved sidecar is available, its preview GLB and editable quad OBJ must use distinct paths (case-only aliases are rejected on Windows).

## Local data and recovery

The app stores source assets, managed Python environments, and isolated Hugging Face caches under its local data directory. By default that is `%LOCALAPPDATA%\MeshHarbor` on Windows (falling back to `%USERPROFILE%\AppData\Local\MeshHarbor`), `~/Library/Application Support/MeshHarbor` on macOS, and `$XDG_DATA_HOME/meshharbor` or `~/.local/share/meshharbor` on Linux. Empty environment-variable values are ignored. Set `MESHHARBOR_HOME` to override the location. Use the same `--data-dir` or `MESHHARBOR_HOME` value for `serve` and the corresponding `*-hf-login` command. Review and accept any model-host terms in your own account before downloading. CUDA is limited to Linux x86_64/NVIDIA, requests roughly 25–30 GB of weights, uses a fixed camera and does not install MoGe/rembg. MLX retains its separate conversion step and profile. See [Studio settings](../apps/studio/README.md#settings) to set up a supported profile in the browser.

MeshHarbor keeps managed downloads and links to reusable tools and model files inside its data directory. After stopping MeshHarbor, removing that directory would remove its private downloads and links without deleting the linked system tools or shared cache files. Back up any assets or job history you want to keep first. This does not remove a native service registration or manually configured runtimes outside the data directory.

All processes using the same data directory coordinate through OS advisory locks. REST and MCP may run together. Each service cancels its queued and running jobs on graceful exit. At startup it marks unfinished jobs from previous, no-longer-running services as failed, leaving other live services' jobs alone. Pixal3D runtime setup, downloads, Hugging Face login, and conversion take an exclusive runtime lock, while image generation takes a compatible shared lock. This prevents those MeshHarbor actions from racing over managed Python environments or Hugging Face caches. If an incompatible operation is already active, the new operation fails fast with a retry message. The operating system releases locks when a process exits or crashes, and separate data directories do not block one another. This coordinates MeshHarbor processes only; it is not a lock on unrelated software that accesses the same files. After force-killing MeshHarbor during a managed uv/Python command, verify the spawned uv/Python child has exited before retrying; the OS releases the parent process's lock, and orphaned child supervision is not yet implemented.

To force manual recovery instead of restarting a service, stop all REST and MCP processes that use this data directory before running:

```sh
meshharbor jobs recover-interrupted --confirm-no-running-meshharbor
```

The command marks interrupted jobs as failed; it does not resume work or remove assets or model files.
