# MeshHarbor

[![MIT license for first-party code](https://img.shields.io/badge/first--party%20license-MIT-blue)](LICENSE)
[![Embedded Studio smoke](https://github.com/syi0808/meshharbor/actions/workflows/embedded-studio-package-smoke.yml/badge.svg)](https://github.com/syi0808/meshharbor/actions/workflows/embedded-studio-package-smoke.yml)

MeshHarbor is a local workbench for inspecting, editing, and exporting 3D assets. Use the browser-based Studio for interactive work, or drive the same Rust engine through the CLI, a loopback REST API, or stdio MCP.

- **Inspect and edit GLB files.** Preview models, simplify meshes, clean degenerate faces, or separate connected parts. Compare before and after, revisit earlier versions, and export without overwriting the original.
- **Prepare assets for rendering.** Reorder geometry, export meshopt-compressed GLBs, or rebuild a UV atlas and rebake supported embedded textures from the same mesh.
- **Try local image-to-3D.** Pixal3D has opt-in Apple Silicon (MLX) and Linux/NVIDIA (CUDA) profiles. TRELLIS.2 and TripoSG require manually provisioned runtimes. Model weights are not bundled.

![MeshHarbor Studio with the generation panel open and the Surveyor rover in the 3D viewport](docs/images/studio-workbench.png)

*The Studio generation screen with an imported example GLB.*

**Status:** Early source build. There is no official installer or binary release. Mesh editing works without Python, model weights, or a Hugging Face account. Mesh operations support specific static GLB layouts; generic high-to-low baking is not implemented, and the experimental QuadriFlow operation is unavailable because there is no approved sidecar.

## Quick start

Install Rust/Cargo 1.88 or later, a C/C++ toolchain, and Node.js 24 for Studio. See the [build and smoke guide](docs/embedded-studio-package-smoke.md) for pinned CI versions and additional Windows build requirements.

Clone the repository, build Studio, then start the local server:

```sh
git clone https://github.com/syi0808/meshharbor.git
cd meshharbor
cd apps/studio
npm ci --ignore-scripts --no-audit --no-fund
npm run build
cd ../..
cargo run --locked --bin meshharbor -- serve --listen 127.0.0.1:7432
```

Open **http://127.0.0.1:7432**, choose **Inventory → 3D models → Open GLB**, and select a local static GLB. Try **Optimize**, compare **Original** and **Result**, then use **Export GLB**. The default server is single-user and loopback-only; do not expose it through a public proxy.

The Studio bundle is embedded at Rust build time. For the CLI alone, skip the npm steps and run `cargo run --locked --bin meshharbor -- --help`.

## Documentation

| I want to… | Start here |
| --- | --- |
| Use Studio, version history, or job controls | [Studio guide](apps/studio/README.md) |
| Process assets from a terminal | [CLI guide](docs/cli.md) |
| Integrate with the local API or an MCP client | [REST API](docs/rest-api.md) · [MCP workflow](docs/mcp-workflow.md) |
| Configure image-to-3D | [Pixal3D MLX](docs/pixal3d-mlx-manual-runtime.md) · [Pixal3D CUDA](docs/pixal3d-cuda-managed-runtime.md) · [TRELLIS.2 and TripoSG](docs/external-generation-runtimes.md) |
| Understand format limits or work on the project | [Architecture](docs/architecture.md) · [Documentation index](docs/README.md) · [Contributing](CONTRIBUTING.md) |

## Contributing

[Issues and benchmarks](https://github.com/syi0808/meshharbor/issues) are welcome. Include the revision, platform, and a minimal shareable fixture when possible. Use the [contribution guide](CONTRIBUTING.md) for development checks and the [security policy](SECURITY.md) for private vulnerability reports.

## License

MeshHarbor's first-party code and documentation are licensed under [MIT](LICENSE). Vendored code, bundled Studio dependencies, fonts, and model/runtime components retain their own terms. See [third-party notices](THIRD-PARTY-NOTICES.md) and the [licensing status](docs/licensing-status.md) before redistribution; this is not a blanket license for all components or model weights.
