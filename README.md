# MeshHarbor

[![MIT license for first-party code](https://img.shields.io/badge/first--party%20license-MIT-blue)](LICENSE)
[![Embedded Studio smoke](https://github.com/syi0808/meshharbor/actions/workflows/embedded-studio-package-smoke.yml/badge.svg)](https://github.com/syi0808/meshharbor/actions/workflows/embedded-studio-package-smoke.yml)

**Generate 3D assets locally with open-weight models.**

MeshHarbor is a local-first alternative to [Tripo](https://www.tripo3d.ai/) and [Meshy](https://www.meshy.ai/) for 3D asset generation. After you set up a runtime and download its weights, generation runs on your hardware instead of sending inputs to a hosted service.

- **Generate with local models.** Run Pixal3D from images or TRELLIS Text Large from prompts on Apple Silicon (MLX) and Linux with NVIDIA (CUDA). The experimental manual backends also include TRELLIS.2 and TripoSG for image input.
- **Refine and export GLBs.** Inspect, simplify, clean, split connected parts, or compress meshes. Compare revisions and export without overwriting the original. Supported textured meshes can get a new UV atlas with a same-mesh rebake.
- **Work in Studio or automate locally.** Use the browser workbench, CLI, loopback REST API, or stdio MCP through the same Rust engine.

![MeshHarbor Studio with the generation panel open and the Surveyor rover in the 3D viewport](docs/images/studio-workbench.png)

*The Studio generation screen with an imported example GLB.*

**Status:** Early source build, not feature parity with hosted services. No official installer, binary release, or bundled model weights. Some checkpoints require gated access and have separate usage terms. Mesh editing needs no Python or model setup. Generic high-to-low baking and QuadriFlow retopology are not available.

## Quick start

### From source

Install Rust/Cargo 1.88 or later and a C/C++ toolchain. The checkout includes the built Studio client, so you don't need Node.js, pnpm, or a separate frontend build to run it. See the [build and smoke guide](docs/embedded-studio-package-smoke.md) for additional Windows build requirements.

Clone the repository and start the local server:

```sh
git clone https://github.com/syi0808/meshharbor.git
cd meshharbor
cargo run --locked --bin meshharbor -- serve --listen 127.0.0.1:7432
```

Open **http://127.0.0.1:7432**. The server is single-user and loopback-only; do not expose it through a public proxy.

- **Generate a model:** Set up Pixal3D for images ([MLX](docs/pixal3d-mlx-manual-runtime.md) or [CUDA](docs/pixal3d-cuda-managed-runtime.md)) or TRELLIS Text Large for prompts ([MLX](docs/trellis-text-mlx.md) or [CUDA](docs/trellis-text-cuda.md)). Once the backend is ready, open **New model**, provide its input, and click **Generate 3D**.
- **Try Studio without a model download:** Select **Inventory → 3D models → Open GLB**. Use **Optimize**, compare **Original** and **Result**, then **Export GLB**.

For the CLI, run `cargo run --locked --bin meshharbor -- --help`.

### Docker

From the repository root, build and start MeshHarbor:

```sh
docker build -t meshharbor .
docker run --rm --init --network host --name meshharbor -v meshharbor-data:/data meshharbor
```

Open **http://127.0.0.1:7432**. The named volume keeps imported assets, jobs, and settings when the container is removed. Stop the container with `docker stop meshharbor`.

MeshHarbor still listens only on loopback, so use [host networking](https://docs.docker.com/engine/network/drivers/host/) rather than `-p`. On Docker Desktop 4.34 or later, enable **Settings → Resources → Network → Enable host networking**, then apply and restart. Do not expose Studio through a public proxy.

The image includes Studio, the API, and CPU mesh editing. It does not include GPU generation runtimes or model weights; use the native setup guides for MLX or CUDA generation.

## Documentation

| I want to… | Start here |
| --- | --- |
| Use Studio, version history, or job controls | [Studio guide](apps/studio/README.md) |
| Process assets from a terminal | [CLI guide](docs/cli.md) |
| Integrate with the local API or an MCP client | [REST API](docs/rest-api.md) · [MCP workflow](docs/mcp-workflow.md) |
| Set up a generation model | [Pixal3D MLX](docs/pixal3d-mlx-manual-runtime.md) · [Pixal3D CUDA](docs/pixal3d-cuda-managed-runtime.md) · [TRELLIS.2 and TripoSG](docs/external-generation-runtimes.md) · TRELLIS Text [MLX](docs/trellis-text-mlx.md) / [CUDA](docs/trellis-text-cuda.md) |
| Understand format limits or work on the project | [Architecture](docs/architecture.md) · [Documentation index](docs/README.md) · [Contributing](CONTRIBUTING.md) |

## Contributing

[Issues and benchmarks](https://github.com/syi0808/meshharbor/issues) are welcome. Include the revision, platform, and a minimal shareable fixture when possible. Use the [contribution guide](CONTRIBUTING.md) for development checks and the [security policy](SECURITY.md) for private vulnerability reports.

## License

MeshHarbor's first-party code and documentation are licensed under [MIT](LICENSE). Vendored code, bundled Studio dependencies, fonts, and model/runtime components retain their own terms. See [third-party notices](THIRD-PARTY-NOTICES.md) and the [licensing status](docs/licensing-status.md) before redistribution; this is not a blanket license for all components or model weights.
