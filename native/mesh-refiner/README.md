# GLB simplification worker

This directory contains the core GLB simplifier from [syi0808/glb-mesh-refiner](https://github.com/syi0808/glb-mesh-refiner), pinned to commit `35383ddab8bbe491b98264b42892c42684c7d514`. See [source provenance](../../docs/source-provenance.md) and [licensing status](../../docs/licensing-status.md) before distribution.

## Build and test

```sh
make
make test
```

The core worker needs only a C++17 compiler. The test suite additionally uses Python 3, NumPy, and Pillow to build small GLB fixtures. The optional atlas baker and its third-party libraries are intentionally omitted from this first integration.

## CLI

```text
glb_refine input.glb output.glb [target_triangles=4000] [report.tsv] [envelope_world_limit=0.004] [uv_stretch_limit=1.25] [source_normal_cos=0.8660254]
```

The target is aspirational. Safety and preservation checks may retain triangles above target. The envelope is a sampled heuristic, not a certified world-space bound; the source worker compares stored mesh positions without applying node transforms.

The worker accepts one static mesh with one indexed triangle primitive and float `POSITION`, `NORMAL`, and `TEXCOORD_0`. It rejects other attributes, animation, skinning, morph targets, sparse accessors, extensions, extra meshes, and extra primitives. The output GLB, a component report, and a `*.provenance.csv` mapping are created separately from the input.
