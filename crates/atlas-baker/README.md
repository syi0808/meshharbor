# GLB atlas UV rebake

`meshharbor-atlas-baker` is an isolated, static-mesh GLB operation built around xatlas. It creates a fresh `TEXCOORD_0` chart layout and resamples the existing surface appearance into an embedded base-color PNG and metallic/roughness PNG. The output keeps the original triangle count and geometry; this operation does **not** simplify a mesh or project texture from a different high-resolution mesh.

## Current support boundary

- One GLB mesh and one TRIANGLES primitive, with one PBR metallic-roughness material.
- Existing float `POSITION` and `TEXCOORD_0`; optional float `NORMAL`.
- Embedded PNG base-color and metallic/roughness images. Both texture slots must exist and use separate texture objects.
- REPEAT wrapping and linearly filtered samplers only. The atlas size is an xatlas packing hint, so the final width/height can be smaller than requested.
- Static, non-morphing, non-skinned GLB without glTF extensions, texture transforms, external URIs, sparse accessors, or primitive extensions.
- Other scalar/vector vertex streams (for example COLOR, JOINTS, WEIGHTS, secondary UVs and custom attributes) are copied byte-for-byte through xatlas' source-vertex references. `TEXCOORD_0` and triangle indices are regenerated. `TANGENT` is rejected because its basis must be recomputed for the new UVs.
- Normal, occlusion and emissive texture maps are rejected rather than left pointed at stale UVs. Geometry-changing high-to-low projection is a separate future capability.

Resource limits are 5 million input vertices, 15 million indices, 128 MiB per PNG, and at most a 4096 packing resolution / 16 million texels. `padding` is limited to 64. The baker filters base-color RGB in linear light and keeps metallic/roughness channels linear; it uses bilinear REPEAT sampling and fills xatlas-labeled padding within the same chart.

## Tests

Run `cargo test --manifest-path crates/atlas-baker/Cargo.toml`. The native xatlas dependency is compiled from vendored source using the platform C++ compiler. The GLB tests exercise a tiny textured mesh, attribute remapping, texture rewiring and fail-closed behavior for unsupported tangent/map cases.

## Provenance and notices

Only xatlas and stb_image/stb_image_write are vendored for unwrap and PNG I/O. The experimental `glb-mesh-refiner` simplifier, TinyBVH, meshoptimizer copy, and unrelated refiner CLI are not included. See [`../xatlas-sys/THIRD_PARTY_NOTICES.md`](../xatlas-sys/THIRD_PARTY_NOTICES.md) and the license blocks embedded in the upstream headers.
