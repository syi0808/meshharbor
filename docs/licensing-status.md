# Licensing and dependency status

This is an implementation audit record, not legal advice or a final commercial-use approval. MeshHarbor's own project license has not been selected; there is no root `LICENSE`. The upstream MIT licenses do not grant a project-wide open-source license. Do not describe the full repository, model runtime, or future release as commercially cleared.

## Rust application dependencies

The current `Cargo.lock` resolves the Rust workspace dependency graph. A `cargo metadata --locked` inventory found no missing license expressions among third-party packages in the resolved metadata. Most packages state MIT, Apache-2.0, or both. Some require selection/preservation of their terms: `foldhash` is Zlib; `matchit` is MIT AND BSD-3-Clause; `ryu` allows Apache-2.0 OR BSL-1.0; `r-efi` offers MIT/Apache-2.0 OR LGPL-2.1-or-later. MeshHarbor application crates have no selected project license. A complete release SBOM, target-specific binary notices, and legal review are pending.

The mesh simplifier uses `meshopt` 0.7.0 (`MIT OR Apache-2.0`), which wraps/builds upstream meshoptimizer (`MIT`). The dependency graph is in `Cargo.lock`; it includes no model weights.

The optional `remesh` 0.0.5 crate (`MIT OR Apache-2.0`) is a separate isotropic triangle-remesher prototype, not QuadriFlow and not dispatched by `mesh.retopology`. It remains outside the active operation path; its transitive license inventory still needs review before any use. See [the candidate scope](retopology-candidate.md).

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

The CUDA source tree now has an opt-in managed runtime, limited to Linux x86_64/NVIDIA. Both CUDA and MLX source snapshots plus their managed adapter files are embedded into the Rust executable and extracted into app data on setup. Setup bootstraps pinned uv if needed; CUDA setup resolves upstream's loose dependency ranges into a per-install lock because no upstream lock is present, so it is not a repository-committed reproducibility guarantee. Separate model-install actions can download checkpoints only after explicit confirmation and gated access is arranged. These flows have not been exercised on this executor; no weights/checkpoints, user assets, generated images/meshes, credentials, or Python wheels are bundled. No CUDA inference compatibility, successful MLX setup, or output quality is claimed.

Before enabling inference, resolve at least:

