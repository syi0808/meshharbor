# Experimental retopology candidates

## Status

This remains a feature-branch candidate. Core now has an isolated worker adapter
a strict untextured-GLB adapter and child-process end-to-end test, plus a separate
QuadriFlow OBJ-output validator and triangle-only GLB preview builder, but neither
candidate is wired into the operation catalog, GLB job flow, REST/MCP tools, or Studio.
Keep `mesh.retopology` unavailable until broader geometry-quality/self-intersection
acceptance and application packaging of the sidecar worker are complete.

This is a real isotropic **triangle remesher**, not mesh simplification and not a
field-aligned quad remesher. It splits and collapses edges, flips edges for triangle
quality, and smooths vertices. It does not produce quads. It does not split connected
parts into separate assets; that is a different operation.

## Data contract

The worker accepts only a versioned JSON request containing indexed triangle
positions and indices. The GLB adapter accepts one embedded, untextured static
TRIANGLES primitive with POSITION-only float32 geometry; it rejects UVs, textures,
input normals, colors, tangents, custom attributes, skinning, morph targets,
animations, extensions, extra nodes/meshes/primitives, and blended materials. It
recomputes vertex normals and preserves only a supported untextured material. There
is no attribute or appearance transfer. Atlas UV rebaking is same-geometry only,
not a high-to-low projection workflow.

The shared geometry validator checks finite positions/parameters, index range,
triangle arity, repeated/duplicate faces, zero-area faces, closed and consistently
oriented edge-manifold topology, connected vertex links, and an area-based output
budget estimate. It does not eliminate self-intersections or guarantee a measured
Hausdorff/geometric error tolerance.
The output envelope is deliberately loose: positions must stay within a sphere of
radius two source-bounds diagonals around the source center; this is a runaway-output
guard, not an approximation-quality guarantee.

Current limits are 100,000 input vertices, 200,000 input triangles, 250,000
output vertices, 500,000 output triangles, target edge ratio 0.005–0.25 of the
input bounding-box diagonal (subject to a conservative estimated output budget of
100,000 triangles), 1–8 iterations, 64 MiB request/response files, and a 180-second
host timeout. These bounds and the area-based estimate reduce risk but are not a
hard OS memory/CPU quota.

## Isolation and dependency

The optional worker is a separate executable so an upstream panic terminates that
child instead of the REST/MCP process, and unconditional stdout/stderr progress is
captured rather than corrupting MCP's stdio protocol. The child environment is
cleared and its JSON/log files live in a temporary directory (mode 0700 on Unix).
This is **not an OS sandbox** and does not impose a hard memory/CPU limit; it runs
with the same OS user privileges and filesystem access as MeshHarbor.

The adapter pins `remesh = 0.0.5`, an experimental crate whose source carries
`MIT OR Apache-2.0`. The exact transitive graph is committed in `Cargo.lock` and
has passed locked builds/tests on stable Rust and the workspace's declared Rust
1.88 MSRV. This is not a complete per-crate license/SBOM audit; there is no copied
upstream source in MeshHarbor.


## QuadriFlow path (not yet integrated)

The upstream QuadriFlow repository was inspected at commit
`810b7a0967c35b0dc85b4464e3835e26a756c967`. No upstream code has been copied or
built into MeshHarbor. The repository README calls the project MIT, but its
`LICENSE.txt` contains BSD-style three-clause terms plus an additional grant for
publicly available enhancements. Preserve that full license text and obtain a
proper license review before distribution. Other included components declare
Boost Software License 1.0 (Lemon), Apache-2.0 (pcg32), and BSD-3-Clause
(`parallel_stable_sort.h`, used with TBB). Eigen's Sparse Cholesky component is
LGPL; `BUILD_FREE_LICENSE=ON` selects Eigen's MPL2-only SparseLU path. This does
not make every dependency MIT.

Upstream requires C++14, Boost, and Eigen. A controlled build should set
`BUILD_FREE_LICENSE=ON`, disable TBB/Gurobi/SAT, and explicitly disable Lemon's
GLPK, ILOG, COIN, and SoPlex backends. Although `BUILD_OPENMP=OFF` is available,
the upstream CMake file adds `-fopenmp` unconditionally for GNU compilers. Build
compatibility, dependency versions, packaging, and runtime behavior have not yet
been verified on Linux, macOS, or Windows.

The CLI accepts OBJ and outputs OBJ with four-index quad faces. Its loader ignores
UVs and normals, and its argument parser is not robust against missing values.
A future wrapper must create a strict geometry-only OBJ itself, use fixed bounded
arguments and a private per-job working directory, and avoid the `-sat` option:
that path invokes external `minisat` and `timeout` commands. The original quad
OBJ must be retained as a downloadable artifact because GLB can only carry
triangle primitives. The core candidate in `crates/core/src/quadriflow.rs`
validates QuadriFlow-style OBJ output and builds a triangle-only GLB preview; it
does not invoke QuadriFlow or attach/save the OBJ artifact.

The current quad-output validator enforces UTF-8 and file/record/count bounds,
finite coordinates within a broad source-bounds envelope, four distinct indices
per triangulable quad, duplicate-face rejection, closed and consistently oriented
edge-manifold topology, and connected vertex links. It rejects unsupported OBJ
records and creates a smooth-normal GLB preview. It does **not** test
self-intersections, compare output surface error to the input, or certify
QuadriFlow's output quality. Keep the operation unavailable until source/build
packaging, OBJ artifact delivery, quality gates, and native-process isolation are
implemented and tested.

The branch workflow checks formatting, strict Clippy (allowing one new lint from
the latest toolchain in an existing xatlas file), locked all-feature tests on stable
and Rust 1.88, and exports the lockfile artifact. Re-run these checks on the final
code before integration.
