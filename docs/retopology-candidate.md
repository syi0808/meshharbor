# Experimental retopology

## Product status

MeshHarbor's `mesh.retopology` path is now a real QuadriFlow quad-remeshing candidate, not the separate isotropic triangle-remesher prototype.

The operation is wired through the Rust core, REST job API, MCP tool, CLI, and Studio. It is reported as available only when an executable `quadriflow` sidecar is present beside the running MeshHarbor executable (on Windows, `quadriflow.exe`) or in the managed MeshHarbor data directory. Availability detection validates the managed activation marker and executable hash without executing it. CLI (`meshharbor backend status` / `meshharbor backend install quadriflow`), REST (`GET /api/v1/backends/quadriflow`, `POST /api/v1/backends/quadriflow/install`), MCP tools, and Studio expose backend status and an install request. The release manifest is empty, and this build has no release-signature/provenance verifier, so installation stays unavailable and fails before network downloads or QuadriFlow backend-directory writes.

A completed job stores two outputs together:
- A triangle-only GLB preview for viewing.
- The exact validated QuadriFlow OBJ bytes as a `quad_topology` artifact, which preserves editable quad faces.

The core commits both artifacts in one database transaction. REST artifact downloads and the Studio download control can retrieve the OBJ. CLI can write it with `--quad-output`; MCP can export it with `export_quad_obj`, restricted to the configured MeshHarbor export directory and refusing to overwrite an existing file.

## Geometry and operation contract

The operation accepts one embedded-buffer GLB containing one closed, static, indexed TRIANGLES primitive. Geometry-only input uses POSITION-only float32 data. The optional textured path additionally requires float32 TEXCOORD_0, optional NORMAL, one PBR material, and embedded PNG base-color and metallic/roughness maps. It rejects animation/skinning, morph targets, extensions, extra nodes/meshes/primitives, node transforms, tangents, vertex colors, external images, unsupported material maps, and other attributes. The source asset remains immutable. The sidecar receives geometry only; its original quad topology is stored as a geometry-only OBJ. The GLB preview is triangulated because glTF/GLB has no quad primitive mode.

For supported textured input, MeshHarbor creates a fresh xatlas UV layout for the retopologized geometry and projects the original base-color/metallic-roughness maps through a BVH nearest-surface query. It fails closed if the distance cap is exceeded (2% of the source bounds diagonal), the source-facing-normal dot falls below 0.25, opposing near-equal surfaces make projection ambiguous, or coverage falls below 99.5%. The target atlas is capped at 16 million texels and input images are capped at 16 million pixels. Only those two maps appear in the preview; the OBJ has no UVs or appearance. Passing these gates does not establish perceptual texture quality, thin-feature retention, or correspondence on arbitrary topology.

Input validation requires closed, consistently oriented, edge- and vertex-manifold geometry, rejects duplicate or degenerate faces, and enforces the conservative output budget used by the existing geometry validator. The generated OBJ contains only positions and triangle faces; arbitrary OBJ from a user is never passed directly to QuadriFlow. The wrapper uses fixed arguments and seed, never enables `-sat`, clears the child environment, redirects standard streams to null, uses a private per-job temporary directory, bounds inputs/outputs, and terminates the child after 180 seconds. This is process isolation, not an OS sandbox, and it does not impose a hard memory or CPU quota.

Current limits are 64 MiB input GLB, 100,000 input vertices, 200,000 input triangles, a requested target of 4–50,000 quads (default 10,000), 64 MiB output OBJ, 250,000 output vertices, 500,000 output quads, and 180 seconds. QuadriFlow's target count is approximate.

The output parser enforces UTF-8 and byte/record/count bounds, finite positions in a broad source-bounds envelope, four distinct in-range vertices per triangulable quad, duplicate-face rejection, nonzero face area, closed and consistently oriented edge manifoldness, and connected vertex links. It creates smooth-normal triangle GLB previews and optionally projects the two supported maps under the gates above. It does **not** certify self-intersection freedom, a Hausdorff/surface-error tolerance, thin-feature retention, or high-to-low texture fidelity. The UI and operation report label this limitation; inspect experimental output before production use.

## Verified native smoke test

