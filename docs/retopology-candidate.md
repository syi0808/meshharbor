# Experimental retopology

## Product status

MeshHarbor's `mesh.retopology` path is now a real QuadriFlow quad-remeshing candidate, not the separate isotropic triangle-remesher prototype.

The operation is wired through the Rust core, REST job API, MCP tool, CLI, and Studio. It is reported as available only when an executable `quadriflow` sidecar is present beside the running MeshHarbor executable (on Windows, `quadriflow.exe`). Availability detection checks the file and its Unix executable bit without executing it. The current source build does not build, download, or install the sidecar automatically, so ordinary packages still report the operation as unavailable until a sidecar is packaged.

A completed job stores two outputs together:
- A triangle-only GLB preview for viewing.
- The exact validated QuadriFlow OBJ bytes as a `quad_topology` artifact, which preserves editable quad faces.

The core commits both artifacts in one database transaction. REST artifact downloads and the Studio download control can retrieve the OBJ. CLI can write it with `--quad-output`; MCP can export it with `export_quad_obj`, restricted to the configured MeshHarbor export directory and refusing to overwrite an existing file.

## Geometry and operation contract

The operation accepts one embedded-buffer GLB containing one static, indexed TRIANGLES primitive with POSITION-only float32 geometry. It rejects animated/skinned assets, morph targets, extensions, multiple nodes/meshes/primitives, node transforms, UVs, images, textures, vertex colors, and unsupported attributes. The source asset remains immutable. A supported untextured material may be present, but appearance, materials, normals, UVs, and textures are not transferred to the OBJ. The GLB preview is triangulated because glTF/GLB has no quad primitive mode.

Input validation requires closed, consistently oriented, edge- and vertex-manifold geometry, rejects duplicate or degenerate faces, and enforces the conservative output budget used by the existing geometry validator. The generated OBJ contains only positions and triangle faces; arbitrary OBJ from a user is never passed directly to QuadriFlow. The wrapper uses fixed arguments and seed, never enables `-sat`, clears the child environment, redirects standard streams to null, uses a private per-job temporary directory, bounds inputs/outputs, and terminates the child after 180 seconds. This is process isolation, not an OS sandbox, and it does not impose a hard memory or CPU quota.

Current limits are 64 MiB input GLB, 100,000 input vertices, 200,000 input triangles, a requested target of 4–50,000 quads (default 10,000), 64 MiB output OBJ, 250,000 output vertices, 500,000 output quads, and 180 seconds. QuadriFlow's target count is approximate.

The output parser enforces UTF-8 and byte/record/count bounds, finite positions in a broad source-bounds envelope, four distinct in-range vertices per triangulable quad, duplicate-face rejection, nonzero face area, closed and consistently oriented edge manifoldness, and connected vertex links. It creates smooth-normal triangle GLB previews. It does **not** certify self-intersection freedom, a Hausdorff/surface-error tolerance, thin-feature retention, or high-to-low texture fidelity. The UI and operation report label this limitation; inspect experimental output before production use.

## Verified native smoke test

The isolated GitHub Actions job built the official QuadriFlow source at commit `810b7a0967c35b0dc85b4464e3835e26a756c967` on Ubuntu 24.04, using the free-license SparseLU option and disabling optional TBB, OpenMP, Gurobi, and Lemon solver backends. It ran exactly once on a synthetic cube (not a user model), with a fixed face target and seed. The actual output passed the strict quad OBJ validator and GLB preview inspector. Run: [43397667911](https://github.com/syi0808/meshharbor/actions/runs/43397667911).

That one test proves only a minimal build/run/parse path for a synthetic cube. It did not preserve a distributable executable artifact or record a general quality metric, and it does not validate complex models, self-intersections, thin leaves, textures, macOS, or Windows. No additional QuadriFlow build or model run is authorized by this smoke result.

## Upstream and licensing

No QuadriFlow source code is copied into MeshHarbor. The upstream repository was inspected at the pinned commit above. Its README describes the project as MIT, but `LICENSE.txt` contains BSD-style three-clause terms plus an additional grant for publicly available enhancements. Preserve the complete upstream license and copyright notices and obtain license review before distribution.

Other reviewed components declare Boost Software License 1.0 (Lemon), Apache-2.0 (pcg32), and BSD-3-Clause (`parallel_stable_sort.h`, included with TBB). Eigen's Sparse Cholesky path is LGPL; `BUILD_FREE_LICENSE=ON` defines `EIGEN_MPL2_ONLY` and selects Eigen SparseLU. This does not make every dependency MIT. A controlled build should explicitly disable TBB, OpenMP, Gurobi, and Lemon's GLPK, ILOG, COIN, and SoPlex backends. The upstream CMake file adds `-fopenmp` unconditionally for GNU compilers despite the OpenMP option, so every intended platform still needs independent build and packaging verification.

## Packaging work still required

A distributable build must place a pinned, tested `quadriflow` executable beside the MeshHarbor executable and retain all applicable upstream/dependency notices. The current branch detects that sidecar and routes jobs through it, but does not build or download the executable automatically, does not yet ship it for any platform, and has not passed a full per-crate license/SBOM audit. Do not mark the feature available in a package that omits the sidecar. No model data leaves the local MeshHarbor process.

## Separate triangle-remesher prototype

The optional Rust `remesh = 0.0.5` worker is an experimental isotropic **triangle** remesher. It splits/collapses/flips edges and smooths vertices; it is not QuadriFlow, does not produce quads, does not split assets into separate scene nodes, and is not dispatched by `mesh.retopology`. Its strict geometry-only GLB candidate remains separate. It pins its Cargo dependency graph in `Cargo.lock`, uses a separate child process with private temporary files and a 180-second timeout, and has passed stable/MSRV checks. This is not an OS sandbox or proof of geometric quality.
