# Experimental triangle-remeshing worker

## Status

This remains a feature-branch candidate. Core now has an isolated worker adapter
and a strict untextured-GLB adapter, with a real child-process end-to-end test, but
it is not wired into the operation catalog, GLB job flow, REST/MCP tools, or Studio.
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

The branch workflow checks formatting, strict Clippy (allowing one new lint from
the latest toolchain in an existing xatlas file), locked all-feature tests on stable
and Rust 1.88, and exports the lockfile artifact. Re-run these checks on the final
code before integration.
