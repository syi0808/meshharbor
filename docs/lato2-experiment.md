# LATO.2 retopology and texture rebake

`mesh.retopology_lato2` is opt-in. It does not replace `mesh.simplify` or the QuadriFlow `mesh.retopology` job. It sends a geometry-only OBJ to local [LATO.2 Mesh2Mesh](https://github.com/LoHhhha/LATO.2#inference), validates and orients the predicted triangles, then creates UVs and projects the source PBR textures onto them using headless Blender/Cycles. Only a validated, textured GLB is stored as a derived asset. The source is not modified.

## Operator Setup

1. Follow the [upstream installation guide](https://github.com/LoHhhha/LATO.2#installation) on a CUDA worker. Use a trusted checkout and install the [official checkpoints](https://huggingface.co/0x4c48/LATO.2/tree/main) into its `ckpt/` directory. The adapter targets the upstream Mesh2Mesh CLI at revision `650b48fb1257c83ec1358f0c9f1d95d5cb9c7468`.
2. Run the upstream Mesh2Mesh example on a bundled sample once during setup to verify CUDA/EGL and prime the local DINOv2 code and weights in `ckpt/dinov2`. A job does not fetch missing PyTorch Hub weights. Do not use a user asset for setup.
3. Make a Cycles-enabled Blender installation available (rebaking tested with Blender 5.1.2). Set these absolute paths in the backend service environment and restart the service:

   ```sh
   export MESHHARBOR_LATO2_DIR=/srv/models/LATO.2
   export MESHHARBOR_LATO2_PYTHON=/srv/conda/envs/lato2/bin/python
   export MESHHARBOR_LATO2_BLENDER=/opt/blender/blender
   ```

`GET /api/v1/operations` lists the operation as implemented when the configured files are present. This does not verify GPU availability, Python packages, or the complete DINOv2 cache. MeshHarbor does not send files to the public demo or download models during setup or inference for this job. The installed binaries and model checkout run with the service account's permissions; use trusted files and a restricted worker account/network.

## Run A Job

Submit to `POST /api/v1/jobs`:

```json
{
  "operation": "mesh.retopology_lato2",
  "input": {"asset_id": "<imported GLB asset id>"},
  "parameters": {"target_vertices": 1200, "max_triangles": 3200, "atlas_resolution": 1024}
}
```

- `target_vertices`: approximate model conditioning, 200–5000, default 2000. It is not a limit or a guaranteed vertex count. UV seams can increase the exported vertex count.
- `max_triangles`: optional hard cap, 4–20,000. The result must also have fewer triangles than the source. A prediction over the cap fails rather than being simplified again.
- `atlas_resolution`: 512, 1024, or 2048; default 1024.

Poll `GET /api/v1/jobs/{job_id}` for the derived asset and report. The report records the triangle counts, separate geometry and exported vertex counts, topology check, baked maps, and projection-hit coverage. Use `GET /api/v1/jobs/{job_id}/steps` to see the failed stage and worker logs. Inference and Blender are limited to five and three minutes respectively.

## Supported Inputs

Use one static opaque indexed triangle primitive and one material with `TEXCOORD_0`, an embedded PNG base-color map, and optionally an embedded PNG metallic/roughness map. Static transform-free parent nodes are allowed. GLBs are limited to 64 MiB, 100,000 vertices, 500,000 faces, and PNG dimensions of 4,096 pixels per axis. Transparency, extra UV sets or vertex attributes, normal/occlusion/emissive maps, and compressed or external textures are not supported. Geometry-only inputs do not enter this route.

The topology checks reject open edges, nonmanifold edges/vertices, inconsistent winding, and degenerate or duplicate faces on the retopologized geometry before vertex splits at UV seams. The bake requires hits on at least 95% of occupied atlas texels using a fixed-distance projection relative to the source extent. Passing does not certify correct-side projection on thin shells, self-intersection freedom, or geometric distance from the source. Small missed regions can remain visible even on a passing bake.