- DINOv3 license and gated weight access, and the applicable Tencent Pixal3D model/checkpoint terms.
- NAF checkpoint commercial-use scope (upstream issue #13 remains open).
- MLX `rembg` selection: the lock uses `rembg` 2.0.76/U²-Net, while an unpinned install may choose 2.0.87 with BRIA RMBG-2.0 default (CC BY-NC absent a commercial agreement); pin the package and select an allowed model explicitly.
- CGAL linked-binary obligations, the complete native/Python package dependency license closure, and the exact OS/accelerator support matrix.
- Any generated web bundle: `web/vendor.mjs` is source only; if run, its generated `@google/model-viewer` bundle is Apache-2.0 and needs corresponding notices. No bundle is present here.

## Atlas UV integration

MeshHarbor vendors only xatlas at `f700c7790aaa030e794b52ba7791a05c085faf0c` and stb_image/stb_image_write at `4e9b368bfc071e97206ea5c6adba68dd0cbf8897` for the bounded atlas baker. xatlas is MIT; its LICENSE and upstream embedded Thekla/NVIDIA attribution blocks are retained. stb has public-domain-or-MIT terms; the full alternatives and attribution are preserved in the headers, and the integration uses the MIT option. See `crates/xatlas-sys/THIRD_PARTY_NOTICES.md`. No source from the experimental `glb-mesh-refiner` implementation, TinyBVH, or a second meshoptimizer copy is included. These notices do not select a license for the MeshHarbor project or establish commercial clearance for upstream model weights.

The new local crates were added to the workspace and lockfile, but `cargo metadata --locked` and the full workspace dependency-license inventory have not yet been rerun after that change. Re-audit the generated lock and target-specific native build before release. Current atlas tests are synthetic and do not establish broad visual or real-asset quality. See [`source-provenance.md`](source-provenance.md).

## QuadriFlow sidecar candidate

`mesh.retopology` is wired to an external QuadriFlow process, but no QuadriFlow source or binary is copied into the MeshHarbor tree. It is unavailable in the current product build. Runtime code no longer searches beside the MeshHarbor executable and never auto-executes an adjacent `quadriflow` file. A managed executable can become ready only after an in-process verifier accepts release signature/provenance and its SHA-256 matches the trusted bundled manifest. The in-process verifier is integrated, but the checked-in manifest is empty and no approved sidecar is listed, so managed readiness, installation, and rollback remain fail-closed. No MeshHarbor-issued candidate bundle has been verified yet. This also prevents a crafted activation marker from self-authorizing its own executable hash. Studio, CLI, REST, and MCP do expose backend status and an install request; Studio keeps the button disabled and the API/CLI reject installation before network downloads or QuadriFlow backend-directory writes. The manual candidate verifier is maintainer-only and is not connected to product installation. The candidate source is pinned to `hjwdzh/QuadriFlow` commit `810b7a0967c35b0dc85b4464e3835e26a756c967`. Its `LICENSE.txt` contains BSD-style three-clause terms plus an additional grant for publicly available enhancements, despite the README's MIT description.

The build smoke uses `BUILD_FREE_LICENSE=ON` (which selects Eigen's `EIGEN_MPL2_ONLY` path) and disables TBB, OpenMP, Gurobi, and Lemon's GLPK, ILOG, COIN, and SoPlex backends. Reviewed bundled components include Lemon under Boost Software License 1.0 and pcg32 under Apache-2.0. These settings and notices do not constitute complete dependency/SBOM clearance or a redistribution approval. Structural SHA-256 checks do not cryptographically verify Sigstore provenance, validate its claims, or authorize redistribution. The new manual candidate workflow has a 7-day retention for temporary CI artifacts; they are not product installers, and it has not been dispatched. The earlier synthetic smoke artifacts were temporary and are also not product installers. The current artifact collector copies notice files found in the pinned QuadriFlow source tree but does not collect package-manager license texts for installed dependencies (vcpkg, Homebrew, or apt), so the smoke archives are not complete third-party notice bundles. Preserve the upstream and third-party notices and obtain a full per-platform dependency review before shipping any binary.

The offline `scripts/quadriflow_notices.py` helper now collects only an explicit, caller-supplied package/notice inventory and emits preserved notice files, an index, an SPDX 2.3-shaped SBOM, and checksums. It always marks `release_ready=false` and `dependency_closure_verified=false`; it does not discover the full binary dependency graph, run an external SPDX validator, prove the pinned checkout, or make a legal conclusion. Complete per-platform notice closure and release review are still pending, and the existing smoke artifacts are not inputs to the manifest.

The [retopology candidate report](retopology-candidate.md) records the platform-by-platform smoke status and evidence artifact checksums. QuadriFlow itself receives geometry-only OBJ input. MeshHarbor can optionally project only supported embedded base-color/metallic-roughness PNG maps onto the triangulated GLB preview after the sidecar returns; the editable quad OBJ remains geometry-only. This candidate texture path is fail-closed but has no real-asset fidelity certification.

## Scope of use today

The operation catalog includes simplify, cleanup, shared-index part separation, render ordering, required mesh compression, and bounded xatlas atlas/rebake functions. QuadriFlow remeshing is wired through the core and API surfaces but is unavailable in this build: adjacent binaries are ignored and the release manifest is empty. The in-process Sigstore/provenance verifier is integrated, but no reviewed release artifact is embedded. The earlier source-build smoke results are platform-limited and do not establish real-asset or visual quality. Generic high-to-low texture baking remains unavailable. The separate MLX and CUDA runtime actions can download dependencies and model files only when explicitly requested, but neither setup nor inference was run here. This limited validation does not establish commercial clearance for the whole application or an inference backend.

