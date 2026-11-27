# MeshHarbor CLI

The examples use the built `meshharbor` executable on `PATH`. From a source checkout, substitute `cargo run --locked --bin meshharbor --` or use the executable in `target/debug` after building.

## Control the running engine

Use `meshharbor engine` against the same server and queue as Studio. Start `meshharbor serve` or the background service and set `MESHHARBOR_API_TOKEN` to the server's bearer token. Override the default `127.0.0.1:7432` with `MESHHARBOR_SERVER` or `engine --server 127.0.0.1:PORT`. The server selects its own data directory; a Studio browser tab is not required.

```sh
meshharbor engine assets import ./scene.glb
meshharbor engine assets page --kind mesh --query castle
meshharbor engine assets show "$ASSET_ID"
meshharbor engine models list --query castle
meshharbor engine models versions "$MODEL_ID"
meshharbor engine models restore "$MODEL_ID" "$ASSET_ID"
meshharbor engine assets export "$ARTIFACT_ID" ./existing-version.glb

meshharbor engine jobs list --include-active
meshharbor engine jobs show "$JOB_ID"
meshharbor engine jobs steps "$JOB_ID"
meshharbor engine jobs cancel "$JOB_ID" --queued-only
meshharbor engine jobs retry "$JOB_ID"
```

Use `--confirm-download` only when approving the repeat of a model download. Jobs commands control previously submitted work; to start work, select the matching feature. Asset and model lists both support `--cursor`; `assets page` accepts `--derived true|false` and `models list` accepts `--generated true|false`.

```sh
meshharbor engine runtime status
meshharbor engine runtime setup-environment --confirmed
meshharbor engine connections login-start mlx
meshharbor engine connections login-status mlx "$SESSION_ID"
meshharbor engine connections check-account mlx
meshharbor engine connections check-model-access mlx
meshharbor engine runtime install pixal3d-mlx --confirmed --confirm-gated-access
meshharbor engine runtime install pixal3d-cuda --package multiview --confirmed
meshharbor engine runtime jobs pixal3d-mlx
meshharbor engine runtime remove-model-files pixal3d-mlx --confirmed
meshharbor engine connections disconnect mlx --confirmed
```

Use `cuda` instead of `mlx` for CUDA credentials. Complete device authorization in your browser, then check the session for `signed_in`. Do not put a Hugging Face token in command-line arguments.

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

All editing commands target a stored mesh asset ID and return a job record. Preflight returns a report immediately and does not start a job.

```sh
meshharbor engine mesh simplify "$ASSET_ID" --target-triangles 4000 --max-error 0.02
meshharbor engine mesh cleanup "$ASSET_ID" --for-retopology
meshharbor engine mesh voxel-remesh "$ASSET_ID" --resolution 96
meshharbor engine mesh unwrap-uv "$ASSET_ID" --resolution 1024 --padding 8
meshharbor engine mesh optimize-rendering "$ASSET_ID" --overdraw-threshold 1.05
meshharbor engine mesh compress "$ASSET_ID"
meshharbor engine mesh preflight "$ASSET_ID"
meshharbor engine mesh retopology "$ASSET_ID" --target-quads 1000
meshharbor engine mesh retopology-lato2 "$ASSET_ID" --backend-id lato2-mlx --target-triangles 4000
```

Use `engine jobs show` to find the result's asset ID and `engine assets export` to save its GLB or OBJ artifact. See [mesh editing limits](rest-api.md#mesh-editing) and [QuadriFlow readiness](retopology-candidate.md).

## Send to Blender

With Blender closed, run `meshharbor install-blender-addon` to edit local Blender preferences for the next launch. This needs neither a running server nor a token. For manual installation use `meshharbor engine blender download-addon ./meshharbor_bridge.py`. If you install for a nondefault port, use the same `MESHHARBOR_SERVER` or `--server` setting for the installer and engine commands.

In Blender, choose **N → MeshHarbor → Start Bridge**, then submit the selected version. Send returns immediately with a transfer ID; query it until Blender reports completion.

```sh
meshharbor engine blender status
meshharbor engine blender send "$ASSET_ID"
meshharbor engine blender transfer "$TRANSFER_ID"
```

