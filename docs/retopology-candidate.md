# Experimental triangle-remeshing worker

## Status

This is a feature-branch candidate only. The worker and geometry adapter are not
wired into MeshHarbor's operation catalog, GLB job flow, REST/MCP tools, or Studio.
Keep `mesh.retopology` unavailable until the Cargo lock is committed, the worker
builds and passes its tests, and an asset-level acceptance path rejects unsupported
GLB data.

This is a real isotropic **triangle remesher**, not mesh simplification and not a
field-aligned quad remesher. It splits and collapses edges, flips edges for triangle
quality, and smooths vertices. It does not produce quads. It does not split connected
parts into separate assets; that is a different operation.

## Data contract

The candidate worker accepts only a versioned JSON request containing indexed
triangle positions and indices. It has no attribute-transfer API. Any GLB caller
must reject UVs, textures, colors, tangents, normals/custom vertex attributes,
skinning, morph targets, animations, compressed/extension-bearing primitives, and
multi-primitive or multi-mesh inputs unless a later implementation explicitly
preserves or rebakes them. In particular, atlas UV rebaking is not a high-to-low
projection workflow, so remeshed geometry cannot inherit the source appearance.

The geometry adapter validates finite positions/parameters, index range, triangle
arity, repeated/duplicate faces, zero-area faces, and edge-manifold input/output.
It also bounds the output to the source bounding region. It does not prove
vertex-manifold topology, eliminate self-intersections, or guarantee a measured
Hausdorff/geometric error tolerance.

Current limits are 200,000 input vertices, 400,000 input triangles, 1,000,000
output vertices, 2,000,000 output triangles, target edge ratio 0.005–0.25 of the
input bounding-box diagonal, 1–8 iterations, a 128 MiB request and 256 MiB response,
and a 180-second host timeout.

## Isolation and dependency

The optional worker is a separate executable so an upstream panic terminates that
child instead of the REST/MCP process, and unconditional stdout/stderr progress is
captured rather than corrupting MCP's stdio protocol. The child environment is
cleared and its JSON/log files live in a temporary directory (mode 0700 on Unix).
This is **not an OS sandbox** and does not impose a hard memory/CPU limit; it runs
with the same OS user privileges and filesystem access as MeshHarbor.

The adapter pins `remesh = 0.0.5`, an experimental crate dual-licensed under
MIT OR Apache-2.0. Do not treat that direct-dependency notice as a project-wide
license decision. The exact MeshHarbor transitive lock graph still needs to be
generated, reviewed, committed, and tested before merging or enabling this path.

The branch workflow checks formatting and runs `cargo test --workspace
--all-features`, then exports the generated lockfile for review. These CI checks
must pass on the final code before integration.
