# Licensing and dependency status

This is an implementation audit record, not legal advice or a final commercial-use approval. MeshHarbor's own project license has not been selected; there is no root `LICENSE`. The upstream MIT licenses do not grant a project-wide open-source license. Do not describe the full repository, model runtime, or future release as commercially cleared.

## Rust application dependencies

The current `Cargo.lock` resolves the Rust workspace dependency graph. A `cargo metadata --locked` inventory found no missing license expressions among third-party packages in the resolved metadata. Most packages state MIT, Apache-2.0, or both. Some require selection/preservation of their terms: `foldhash` is Zlib; `matchit` is MIT AND BSD-3-Clause; `ryu` allows Apache-2.0 OR BSL-1.0; `r-efi` offers MIT/Apache-2.0 OR LGPL-2.1-or-later. MeshHarbor application crates have no selected project license. A complete release SBOM, target-specific binary notices, and legal review are pending.

The mesh simplifier uses `meshopt` 0.7.0 (`MIT OR Apache-2.0`), which wraps/builds upstream meshoptimizer (`MIT`). The dependency graph is in `Cargo.lock`; it includes no model weights.

## Vendored Pixal3D source snapshots

The two source repositories were copied as pinned, file-by-file snapshots, not reduced to helper-only subsets:

- CUDA `syi0808/pixal3d-cuda` at `a4cb2b226cdb2bf3328eb280d4a02e510cc78e8c`: 56 of 59 Git blobs preserved; 3 generated historical evidence files omitted.
- MLX `syi0808/pixal3d-mlx` at `3a832ebf5f346a7eafb1ba40ca2926cfbd0bb2d6`: 150 of 189 Git blobs preserved; 37 image/GLB assets and 2 generated witness records omitted because asset rights/provenance are unverified.

Every preserved local file was compared against its pinned upstream Git blob hash. Paths, hashes, sizes, and reasons for all exclusions are recorded in [`pixal3d-source-inventory.json`](pixal3d-source-inventory.json). Each source root retains the full upstream MIT `LICENSE` naming Noah Lyons. That license does not automatically clear every credited or derived component. The referenced projects' license copies and source-attribution notes are in [`runtimes/pixal3d/THIRD-PARTY-NOTICES.md`](../runtimes/pixal3d/THIRD-PARTY-NOTICES.md) and `runtimes/pixal3d/THIRD-PARTY-LICENSES/`.

Known third-party conditions remain distinct from the port repositories' MIT notices:

- DINOv3-related source is accompanied by Meta's custom DINO Materials Agreement. It is not a standard MIT license; its use/distribution conditions and gated weight access require separate review.
- NAF-related code is accompanied by Apache-2.0 terms; the commercial inference scope for the exact NAF release checkpoint remains unresolved upstream.
- MoGe code is accompanied by the upstream MIT/Apache-2.0 license file. The CUDA source pins a MoGe model revision separately.
- TRELLIS.2-related code retains Microsoft's MIT notice. Other credited MIT port licenses are also preserved, without claiming all source lineage has been legally adjudicated.
- The MLX tree includes source for a CGAL bridge, but not CGAL headers, library, or binary. Building/distributing an executable that uses CGAL needs a separate GPL-3.0-or-later or commercial licensing decision.

The CUDA source tree remains a dormant archive. The MLX source snapshot and first-party managed adapters are embedded into the Rust executable and extracted under the app data directory when runtime setup is explicitly run; setup provisions Python/dependencies with user-installed `uv`, and the separate model-install action can download checkpoints after gated access is arranged. These flows are implemented but were not exercised on Apple Silicon in this environment. No weights/checkpoints, user assets, generated images/meshes, credentials, or Python wheels are bundled. No CUDA compatibility, successful MLX setup, or inference quality is claimed.

Before enabling inference, resolve at least:

- DINOv3 license and gated weight access, and the applicable Tencent Pixal3D model/checkpoint terms.
- NAF checkpoint commercial-use scope (upstream issue #13 remains open).
- MLX `rembg` selection: the lock uses `rembg` 2.0.76/U²-Net, while an unpinned install may choose 2.0.87 with BRIA RMBG-2.0 default (CC BY-NC absent a commercial agreement); pin the package and select an allowed model explicitly.
- CGAL linked-binary obligations, the complete native/Python package dependency license closure, and the exact OS/accelerator support matrix.
- Any generated web bundle: `web/vendor.mjs` is source only; if run, its generated `@google/model-viewer` bundle is Apache-2.0 and needs corresponding notices. No bundle is present here.

## Atlas UV baking research

The old experimental atlas baker is not copied or used in the product path. Its reviewed dependency closure includes meshoptimizer v1.3 (MIT), xatlas (MIT plus an embedded BSD-style notice and other attribution), TinyBVH v1.9.1 (MIT with its 2024–2026 Jacco Bikker/Breda University attribution), and stb (MIT/public-domain notices). Any future integration must preserve all applicable notices and pass broader texture and target-renderer QA. The current simplifier retains UVs and textures, so rebaking is not needed. See [`source-provenance.md`](source-provenance.md).

## Scope of use today

The validated Rust processing slice imports and inspects supported local assets and simplifies a constrained set of GLBs with meshoptimizer. That path performs no model inference, external network access, or atlas texture rebake. The separate MLX runtime actions can download dependencies and model files only when explicitly requested, but were not run here. This limited validation does not establish commercial clearance for the whole application or an inference backend.
