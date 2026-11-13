# Source provenance

## MeshHarbor core

`crates/core` and `crates/meshharbor` are the Rust product-core implementation created for this project. GLB simplification uses `meshopt` 0.7.0, pinned in `Cargo.lock`, and the upstream meshoptimizer library. MeshHarbor's own license remains undecided; no root `LICENSE` has been added.

## Pixal3D backend snapshots

The full source trees of `syi0808/pixal3d-cuda` and `syi0808/pixal3d-mlx` are kept in `runtimes/pixal3d/{cuda,mlx}/upstream/` at pinned revisions. Each retained file is byte-checked against its Git blob SHA. The JSON inventory [`pixal3d-source-inventory.json`](pixal3d-source-inventory.json) records both pinned trees, all copied file hashes, and every omission. The complete upstream MIT license files remain alongside each tree. Since upstream READMEs credit multiple other projects and some portions are based on third-party work, their exact license notices are preserved separately in [`../runtimes/pixal3d/THIRD-PARTY-NOTICES.md`](../runtimes/pixal3d/THIRD-PARTY-NOTICES.md) and `../runtimes/pixal3d/THIRD-PARTY-LICENSES/`. This source preservation is not a blanket commercial-use or project-license determination.

The omitted files are limited to generated test/witness records and binary images/3D assets for which individual rights/provenance were not established. The source code, tests, lockfiles, scripts, documentation, notebooks, deployment source, and nested workflow source are preserved. No model weights, CGAL dependencies, generated bundle, or credentials are included. MeshHarbor leaves both upstream trees byte-identical. The CUDA tree remains source-only; a separate first-party adapter under `runtimes/pixal3d/mlx/managed/` and core code provisions the MLX runtime while keeping the pinned upstream tree unchanged. Optional CGAL source is not built or linked.

## Atlas UV integration

Reviewed `syi0808/glb-mesh-refiner` at `35383ddab8bbe491b98264b42892c42684c7d514`, including `docs/standalone-baker.md` and its texture-baking tests. No experimental `glb-mesh-refiner` implementation is included in MeshHarbor. The separate `crates/atlas-baker` integration uses xatlas at `f700c7790aaa030e794b52ba7791a05c085faf0c` and stb_image/stb_image_write at `4e9b368bfc071e97206ea5c6adba68dd0cbf8897`; it does not copy TinyBVH, the refiner executable, or an extra meshoptimizer source tree. Full xatlas attribution and MIT license, and the full stb dual-license notices, are retained in `crates/xatlas-sys/THIRD_PARTY_NOTICES.md` and their vendored source files.

The integrated atlas operation is intentionally separate from simplification. It currently accepts one static primitive/material with embedded PNG base-color and metallic/roughness textures and rebakes those existing maps from the mesh's prior UVs. It does not project from a high-poly source or handle arbitrary material maps. The current synthetic tests are not a broad visual-quality or real-asset acceptance suite; see [`architecture.md`](architecture.md) and [`licensing-status.md`](licensing-status.md) for limits and dependency status.

