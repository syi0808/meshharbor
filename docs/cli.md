# MeshHarbor CLI

The examples use the built `meshharbor` executable on `PATH`. From a source checkout, substitute `cargo run --locked --bin meshharbor --` or use the executable in `target/debug` after building.

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

# Conservative cleanup, connected-part primitives, atlas UV rebake, render optimization, or required KHR_meshopt compression
meshharbor process-mesh "$ASSET_ID" --operation mesh.cleanup --output ./scene-cleaned.glb
meshharbor process-mesh "$ASSET_ID" --operation mesh.separate_parts --output ./scene-parts.glb
meshharbor process-mesh "$ASSET_ID" --operation uv.unwrap --output ./scene-atlas.glb
# Adjust atlas resolution (64–4,096; default 1,024) and chart padding (0–64; default 8)
meshharbor process-mesh "$ASSET_ID" --operation uv.unwrap --resolution 2048 --padding 12 --output ./scene-atlas-hd.glb
meshharbor process-mesh "$ASSET_ID" --operation render.optimize --output ./scene-render-optimized.glb
# Optional overdraw threshold (1.0–1.5; default 1.05)
meshharbor process-mesh "$ASSET_ID" --operation render.optimize --overdraw-threshold 1.2 --output ./scene-render-tuned.glb
meshharbor process-mesh "$ASSET_ID" --operation mesh.compress --output ./scene-compressed.glb
```

Mesh processing targets supported static indexed triangle GLBs and creates a new asset. The simplification target is a goal; check the report for the actual count and error. Part separation accepts up to 64 MiB of GLB data, 4 MiB of JSON, 1,000,000 vertices, and 3,000,000 indices. It keeps the largest 255 components separate when more than 256 are found and groups the remainder without discarding triangles. Compressed GLBs require a viewer that supports `KHR_meshopt_compression`; they do not include an uncompressed geometry fallback. See [architecture and operation limits](architecture.md#simplification-and-uvbaking-boundary).

## Image-to-3D

Run `meshharbor doctor` to check local backend readiness. Follow the [Pixal3D MLX](pixal3d-mlx-manual-runtime.md) or [CUDA](pixal3d-cuda-managed-runtime.md) guide for setup and gated model downloads. After setup, import your image and substitute its asset ID:

```sh
meshharbor import ./reference.png
IMAGE_ASSET_ID=asset_REPLACE_ME
meshharbor generate "$IMAGE_ASSET_ID" --backend-id pixal3d-mlx --output ./generated.glb
```

Use `pixal3d-cuda` for the Linux/NVIDIA profile.

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

All processes using the same data directory coordinate through OS advisory locks. REST and MCP may run together. Each service cancels its queued and running jobs on graceful exit. At startup it marks unfinished jobs from previous, no-longer-running services as failed, leaving other live services' jobs alone. Pixal3D runtime setup, downloads, Hugging Face login, and conversion take an exclusive runtime lock, while image generation takes a compatible shared lock. This prevents those MeshHarbor actions from racing over managed Python environments or Hugging Face caches. If an incompatible operation is already active, the new operation fails fast with a retry message. The operating system releases locks when a process exits or crashes, and separate data directories do not block one another. This coordinates MeshHarbor processes only; it is not a lock on unrelated software that accesses the same files. After force-killing MeshHarbor during a managed uv/Python command, verify the spawned uv/Python child has exited before retrying; the OS releases the parent process's lock, and orphaned child supervision is not yet implemented.

To force manual recovery instead of restarting a service, stop all REST and MCP processes that use this data directory before running:

```sh
meshharbor jobs recover-interrupted --confirm-no-running-meshharbor
```

The command marks interrupted jobs as failed; it does not resume work or remove assets or model files.