The user-approved platform smoke builds the pinned official QuadriFlow source at `810b7a0967c35b0dc85b4464e3835e26a756c967`, sets `BUILD_FREE_LICENSE=ON`, disables TBB/OpenMP/Gurobi and Lemon solver backends, and executes only the repository's synthetic cube fixture (never a user model) with fixed face target/seed. It then parses the actual OBJ and validates the GLB preview. No textured or representative production asset was used.

- Linux x64: passed on Ubuntu 24.04 in [run 43416384828](https://github.com/syi0808/meshharbor/actions/runs/43416384828); uploaded artifact ID `14032331288`, SHA-256 `27f8da01b174633403c4f9180067e9287da1953196f03f635422a3e43b4ec90e`.
- macOS ARM64: passed on macOS 15 in [run 43418178432](https://github.com/syi0808/meshharbor/actions/runs/43418178432); uploaded artifact ID `14032633014`, SHA-256 `c91d2b89f0ab8844d53eb306e803d3733fd93f8e28f384624b8c21978eecc685`. The runner's CMake 4 required `CMAKE_POLICY_VERSION_MINIMUM=3.5` because upstream declares CMake 3.1; the compatibility flag does not patch upstream source.
- Windows x64: passed on Windows Server 2022 in [run 43424082291](https://github.com/syi0808/meshharbor/actions/runs/43424082291); artifact ID `14039052050`, artifact ZIP SHA-256 `98d40bf1f35eb95813da6ad484c412d6857866a91e8dbe94048fec5a7b0375cf`. The first attempt exposed a mismatch between vcpkg's static CRT and vendored LEMON; the retry set `CMP0091=NEW` and `CMAKE_MSVC_RUNTIME_LIBRARY` to the static `/MT` runtime and passed.

An initial macOS 14 runner job was cancelled before receiving a runner. The label is marked deprecated in GitHub's runner image inventory, but that alone does not prove why the job was cancelled. The three successful artifacts are temporary, unsigned Actions evidence retained for 14 days; they are not signed or packaged MeshHarbor sidecars.

These successful runs prove only a minimal source-build/run/parse path for a synthetic cube on those exact runners. They do not validate complex models, self-intersections, thin leaves, texture projection fidelity, macOS Intel, or a distributable MeshHarbor product.

## Upstream and licensing

No QuadriFlow source code is copied into MeshHarbor. The upstream repository was inspected at the pinned commit above. Its README describes the project as MIT, but `LICENSE.txt` contains BSD-style three-clause terms plus an additional grant for publicly available enhancements. Preserve the complete upstream license and copyright notices and obtain license review before distribution.

Other reviewed components declare Boost Software License 1.0 (Lemon), Apache-2.0 (pcg32), and BSD-3-Clause (`parallel_stable_sort.h`, included with TBB). Eigen's Sparse Cholesky path is LGPL; `BUILD_FREE_LICENSE=ON` defines `EIGEN_MPL2_ONLY` and selects Eigen SparseLU. This does not make every dependency MIT. A controlled build should explicitly disable TBB, OpenMP, Gurobi, and Lemon's GLPK, ILOG, COIN, and SoPlex backends. The upstream CMake file adds `-fopenmp` unconditionally for GNU compilers despite the OpenMP option, so every intended platform still needs independent build and packaging verification.

## Packaging work still required

A distributable build must place a pinned, tested `quadriflow` executable beside the MeshHarbor executable and retain all applicable upstream/dependency notices. The current branch detects that sidecar and routes jobs through it, but does not build or download the executable automatically, does not yet ship it for any platform, and has not passed a full per-crate license/SBOM audit. Do not mark the feature available in a package that omits the sidecar. No model data leaves the local MeshHarbor process.

## Separate triangle-remesher prototype

The optional Rust `remesh = 0.0.5` worker is an experimental isotropic **triangle** remesher. It splits/collapses/flips edges and smooths vertices; it is not QuadriFlow, does not produce quads, does not split assets into separate scene nodes, and is not dispatched by `mesh.retopology`. Its strict geometry-only GLB candidate remains separate. It pins its Cargo dependency graph in `Cargo.lock`, uses a separate child process with private temporary files and a 180-second timeout, and has passed stable/MSRV checks. This is not an OS sandbox or proof of geometric quality.
