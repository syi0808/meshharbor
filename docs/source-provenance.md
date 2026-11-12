# Source provenance

## MeshHarbor core

`crates/core` and `crates/meshharbor` are the Rust product-core implementation created for this project. GLB simplification uses `meshopt` 0.7.0, pinned in `Cargo.lock`, and the upstream meshoptimizer library. MeshHarbor's own license remains undecided; no root `LICENSE` has been added.

## Pixal3D backend snapshots

The full source trees of `syi0808/pixal3d-cuda` and `syi0808/pixal3d-mlx` are kept in `runtimes/pixal3d/{cuda,mlx}/upstream/` at pinned revisions. Each retained file is byte-checked against its Git blob SHA. The JSON inventory [`pixal3d-source-inventory.json`](pixal3d-source-inventory.json) records both pinned trees, all copied file hashes, and every omission. The complete upstream MIT license files remain alongside each tree. Since upstream READMEs credit multiple other projects and some portions are based on third-party work, their exact license notices are preserved separately in [`../runtimes/pixal3d/THIRD-PARTY-NOTICES.md`](../runtimes/pixal3d/THIRD-PARTY-NOTICES.md) and `../runtimes/pixal3d/THIRD-PARTY-LICENSES/`. This source preservation is not a blanket commercial-use or project-license determination.

The omitted files are limited to generated test/witness records and binary images/3D assets for which individual rights/provenance were not established. The source code, tests, lockfiles, scripts, documentation, notebooks, deployment source, and nested workflow source are preserved. No model weights, CGAL dependencies, generated bundle, or credentials are included. MeshHarbor leaves both upstream trees byte-identical. The CUDA tree remains source-only; a separate first-party adapter under `runtimes/pixal3d/mlx/managed/` and core code provisions the MLX runtime while keeping the pinned upstream tree unchanged. Optional CGAL source is not built or linked.

## Experimental atlas UV baker research

Reviewed `syi0808/glb-mesh-refiner` at `35383ddab8bbe491b98264b42892c42684c7d514`, including `docs/standalone-baker.md` and its texture-baking tests. No `glb-mesh-refiner` implementation is included in MeshHarbor. The atlas baker is not copied or built. Its dependency closure includes meshoptimizer v1.3, xatlas, TinyBVH v1.9.1, and stb; no source from those copies has been brought over. Any future integration must preserve their exact upstream notices, including xatlas's embedded BSD-style notice and TinyBVH's 2024–2026 Jacco Bikker/Breda University notice. Its material and validation limits are documented in [`architecture.md`](architecture.md) and [`licensing-status.md`](licensing-status.md).

The current simplifier uses meshoptimizer and retains the source vertex streams, UVs, material references, and embedded textures. Since it does not generate a new UV atlas, no texture rebake is part of the current path. If a future operation changes UV layout, atlas unwrap/baking should remain a separate explicit capability until seam, material, visual-quality and license validation are complete.
