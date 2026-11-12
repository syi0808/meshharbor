import test from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createApiServer } from "./api.mjs";
import { createGridGlb } from "./fixtures.test-helper.mjs";
import { inspectGlb } from "./glb.mjs";
import { JobManager } from "./jobs.mjs";
import { Store } from "./store.mjs";

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

test("REST import and simplification create a derived asset while preserving the source", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "meshharbor-api-"));
  const store = new Store(root);
  const projectRoot = path.resolve(path.dirname(new URL(import.meta.url).pathname), "../../../..");
  const jobs = new JobManager({ store, projectRoot, timeoutMs: 30_000 });
  const token = "test-token-" + Math.random().toString(16).slice(2);
  const server = createApiServer({ store, jobs, token, projectRoot });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  const base = `http://127.0.0.1:${address.port}`;
  const auth = { Authorization: `Bearer ${token}` };
  try {
    const health = await fetch(`${base}/v1/health`);
    assert.equal(health.status, 200);
    const locked = await fetch(`${base}/v1/operations`);
    assert.equal(locked.status, 401);

    const operationsResponse = await fetch(`${base}/v1/operations`, { headers: auth });
    const operations = (await operationsResponse.json()).operations;
    assert.equal(operations.find(operation => operation.id === "mesh.simplify").state, "available_on_this_host");
    assert.equal(operations.find(operation => operation.id === "image.generate").state, "pending");

    const pendingGeneration = await fetch(`${base}/v1/jobs`, {
      method: "POST",
      headers: { ...auth, "Content-Type": "application/json" },
      body: JSON.stringify({ operation: "image.generate", input: {}, parameters: {} })
    });
    assert.equal(pendingGeneration.status, 409);

    const sourceBytes = createGridGlb(4);
    const importedResponse = await fetch(`${base}/v1/assets/import?name=harbor-grid.glb`, {
      method: "POST",
      headers: { ...auth, "Content-Type": "application/octet-stream" },
      body: sourceBytes
    });
    assert.equal(importedResponse.status, 201);
    const imported = await importedResponse.json();
    assert.equal(imported.asset.kind, "mesh");
    assert.equal(imported.asset.metadata.triangles, 32);
    const sourceArtifact = imported.asset.artifacts.find(artifact => artifact.kind === "source");
    const before = await readFile(store.getArtifact(sourceArtifact.id).file_path);

    const jobResponse = await fetch(`${base}/v1/jobs`, {
      method: "POST",
      headers: { ...auth, "Content-Type": "application/json" },
      body: JSON.stringify({ operation: "mesh.simplify", input: { asset_id: imported.asset.id }, parameters: { target_triangles: 16 } })
    });
    assert.equal(jobResponse.status, 202);
    const { job: createdJob } = await jobResponse.json();
    const deadline = Date.now() + 10_000;
    let job = store.getJob(createdJob.id);
    while (Date.now() < deadline && !["succeeded", "failed", "cancelled"].includes(job.state)) {
      await sleep(40);
      job = store.getJob(createdJob.id);
    }
    assert.equal(job.state, "succeeded", job.error?.message || "Simplification job did not finish.");
    assert.notEqual(job.result.output_asset_id, imported.asset.id);
    assert.equal(job.result.input_triangles, 32);
    assert.ok(job.result.actual_triangles > 0);
    const outputArtifact = store.getArtifact(job.result.output_artifact_id);
    const outputBytes = await readFile(outputArtifact.file_path);
    assert.equal(inspectGlb(outputBytes).triangles, job.result.actual_triangles);
    assert.deepEqual(await readFile(store.getArtifact(sourceArtifact.id).file_path), before);
    assert.equal(store.getAsset(job.result.output_asset_id).source_asset_id, imported.asset.id);
  } finally {
    await new Promise(resolve => server.close(resolve));
    store.close();
    await rm(root, { recursive: true, force: true });
  }
});
