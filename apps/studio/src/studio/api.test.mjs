import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';

// Use the project's TypeScript compiler when installed (Node 20+), with a
// native type-stripping fallback for minimal Node 24 test environments.
let MeshHarborApi, quadTopologyArtifact, cancelJobNotice, isQueuedJobCancellable, isActiveJob, canInspectJobHistoryRecord, preferTrackedTerminalJob, shouldDeferTrackedJobCompletion, LatestRequestGuard, jobPollRetryDelay;
try {
  const ts = await import('typescript');
  const source = await readFile(new URL('./api.ts', import.meta.url), 'utf8');
  const compiled = ts.default.transpileModule(source, {
    compilerOptions: { module: ts.default.ModuleKind.ESNext, target: ts.default.ScriptTarget.ES2022 },
  }).outputText;
  ({ MeshHarborApi, quadTopologyArtifact, cancelJobNotice, isQueuedJobCancellable, isActiveJob, canInspectJobHistoryRecord, preferTrackedTerminalJob, shouldDeferTrackedJobCompletion, LatestRequestGuard, jobPollRetryDelay } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`));
} catch (error) {
  if (error?.code !== 'ERR_MODULE_NOT_FOUND') throw error;
  ({ MeshHarborApi, quadTopologyArtifact, cancelJobNotice, isQueuedJobCancellable, isActiveJob, canInspectJobHistoryRecord, preferTrackedTerminalJob, shouldDeferTrackedJobCompletion, LatestRequestGuard, jobPollRetryDelay } = await import('./api.ts'));
}

test('Studio offers cancellation only before a job leaves the queue', () => {
  assert.equal(isQueuedJobCancellable({ state: 'queued' }), true);
  for (const state of ['running', 'validating', 'cancelling', 'cancelled', 'succeeded', 'failed']) {
    assert.equal(isQueuedJobCancellable({ state }), false, `${state} jobs must not be cancellable from the UI`);
  }
});

test('history inspection cannot replace a live tracked job', () => {
  const tracked = { id: 'active', state: 'running' };
  const saved = { id: 'saved', state: 'succeeded' };
  assert.equal(isActiveJob(tracked), true);
  assert.equal(canInspectJobHistoryRecord(tracked, saved), false);
  assert.equal(canInspectJobHistoryRecord(tracked, tracked), true);
  assert.equal(canInspectJobHistoryRecord({ ...tracked, state: 'succeeded' }, saved), true);
});

test('a stale active check cannot regress a terminal tracked job', () => {
  const completed = { id: 'runtime-job', operation: 'runtime.pixal3d.setup', state: 'succeeded' };
  const stale = { ...completed, state: 'running' };
  assert.equal(preferTrackedTerminalJob(completed, stale), completed);
  assert.equal(preferTrackedTerminalJob(stale, completed), completed);
});

test('checking terminal state defers tracked job completion to the poller', () => {
  const tracked = { id: 'runtime-job', operation: 'runtime.pixal3d.setup', state: 'running' };
  const completed = { ...tracked, state: 'succeeded' };
  assert.equal(shouldDeferTrackedJobCompletion(tracked, completed), true);
  assert.equal(shouldDeferTrackedJobCompletion(tracked, { ...completed, id: 'other' }), false);
  assert.equal(shouldDeferTrackedJobCompletion(completed, completed), false);
  assert.equal(shouldDeferTrackedJobCompletion(tracked, { ...tracked, state: 'running' }), false);
});

test('latest-request guard discards superseded and invalidated results', () => {
  const guard = new LatestRequestGuard();
  const first = guard.begin();
  const second = guard.begin();
  assert.equal(guard.isCurrent(first), false);
  assert.equal(guard.isCurrent(second), true);
  guard.invalidate();
  assert.equal(guard.isCurrent(second), false);
});

test('job polling retries use capped exponential backoff', () => {
  assert.deepEqual(
    [1, 2, 3, 4, 5, 6, 7].map(jobPollRetryDelay),
    [500, 1000, 2000, 4000, 8000, 10000, 10000],
  );
  assert.equal(jobPollRetryDelay(Number.NaN), 500);
});

test('job history uses a bounded read-only dashboard request', async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  globalThis.fetch = async (input, init = {}) => {
    calls.push({ path: String(input), init });
    return Response.json([{ id: 'job-history', state: 'succeeded' }]);
  };
  try {
    const api = new MeshHarborApi();
    assert.equal((await api.jobs(500))[0].id, 'job-history');
    await api.jobs(-2);
    await api.jobs(Number.NaN);
    assert.deepEqual(calls.map(({ path }) => path), [
      '/api/v1/jobs?limit=100',
      '/api/v1/jobs?limit=1',
      '/api/v1/jobs?limit=20',
    ]);
    assert.equal(calls[0].init.method, undefined);
    assert.equal(calls[0].init.body, undefined);
    assert.equal(new Headers(calls[0].init.headers).get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('cancellation notices describe queued, running, and finished outcomes', () => {
  assert.equal(cancelJobNotice({ state: 'cancelled' }), 'The queued job was cancelled.');
  assert.equal(cancelJobNotice({ state: 'queued' }), 'This job is still queued. Try cancelling it again.');
  for (const state of ['running', 'validating', 'cancelling']) {
    assert.match(cancelJobNotice({ state }), /cannot be interrupted safely/);
  }
  for (const state of ['succeeded', 'failed']) {
    assert.match(cancelJobNotice({ state }), /finished before cancellation took effect/);
  }
});

test('cancelJob reports a successfully cancelled queued job without an extra read', async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  globalThis.fetch = async (input, init = {}) => {
    calls.push({ path: String(input), init });
    if (String(input) === '/api/v1/jobs/job-cancel/cancel') {
      return Response.json({ id: 'job-cancel', state: 'cancelled' });
    }
    throw new Error('Unexpected request: ' + input);
  };
  try {
    const api = new MeshHarborApi();
    const result = await api.cancelJob('job-cancel');
    assert.equal(result.state, 'cancelled');
    assert.deepEqual(calls.map(({ path }) => path), ['/api/v1/jobs/job-cancel/cancel']);
    assert.equal(calls[0].init.method, 'POST');
    assert.equal(new Headers(calls[0].init.headers).get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('cancelJob refreshes server state if the worker wins the queued-cancel race', async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  globalThis.fetch = async (input, init = {}) => {
    calls.push({ path: String(input), init });
    if (String(input) === '/api/v1/jobs/job-race/cancel') {
      return Response.json({ error: 'conflict: This operation has already started.' }, { status: 409 });
    }
    if (String(input) === '/api/v1/jobs/job-race') {
      return Response.json({ id: 'job-race', operation: 'mesh.simplify', state: 'running', progress: 5 });
    }
    throw new Error('Unexpected request: ' + input);
  };
  try {
    const api = new MeshHarborApi();
    const latest = await api.cancelJob('job-race');
    assert.equal(latest.state, 'running');
    assert.deepEqual(calls.map(({ path }) => path), [
      '/api/v1/jobs/job-race/cancel',
      '/api/v1/jobs/job-race',
    ]);
    assert.equal(calls[0].init.method, 'POST');
    assert.equal(new Headers(calls[1].init.headers).get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('cancelJob preserves the server error when the job is still queued', async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (input) => {
    if (String(input) === '/api/v1/jobs/job-queued/cancel') {
      return Response.json({ error: 'conflict: cancellation was not applied.' }, { status: 409 });
    }
    if (String(input) === '/api/v1/jobs/job-queued') {
      return Response.json({ id: 'job-queued', operation: 'runtime.pixal3d.setup', state: 'queued', progress: 0 });
    }
    throw new Error('Unexpected request: ' + input);
  };
  try {
    const api = new MeshHarborApi();
    await assert.rejects(api.cancelJob('job-queued'), /conflict: cancellation was not applied/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('QuadriFlow installer surface stays guarded when no reviewed release is available', async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  globalThis.fetch = async (path, init = {}) => {
    calls.push({ path, init });
    if (path === '/api/v1/backends/quadriflow/install') {
      return Response.json({ error: 'This build does not include an approved QuadriFlow release artifact.' }, { status: 422 });
    }
    throw new Error('Unexpected request: ' + path);
  };
  try {
    const api = new MeshHarborApi();
    await assert.rejects(api.installQuadriflowBackend(), /does not include an approved QuadriFlow release artifact/);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].init.method, 'POST');
    assert.equal(new Headers(calls[0].init.headers).get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('Studio backend setup and image-generation flow downloads the generated GLB', async () => {
  const sourceAsset = { id: 'image-1', artifacts: [{ id: 'source-1', kind: 'source' }] };
  const generatedAsset = { id: 'mesh-1', artifacts: [{ id: 'generated-1', kind: 'derived' }] };
  const queuedJob = { id: 'job-1', state: 'queued' };
  const runtimeInfo = { model_backends: [
    { id: 'pixal3d-mlx', ready: true, installable: true },
    { id: 'pixal3d-cuda', ready: true, installable: true },
  ] };
  const succeededJob = { id: 'job-1', state: 'succeeded', result: { asset_id: 'mesh-1' } };
  const generatedGlb = new Uint8Array([0x67, 0x6c, 0x54, 0x46, 0x02]);
  const calls = [];
  const originalFetch = globalThis.fetch;

  globalThis.fetch = async (input, init = {}) => {
    const path = String(input);
    calls.push({ path, init });
    if (path === '/api/v1/runtime') return Response.json(runtimeInfo);
    if (path === '/api/v1/runtime/pixal3d-mlx/setup' || path === '/api/v1/runtime/pixal3d-cuda/setup') {
      return Response.json({ id: path.includes('cuda') ? 'cuda-setup' : 'mlx-setup', state: 'queued' }, { status: 202 });
    }
    if (path === '/api/v1/runtime/pixal3d-mlx/models' || path === '/api/v1/runtime/pixal3d-cuda/models') {
      return Response.json({ id: path.includes('cuda') ? 'cuda-models' : 'mlx-models', state: 'queued' }, { status: 202 });
    }
    if (path === '/api/v1/runtime/pixal3d-mlx/convert-naf') {
      return Response.json({ id: 'mlx-convert', state: 'queued' }, { status: 202 });
    }
    if (path === '/api/v1/assets?filename=source%20image.png') {
      return Response.json(sourceAsset);
    }
    if (path === '/api/v1/jobs' && init.method === 'POST') {
      return Response.json(queuedJob, { status: 202 });
    }
    if (path === '/api/v1/jobs/job-1') return Response.json(succeededJob);
    if (path === '/api/v1/assets/mesh-1') return Response.json(generatedAsset);
    if (path === '/api/v1/artifacts/generated-1') {
      return new Response(generatedGlb, { headers: { 'Content-Type': 'model/gltf-binary' } });
    }
    throw new Error(`Unexpected Studio API request: ${path}`);
  };

  try {
    const api = new MeshHarborApi();
    const status = await api.runtime();
    assert.equal(status.model_backends.find((backend) => backend.id === 'pixal3d-cuda').ready, true);
    await api.setupPixal3dRuntime();
    await api.installPixal3dModels();
    await api.convertPixal3dNaf();
    await api.setupPixal3dCudaRuntime();
    await api.installPixal3dCudaModels();

    const image = new Blob([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], { type: 'image/png' });
    Object.defineProperty(image, 'name', { value: 'source image.png' });
    const imported = await api.importImage(image);
    assert.equal(imported.id, 'image-1');

    const submitted = await api.createImageGenerationJob(imported.id, 'pixal3d-cuda', 1024, 50_000);
    assert.equal(submitted.id, 'job-1');
    assert.equal((await api.job(submitted.id)).state, 'succeeded');
    const mesh = await api.derivedAsset('mesh-1');
    const downloaded = await api.artifact(mesh);
    assert.deepEqual(new Uint8Array(await downloaded.arrayBuffer()), generatedGlb);

    assert.deepEqual(calls.map(({ path }) => path), [
      '/api/v1/runtime',
      '/api/v1/runtime/pixal3d-mlx/setup',
      '/api/v1/runtime/pixal3d-mlx/models',
      '/api/v1/runtime/pixal3d-mlx/convert-naf',
      '/api/v1/runtime/pixal3d-cuda/setup',
      '/api/v1/runtime/pixal3d-cuda/models',
      '/api/v1/assets?filename=source%20image.png',
      '/api/v1/jobs',
      '/api/v1/jobs/job-1',
      '/api/v1/assets/mesh-1',
      '/api/v1/artifacts/generated-1',
    ]);
    for (const { init } of calls) {
      assert.equal(new Headers(init.headers).get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
    }

    assert.deepEqual(JSON.parse(calls[2].init.body), { confirm_large_download: true, confirm_gated_access: true });
    assert.deepEqual(JSON.parse(calls[5].init.body), { confirm_large_download: true, confirm_gated_access: true });

    const upload = calls[6].init;
    assert.equal(new Headers(upload.headers).get('Content-Type'), 'image/png');
    assert.equal(upload.body.type, 'image/png');
    assert.deepEqual(JSON.parse(calls[7].init.body), {
      operation: 'image.generate',
      input: { asset_id: 'image-1' },
      parameters: { backend_id: 'pixal3d-cuda', texture_size: 1024, target_faces: 50_000 },
    });
  } finally {
    globalThis.fetch = originalFetch;
  }
});


test('Studio starts a derived local mesh-processing job with explicit operation parameters', async () => {
  const calls = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (input, init = {}) => {
    const path = String(input);
    calls.push({ path, init });
    if (path === '/api/v1/jobs' && init.method === 'POST') {
      return Response.json({ id: 'mesh-job-1', state: 'queued' }, { status: 202 });
    }
    throw new Error('Unexpected Studio API request: ' + path);
  };
  try {
    const api = new MeshHarborApi();
    const job = await api.createMeshOperationJob(
      'mesh-asset-1',
      'render.optimize',
      { overdraw_threshold: 1.05 },
    );
    assert.equal(job.id, 'mesh-job-1');
    assert.equal(calls.length, 1);
    assert.equal(new Headers(calls[0].init.headers).get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
    assert.deepEqual(JSON.parse(calls[0].init.body), {
      operation: 'render.optimize',
      input: { asset_id: 'mesh-asset-1' },
      parameters: { overdraw_threshold: 1.05 },
    });
  } finally {
    globalThis.fetch = originalFetch;
  }
});



test('Studio submits atlas options and part separation through the shared job API', async () => {
  const requests = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (input, init = {}) => {
    requests.push({ path: String(input), init });
    return Response.json({ id: 'mesh-op-job', state: 'queued' }, { status: 202 });
  };
  try {
    const api = new MeshHarborApi();
    await api.createMeshOperationJob('mesh-asset-1', 'mesh.separate_parts', {});
    await api.createMeshOperationJob('mesh-asset-1', 'uv.unwrap', { resolution: 1024, padding: 8 });
    assert.deepEqual(requests.map(({ path }) => path), ['/api/v1/jobs', '/api/v1/jobs']);
    assert.deepEqual(JSON.parse(requests[0].init.body), {
      operation: 'mesh.separate_parts',
      input: { asset_id: 'mesh-asset-1' },
      parameters: {},
    });
    assert.deepEqual(JSON.parse(requests[1].init.body), {
      operation: 'uv.unwrap',
      input: { asset_id: 'mesh-asset-1' },
      parameters: { resolution: 1024, padding: 8 },
    });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('retopology submits a job and downloads quad OBJ separately from GLB preview', async () => {
  const quad = { id: 'quad/one', kind: 'quad_topology', mime_type: 'model/obj' };
  const glb = { id: 'preview', kind: 'derived', mime_type: 'model/gltf-binary' };
  const asset = { mime_type: 'model/gltf-binary', artifacts: [quad, glb] };
  assert.equal(quadTopologyArtifact(asset), quad);
  assert.equal(quadTopologyArtifact({ artifacts: [] }), undefined);
  assert.equal(quadTopologyArtifact(null), undefined);
  assert.equal(quadTopologyArtifact({ artifacts: [{ ...quad, kind: 'quad_obj' }] }).id, quad.id);
  const calls = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (path, init) => {
    calls.push({ path, init });
    if (path === '/api/v1/jobs') return Response.json({ id: 'retopo-1', state: 'queued' });
    if (path === '/api/v1/artifacts/preview') return new Response('GLB preview');
    if (path === '/api/v1/artifacts/quad%2Fone') return new Response('v 0 0 0\nf 1 2 3 4\n');
    throw new Error('Unexpected request: ' + path);
  };
  try {
    const api = new MeshHarborApi();
    await api.createMeshOperationJob('source', 'mesh.retopology');
    assert.deepEqual(JSON.parse(calls[0].init.body), {
      operation: 'mesh.retopology', input: { asset_id: 'source' }, parameters: {},
    });
    assert.equal(await (await api.artifact(asset)).text(), 'GLB preview');
    assert.equal(await (await api.downloadArtifact(quad)).text(), 'v 0 0 0\nf 1 2 3 4\n');
    assert.throws(() => api.artifact({ artifacts: [quad] }), /no downloadable artifact/);
    for (const { init } of calls) {
      assert.equal(new Headers(init.headers).get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
    }
    globalThis.fetch = async () => new Response('unavailable', { status: 404 });
    await assert.rejects(api.downloadArtifact(quad), /Artifact download failed \(404\)/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