## 3D generation

Use `engine runtime status` to choose a ready profile. See the [REST generation contract](rest-api.md#generation-inputs) for accepted per-profile tuning and multiview fields.

```sh
meshharbor engine assets import ./reference.png
meshharbor engine generation image "$IMAGE_ASSET_ID" --backend pixal3d-mlx
meshharbor engine generation text 'A ceramic cup' --backend trellis-text-mlx
meshharbor engine generation queue
meshharbor engine generation swap "$JOB_ID" "$OTHER_JOB_ID"
```

Pass `--parameters '{"tuning":{"structure_steps":16}}'` for catalog settings supported by the selected backend. A configured CUDA profile accepts `view_input` in the same parameters object.

## Offline commands

The explicit `meshharbor local` group works directly against `--data-dir` without a server. It is for offline batch or diagnostic use, not the Studio queue. For example, run `meshharbor local inspect ./scene.glb` or see `meshharbor local --help` for offline import, processing, and generation. Model provisioning and connected job control are under `engine`.

## File and output limits

- Import and inspection accept GLBs up to 512 MiB and PNG/JPEG images up to 16 MiB and 16 megapixels. Import checks image structure and dimensions but does not decode pixels. Generation verifies stored image sizes and hashes before starting a worker. External image URIs in GLBs are not fetched.
- Create parent directories before exporting. `--output` and `--quad-output` reject existing files and symlinks. A failed write may leave a partial new file; inspect or remove it before retrying.
- [QuadriFlow retopology](retopology-candidate.md) is unavailable in this build. When an approved sidecar is available, its preview GLB and editable quad OBJ must use distinct paths (case-only aliases are rejected on Windows).

## Local data and recovery

The app stores source assets, managed Python environments, and isolated Hugging Face caches under its local data directory. By default that is `%LOCALAPPDATA%\MeshHarbor` on Windows (falling back to `%USERPROFILE%\AppData\Local\MeshHarbor`), `~/Library/Application Support/MeshHarbor` on macOS, and `$XDG_DATA_HOME/meshharbor` or `~/.local/share/meshharbor` on Linux. Empty environment-variable values are ignored. Set `MESHHARBOR_HOME` to override the location. Use `--data-dir` or `MESHHARBOR_HOME` when starting the engine; connected commands work against that engine’s store. Review and accept any model-host terms in your own account before downloading. CUDA is limited to Linux x86_64/NVIDIA, requests roughly 25–30 GB of weights, uses a fixed camera and does not install MoGe/rembg. MLX retains its separate conversion step and profile. See [Studio settings](../apps/studio/README.md#settings) to set up a supported profile in the browser.

MeshHarbor keeps managed downloads and links to reusable tools and model files inside its data directory. After stopping MeshHarbor, removing that directory would remove its private downloads and links without deleting the linked system tools or shared cache files. Back up any assets or job history you want to keep first. This does not remove a native service registration or manually configured runtimes outside the data directory.

All processes using the same data directory coordinate through OS advisory locks. REST and MCP may run together. Each service cancels its queued and running jobs on graceful exit. At startup it marks unfinished jobs from previous, no-longer-running services as failed, leaving other live services' jobs alone. Pixal3D runtime setup, downloads, Hugging Face login, and conversion take an exclusive runtime lock, while image generation takes a compatible shared lock. This prevents those MeshHarbor actions from racing over managed Python environments or Hugging Face caches. If an incompatible operation is already active, the new operation fails fast with a retry message. The operating system releases locks when a process exits or crashes, and separate data directories do not block one another. This coordinates MeshHarbor processes only; it is not a lock on unrelated software that accesses the same files. After force-killing MeshHarbor during a managed uv/Python command, verify the spawned uv/Python child has exited before retrying; the OS releases the parent process's lock, and orphaned child supervision is not yet implemented.

To force manual recovery instead of restarting a service, stop all engine and offline processes that use this data directory before running:

```sh
meshharbor recover-jobs --confirm-no-running-meshharbor
```

The command marks interrupted jobs as failed; it does not resume work or remove assets or model files.
