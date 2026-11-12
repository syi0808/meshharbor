import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { Store } from "./store.mjs";

test("stores immutable source and derived artifacts with SQLite metadata", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "meshharbor-store-"));
  const store = new Store(root);
  try {
    const original = Buffer.from("original GLB bytes");
    const imported = await store.importBuffer({ buffer: original, name: "../turnip.glb", kind: "mesh", mimeType: "model/gltf-binary", metadata: { triangles: 12 } });
    assert.equal(imported.asset.name, "turnip.glb");
    assert.equal((await readFile(imported.artifact.file_path)).toString(), "original GLB bytes");
    const derived = await store.createDerivedAsset({
      sourceAssetId: imported.asset.id,
      name: "simplified-turnip.glb",
      kind: "mesh",
      metadata: { operation: "mesh.simplify", triangles: 4 },
      artifacts: [{ kind: "result", name: "result.glb", mimeType: "model/gltf-binary", buffer: Buffer.from("derived GLB bytes") }]
    });
    assert.equal(derived.asset.source_asset_id, imported.asset.id);
    assert.equal((await readFile(imported.artifact.file_path)).toString(), "original GLB bytes");
    assert.equal((await readFile(derived.artifacts[0].file_path)).toString(), "derived GLB bytes");
    assert.equal(store.getAsset(derived.asset.id).metadata.triangles, 4);
    const job = store.createJob({ operation: "mesh.simplify", input: { asset_id: imported.asset.id }, parameters: { target_triangles: 4 } });
    store.updateJob(job.id, { state: "running" });
    store.addJobEvent(job.id, "progress", { progress: 0.5 });
    assert.equal(store.getJob(job.id).state, "running");
    assert.equal(store.listJobEvents(job.id).at(-1).event, "progress");
  } finally {
    store.close();
    await rm(root, { recursive: true, force: true });
  }
});
