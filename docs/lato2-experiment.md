# LATO.2 in MeshHarbor

`mesh.retopology_lato2` is opt-in. It does not replace `mesh.simplify` or the QuadriFlow `mesh.retopology` job. It sends a geometry-only OBJ to local [LATO.2 Mesh2Mesh](https://github.com/LoHhhha/LATO.2#inference), validates and orients the predicted triangles, unwraps them with xatlas, and projects the source PBR textures along target normals on the CPU. The job does not use Blender or meshoptimizer. Only a validated, textured GLB is stored as a derived asset. The source is not modified.

## Operator Setup

1. Follow the [upstream installation guide](https://github.com/LoHhhha/LATO.2#installation) on a CUDA worker. Use a trusted checkout and install the [official checkpoints](https://huggingface.co/0x4c48/LATO.2/tree/main) into its `ckpt/` directory. The adapter targets the upstream Mesh2Mesh CLI at revision `650b48fb1257c83ec1358f0c9f1d95d5cb9c7468`.
2. Run the upstream Mesh2Mesh example on a bundled sample once during setup to verify CUDA/EGL and prime the local DINOv2 code and weights in `ckpt/dinov2`. A job does not fetch missing PyTorch Hub weights. Do not use a user asset for setup.
3. Set these absolute paths in the backend service environment and restart the service:

   ```sh
   export MESHHARBOR_LATO2_DIR=/srv/models/LATO.2
   export MESHHARBOR_LATO2_PYTHON=/srv/conda/envs/lato2/bin/python
   ```

`GET /api/v1/operations` lists the operation as implemented when the configured files are present. This does not verify GPU availability, Python packages, or the complete DINOv2 cache. MeshHarbor does not send files to the public demo or download models during setup or inference for this job. The installed binaries and model checkout run with the service account's permissions; use trusted files and a restricted worker account/network.

## Mesh To Mesh

In Studio, select a textured GLB, open **Mesh to Mesh**, select **LATO.2**, and run the job. This is separate from **Optimize** and QuadriFlow **Retopology**.

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
- `atlas_resolution`: square atlas size, 512, 1024, or 2048; default 1024.

Poll `GET /api/v1/jobs/{job_id}` for the derived asset and report. The report records the triangle counts, separate geometry and exported vertex counts, topology check, baked maps, direct ray-hit coverage, and counts of ambiguous, locally filled, and unfilled texels. Use `GET /api/v1/jobs/{job_id}/steps` to see the failed stage and inference worker logs. Inference is limited to five minutes. Cancellation during native baking is checked after the bake, before the result is saved.

## Supported Inputs

Use one static opaque indexed triangle primitive and one material with `TEXCOORD_0`, an embedded PNG base-color map, and optionally an embedded PNG metallic/roughness map. Static transform-free parent nodes are allowed. GLBs are limited to 64 MiB, 100,000 vertices, 500,000 faces, and PNG dimensions of 4,096 pixels per axis. Transparency, extra UV sets or vertex attributes, normal/occlusion/emissive maps, and compressed or external textures are not supported. Geometry-only inputs do not enter this route.

The topology checks reject open edges, nonmanifold edges/vertices, inconsistent winding, and degenerate or duplicate faces on the retopologized geometry before vertex splits at UV seams. The bake casts rays in both directions along the target face normal, and retries rejected hits along the interpolated vertex normal where it still points toward the same side. Hits must face the target and lie within 3% of the source bounds diagonal. An opposing intersection near or in front of the selected hit causes that ray to be rejected. The bake needs at least 95% direct-hit coverage before infill. Missed texels can be filled from nearby accepted pixels in the same UV chart, never across charts. If more than 0.5% of occupied texels remain empty, the job fails. Passing does not certify correct-side projection on thin shells, self-intersection freedom, or geometric distance from the source.

## Image To 3D

In Studio, open **New model**, select **LATO.2**, and provide a single PNG or JPEG. This mode uses the upstream [Image2Mesh script](https://github.com/LoHhhha/LATO.2/blob/650b48fb1257c83ec1358f0c9f1d95d5cb9c7468/scripts/image_e2e_inference.py) and outputs a geometry-only GLB; no texture or PBR map is synthesized. The job rejects open or nonmanifold predictions.

In addition to the shared LATO.2 runtime, the image profile needs the local DINOv2 ViT-L/14-with-registers weights under `ckpt/dinov2/checkpoints`, the upstream TRELLIS sparse structure checkpoint pairs under `ckpt/trellis/ckpts`, and the trusted BiRefNet code and weights under `ckpt/rembg/BiRefNet`. Use a bundled image to prime and verify the upstream CUDA pipeline during operator setup, not a user's image. MeshHarbor does not install or download these files when a job is submitted. See **Settings → Models → LATO.2** for the separate readiness of each mode.

`POST /api/v1/jobs` accepts `operation: "model.generate"`, `input: {"kind": "image", "asset_id": "..."}`, and parameters such as `{"backend_id": "lato2-cuda", "target_vertices": 2000, "seed": 42}`. The target vertex count is approximate (200–5000), and one image inference may run for up to ten minutes. Text and posed multiview input are not supported.

## Crocodile At 3,008 Triangles

On the same 11,222-triangle textured source, conventional Simplify preserved shape better: symmetric surface RMS was 0.119% of the source bounding-box diagonal, versus 0.173% for LATO.2. The 95th-percentile errors were 0.242% and 0.348%, respectively. We sampled 50,000 area-weighted points per direction and measured distance to the closest point on the other triangle surface. Simplify used a 3,008-triangle limit and `max_error=0.02`; the captured LATO.2 prediction used a 1,200-vertex hint and seed 42, and its rebaked result passed the same triangle limit.

After welding coincident positions for the topology analysis (without changing either GLB), both outputs had one component and no boundary edges. Simplify had nine nonmanifold edges, while LATO.2 had none and was watertight. The exported textured GLBs still split vertices at UV seams. Prefer Simplify for shape and UV preservation; use LATO.2 when rebuilding a watertight surface matters more than keeping fine details unchanged. This is one sample, not a global accuracy guarantee.
