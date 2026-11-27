import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';

// Use the project's TypeScript compiler when installed (Node 20+), with a
// native type-stripping fallback for minimal Node 24 test environments.
let MeshHarborApi, assetComparisonPairFromJob, assetComparisonPairForSelection, assetComparisonTarget, shouldAutoSelectComparisonResult, resolveComparisonAsset, quadTopologyArtifact, cancelJobNotice, isJobCancellable, isActiveJob, canInspectJobHistoryRecord, preferTrackedTerminalJob, shouldDeferTrackedJobCompletion, LatestRequestGuard, jobPollRetryDelay, importAssetBatch, MAX_ASSET_IMPORT_FILES, MAX_IMAGE_IMPORT_BYTES, MAX_GLB_IMPORT_BYTES, planAssetImportUiCompletion, jobResultCardModel, jobReportMatchesAsset;
try {
  const ts = await import('typescript');
  const source = await readFile(new URL('./api.ts', import.meta.url), 'utf8');
  const compiled = ts.default.transpileModule(source, {
    compilerOptions: { module: ts.default.ModuleKind.ESNext, target: ts.default.ScriptTarget.ES2022 },
  }).outputText;
  ({ MeshHarborApi, assetComparisonPairFromJob, assetComparisonPairForSelection, assetComparisonTarget, shouldAutoSelectComparisonResult, resolveComparisonAsset, quadTopologyArtifact, cancelJobNotice, isJobCancellable, isActiveJob, canInspectJobHistoryRecord, preferTrackedTerminalJob, shouldDeferTrackedJobCompletion, LatestRequestGuard, jobPollRetryDelay, importAssetBatch, MAX_ASSET_IMPORT_FILES, MAX_IMAGE_IMPORT_BYTES, MAX_GLB_IMPORT_BYTES, planAssetImportUiCompletion, jobResultCardModel, jobReportMatchesAsset } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`));
} catch (error) {
  if (error?.code !== 'ERR_MODULE_NOT_FOUND') throw error;
  ({ MeshHarborApi, assetComparisonPairFromJob, assetComparisonPairForSelection, assetComparisonTarget, shouldAutoSelectComparisonResult, resolveComparisonAsset, quadTopologyArtifact, cancelJobNotice, isJobCancellable, isActiveJob, canInspectJobHistoryRecord, preferTrackedTerminalJob, shouldDeferTrackedJobCompletion, LatestRequestGuard, jobPollRetryDelay, importAssetBatch, MAX_ASSET_IMPORT_FILES, MAX_IMAGE_IMPORT_BYTES, MAX_GLB_IMPORT_BYTES, planAssetImportUiCompletion, jobResultCardModel, jobReportMatchesAsset } = await import('./api.ts'));
}

test('multi-file import validates mixed files, uploads sequentially, keeps going after failures, and reports progress', async () => {
  const files = [
    { name: 'Model.GLB', size: 10 },
    { name: 'notes.txt', size: 1 },
    { name: 'too-large.png', size: MAX_IMAGE_IMPORT_BYTES + 1 },
    { name: 'broken.jpg', size: 20 },
    { name: 'texture.PNG', size: 30 },
  ];
  const uploadOrder = [];
  const progress = [];
  let activeUploads = 0;
  let maxActiveUploads = 0;
  const result = await importAssetBatch(files, async (file, kind) => {
    uploadOrder.push([file.name, kind]);
    activeUploads += 1;
    maxActiveUploads = Math.max(maxActiveUploads, activeUploads);
    await new Promise((resolve) => setTimeout(resolve, 1));
    activeUploads -= 1;
    if (file.name === 'broken.jpg') throw new Error('Server rejected the image.');
    return { id: file.name };
  }, (event) => progress.push(event));

  assert.deepEqual(uploadOrder, [['Model.GLB', 'mesh'], ['broken.jpg', 'image'], ['texture.PNG', 'image']]);
  assert.equal(maxActiveUploads, 1, 'files must be uploaded one at a time');
  assert.deepEqual(result.assets, [{ id: 'Model.GLB' }, { id: 'texture.PNG' }]);
  assert.deepEqual(result.failures.map(({ index, fileName, message }) => [index, fileName, message]), [
    [1, 'notes.txt', 'Unsupported file type. Choose a .glb, .png, .jpg, or .jpeg file.'],
    [2, 'too-large.png', 'File exceeds the 16 MiB image limit.'],
    [3, 'broken.jpg', 'Server rejected the image.'],
  ]);
  assert.equal(result.skippedCount, 0);
  assert.equal(result.attemptedCount, 3);
  assert.deepEqual(progress, [
    { current: 1, total: 3, fileName: 'Model.GLB' },
    { current: 2, total: 3, fileName: 'broken.jpg' },
    { current: 3, total: 3, fileName: 'texture.PNG' },
  ]);
});

test('multi-file import caps work to the first ten entries and does not retry a failed POST', async () => {
  const files = Array.from({ length: MAX_ASSET_IMPORT_FILES + 2 }, (_, index) => ({
    name: `model-${index}.glb`,
    size: 1,
  }));
  const uploadOrder = [];
  const result = await importAssetBatch(files, async (file) => {
    uploadOrder.push(file.name);
    if (file.name === 'model-4.glb') throw new Error('temporary failure');
    return file.name;
  });

  assert.equal(uploadOrder.length, MAX_ASSET_IMPORT_FILES);
  assert.deepEqual(uploadOrder, files.slice(0, MAX_ASSET_IMPORT_FILES).map((file) => file.name));
  assert.equal(result.assets.length, MAX_ASSET_IMPORT_FILES - 1);
  assert.deepEqual(result.failures.map((failure) => failure.fileName), ['model-4.glb']);
  assert.equal(result.skippedCount, 2);
  assert.equal(result.attemptedCount, MAX_ASSET_IMPORT_FILES);
});

test('multi-file import reports attempted failures without retrying them', async () => {
  let calls = 0;
  const result = await importAssetBatch([{ name: 'unconfirmed.glb', size: 1 }], async () => {
    calls += 1;
    throw new Error('The response was lost after the server write.');
  });

  assert.equal(calls, 1);
  assert.equal(result.attemptedCount, 1);
  assert.deepEqual(result.assets, []);
  assert.deepEqual(result.failures.map((failure) => failure.fileName), ['unconfirmed.glb']);
});

test('multi-file import refuses empty and oversized assets before calling the uploader', async () => {
  const files = [
    { name: 'empty.glb', size: 0 },
    { name: 'too-large.glb', size: MAX_GLB_IMPORT_BYTES + 1 },
    { name: 'ok.jpg', size: MAX_IMAGE_IMPORT_BYTES },
  ];
  const calls = [];
  const result = await importAssetBatch(files, async (file, kind) => {
    calls.push([file.name, kind]);
    return file.name;
  });

  assert.deepEqual(calls, [['ok.jpg', 'image']]);
  assert.deepEqual(result.assets, ['ok.jpg']);
  assert.deepEqual(result.failures.map(({ fileName, message }) => [fileName, message]), [
    ['empty.glb', 'The file is empty.'],
    ['too-large.glb', 'File exceeds the 512 MiB GLB limit.'],
  ]);
});

test('asset import completion keeps browser filters and selection changed during a batch', () => {
  const before = {
    selectedId: 'asset-a',
    filters: { query: 'old', kind: 'mesh', origin: 'original' },
    searchInput: 'old draft',
    browserInteractionRevision: 4,
    selectionInteractionRevision: 2,
  };
  assert.deepEqual(planAssetImportUiCompletion(before, {
    ...before,
    selectedId: 'asset-b',
    filters: { query: 'new', kind: 'image', origin: 'derived' },
    searchInput: 'new draft',
    browserInteractionRevision: 5,
    selectionInteractionRevision: 3,
  }), { resetFilters: false, selectFirstImportedAsset: false });

  assert.deepEqual(planAssetImportUiCompletion(before, {
    ...before,
    filters: { ...before.filters },
  }), { resetFilters: true, selectFirstImportedAsset: true });

  assert.deepEqual(planAssetImportUiCompletion(before, {
    ...before,
    browserInteractionRevision: 5,
  }), { resetFilters: false, selectFirstImportedAsset: true });

  assert.deepEqual(planAssetImportUiCompletion(before, {
    ...before,
    selectionInteractionRevision: 3,
  }), { resetFilters: true, selectFirstImportedAsset: false });
});

test('Studio offers stop for queued and running jobs that are not finalizing', () => {
  assert.equal(isJobCancellable({ state: 'queued' }), true);
  for (const state of ['validating', 'cancelling', 'cancelled', 'succeeded', 'failed']) {
    assert.equal(isJobCancellable({ state, operation: 'runtime.pixal3d.install_models' }), false);
  }
  for (const operation of ['model.generate', 'image.generate', 'mesh.simplify', 'mesh.cleanup', 'uv.unwrap', 'render.optimize', 'runtime.pixal3d.install_models']) {
    assert.equal(isJobCancellable({ state: 'running', operation }), true);
    assert.equal(isJobCancellable({ state: 'validating', operation }), false);
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

test('comparison controls stay pinned to the selected asset lineage', () => {
  const assets = [
    { id: 'input-a', source_asset_id: null },
    { id: 'result-a1', source_asset_id: 'input-a' },
    { id: 'result-a2', source_asset_id: 'input-a' },
    { id: 'unrelated-c', source_asset_id: null },
  ];
  const pinned = { inputAssetId: 'input-a', resultAssetId: 'result-a1' };

  assert.deepEqual(assetComparisonPairForSelection(assets, 'input-a', pinned), pinned);
  assert.deepEqual(assetComparisonPairForSelection(assets, 'result-a1', pinned), pinned);
  assert.deepEqual(assetComparisonPairForSelection(assets, 'result-a2', pinned), {
    inputAssetId: 'input-a',
    resultAssetId: 'result-a2',
  });
  assert.equal(assetComparisonPairForSelection(assets, 'unrelated-c', pinned), null);
  assert.equal(assetComparisonPairForSelection(assets, 'input-a', pinned, 'input-a'), null);
  assert.equal(assetComparisonPairForSelection(assets, null, pinned), null);
  assert.equal(assetComparisonTarget(pinned, 'source'), 'input-a');
  assert.equal(assetComparisonTarget(pinned, 'result'), 'result-a1');
});

test('a completed job selects its result only if its input is still selected', () => {
  const pair = { inputAssetId: 'input-a', resultAssetId: 'result-a' };
  assert.equal(shouldAutoSelectComparisonResult(pair, 'input-a'), true);
  assert.equal(shouldAutoSelectComparisonResult(pair, 'result-a'), false);
  assert.equal(shouldAutoSelectComparisonResult(pair, 'unrelated-c'), false);
  assert.equal(shouldAutoSelectComparisonResult(null, 'input-a'), false);
});

test('saved job comparison pairs require its exact result and direct input lineage', () => {
  const job = { input_asset_id: 'input-a', result: { asset_id: 'result-a1' } };
  const result = { id: 'result-a1', source_asset_id: 'input-a' };
  assert.deepEqual(assetComparisonPairFromJob(job, result), {
    inputAssetId: 'input-a',
    resultAssetId: 'result-a1',
  });
  assert.equal(assetComparisonPairFromJob(job, { ...result, id: 'other-result' }), null);
  assert.equal(assetComparisonPairFromJob(job, { ...result, source_asset_id: 'other-input' }), null);
  assert.equal(assetComparisonPairFromJob(job, { ...result, source_asset_id: null }), null);
  assert.equal(assetComparisonPairFromJob(job, { id: result.id }), null);
  assert.equal(assetComparisonPairFromJob({ result: job.result }, result), null);
});

test('comparison side loader fetches only missing assets and validates identity', async () => {
  const pair = { inputAssetId: 'input-a', resultAssetId: 'result-a1' };
  const input = { id: 'input-a', source_asset_id: null };
  const result = { id: 'result-a1', source_asset_id: 'input-a' };
  const cached = await resolveComparisonAsset(pair, 'source', [input], async () => {
    throw new Error('cached comparison side should not be fetched');
  });
  assert.equal(cached, input);

  const requested = [];
  const fetched = await resolveComparisonAsset(pair, 'source', [], async (assetId) => {
    requested.push(assetId);
    return input;
  });
  assert.equal(fetched, input);
  assert.deepEqual(requested, ['input-a']);
  await assert.rejects(
    resolveComparisonAsset(pair, 'source', [], async () => ({ id: 'wrong-id' })),
    /did not match the requested asset/,
  );
  await assert.rejects(
    resolveComparisonAsset(pair, 'result', [], async () => ({ ...result, source_asset_id: 'other-input' })),
    /does not match this comparison source/,
  );
  await assert.rejects(
    resolveComparisonAsset(pair, 'result', [], async () => ({ id: result.id, source_asset_id: null })),
    /does not match this comparison source/,
  );
  await assert.rejects(
    resolveComparisonAsset(pair, 'result', [], async () => ({ id: result.id })),
    /does not match this comparison source/,
  );
});

test('job polling retries use capped exponential backoff', () => {
  assert.deepEqual(
    [1, 2, 3, 4, 5, 6, 7].map(jobPollRetryDelay),
    [500, 1000, 2000, 4000, 8000, 10000, 10000],
  );
  assert.equal(jobPollRetryDelay(Number.NaN), 500);
});

test('asset pages use bounded encoded read-only requests', async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  globalThis.fetch = async (input, init = {}) => {
    calls.push({ path: String(input), init });
    return Response.json({ items: [], next_cursor: null });
  };
  try {
    const api = new MeshHarborApi();
    await api.assetPage(500, 'asset/id ?', { query: 'oak %_', kind: 'mesh', derived: true });
    await api.assetPage(-2);
    await api.assetPage(Number.NaN);
    assert.deepEqual(calls.map(({ path }) => {
      const url = new URL(path, 'http://localhost');
      return {
        path: url.pathname,
        limit: url.searchParams.get('limit'),
        cursor: url.searchParams.get('cursor'),
        query: url.searchParams.get('q'),
        kind: url.searchParams.get('kind'),
        derived: url.searchParams.get('derived'),
      };
    }), [
      { path: '/api/v1/assets', limit: '100', cursor: 'asset/id ?', query: 'oak %_', kind: 'mesh', derived: 'true' },
      { path: '/api/v1/assets', limit: '1', cursor: null, query: null, kind: null, derived: null },
      { path: '/api/v1/assets', limit: '50', cursor: null, query: null, kind: null, derived: null },
    ]);
    assert.equal(calls[0].init.method, undefined);
    assert.equal(calls[0].init.body, undefined);
    assert.equal(new Headers(calls[0].init.headers).get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
  } finally {
    globalThis.fetch = originalFetch;
  }
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
    await api.jobs(20, { includeActive: true });
    await api.jobs(20, { before: 'job+last' });
    assert.deepEqual(calls.map(({ path }) => path), [
      '/api/v1/jobs?limit=100',
      '/api/v1/jobs?limit=1',
      '/api/v1/jobs?limit=20',
      '/api/v1/jobs?limit=20&include_active=true',
      '/api/v1/jobs?limit=20&before=job%2Blast',
    ]);
    assert.equal(calls[0].init.method, undefined);
    assert.equal(calls[0].init.body, undefined);
    assert.equal(new Headers(calls[0].init.headers).get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('cancellation notices describe queued, running, and finished outcomes', () => {
  assert.equal(cancelJobNotice({ state: 'cancelled' }), 'Job stopped.');
  assert.match(cancelJobNotice({ state: 'queued' }), /still queued/);
  assert.match(cancelJobNotice({ state: 'cancelling' }), /Stop requested/);
  for (const state of ['running', 'validating']) {
    assert.match(cancelJobNotice({ state }), /could not be stopped/);
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
    if (path === '/api/v1/runtime/quadriflow/install') {
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
    if (path === '/api/v1/runtime/pixal3d-mlx/install' || path === '/api/v1/runtime/pixal3d-cuda/install') {
      return Response.json({ id: path.includes('cuda') ? 'cuda-install' : 'mlx-install', state: 'queued' }, { status: 202 });
    }
    if (path === '/api/v1/assets?filename=source%20image.png') {
      return Response.json(sourceAsset);
    }
    if (path === '/api/v1/generation' && init.method === 'POST') {
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
    await api.installCatalogModel('pixal3d-mlx', true);
    await api.installCatalogModel('pixal3d-cuda', true);

    const image = new Blob([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], { type: 'image/png' });
    Object.defineProperty(image, 'name', { value: 'source image.png' });
    const imported = await api.importImage(image);
    assert.equal(imported.id, 'image-1');

    const submitted = await api.createModelGenerationJob({ kind: 'image', asset_id: imported.id }, 'pixal3d-cuda', { texture_size: 1024, target_faces: 50_000 });
    assert.equal(submitted.id, 'job-1');
    assert.equal((await api.job(submitted.id)).state, 'succeeded');
    const mesh = await api.derivedAsset('mesh-1');
    const downloaded = await api.artifact(mesh);
    assert.deepEqual(new Uint8Array(await downloaded.arrayBuffer()), generatedGlb);

    assert.deepEqual(calls.map(({ path }) => path), [
      '/api/v1/runtime',
      '/api/v1/runtime/pixal3d-mlx/install',
      '/api/v1/runtime/pixal3d-cuda/install',
      '/api/v1/assets?filename=source%20image.png',
      '/api/v1/generation',
      '/api/v1/jobs/job-1',
      '/api/v1/assets/mesh-1',
      '/api/v1/artifacts/generated-1',
    ]);
    for (const { init } of calls) {
      assert.equal(new Headers(init.headers).get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
    }

    for (const { init } of calls.slice(1, 3)) {
      assert.equal(init.method, 'POST');
      assert.deepEqual(JSON.parse(init.body), { confirmed: true, confirm_gated_access: true, package: 'base' });
    }

    const upload = calls[3].init;
    assert.equal(new Headers(upload.headers).get('Content-Type'), 'image/png');
    assert.equal(upload.body.type, 'image/png');
    assert.deepEqual(JSON.parse(calls[4].init.body), {
      input: { kind: 'image', asset_id: 'image-1' },
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
    if (path === '/api/v1/assets/mesh-asset-1/optimize-rendering' && init.method === 'POST') {
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
    assert.deepEqual(JSON.parse(calls[0].init.body), { overdraw_threshold: 1.05 });
  } finally {
    globalThis.fetch = originalFetch;
  }
});



test('Studio submits atlas options to the UV rebake endpoint', async () => {
  const requests = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (input, init = {}) => {
    requests.push({ path: String(input), init });
    return Response.json({ id: 'mesh-op-job', state: 'queued' }, { status: 202 });
  };
  try {
    const api = new MeshHarborApi();
    await api.createMeshOperationJob('mesh-asset-1', 'uv.unwrap', { resolution: 1024, padding: 8 });
    assert.deepEqual(requests.map(({ path }) => path), ['/api/v1/assets/mesh-asset-1/unwrap-uv']);
    assert.deepEqual(JSON.parse(requests[0].init.body), { resolution: 1024, padding: 8 });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('retopology preflight checks the mesh without creating a job', async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  const report = {
    input_vertices: 12, input_triangles: 12, solver_vertices: 8, requested_quads: null,
    coincident_boundary_vertices_joined: true, appearance_projection_required: true,
  };
  globalThis.fetch = async (path, init) => {
    calls.push({ path, init });
    return Response.json(report);
  };
  try {
    const api = new MeshHarborApi();
    assert.deepEqual(await api.preflightRetopology('source/id'), report);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].path, '/api/v1/assets/source%2Fid/retopology-preflight');
    assert.equal(calls[0].init.method, 'POST');
    const headers = new Headers(calls[0].init.headers);
    assert.equal(headers.get('X-MeshHarbor-Dashboard'), 'local-ui-v1');
    assert.equal(headers.get('Authorization'), null);
    assert.deepEqual(JSON.parse(calls[0].init.body), {});
    await api.preflightRetopology('source/id', 384);
    assert.deepEqual(JSON.parse(calls[1].init.body), { target_quads: 384 });
    globalThis.fetch = async () => Response.json({ error: 'Water-tightness check failed.' }, { status: 422 });
    await assert.rejects(api.preflightRetopology('open'), /Water-tightness check failed/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('retopology report distinguishes automatic density from a missing count', () => {
  const job = { id: 'job-retopology', operation: 'mesh.retopology', state: 'succeeded' };
  const requestedQuads = report => jobResultCardModel({ ...job, result: { report } }).metrics
    .find(metric => metric.label === 'Requested quads').value;
  assert.equal(requestedQuads({ requested_quads: null }), 'Automatic');
  assert.equal(requestedQuads({ requested_quads: 12 }), '12');
  assert.equal(requestedQuads({}), 'Not reported');
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
    if (path === '/api/v1/assets/source/retopology') return Response.json({ id: 'retopo-1', state: 'queued' });
    if (path === '/api/v1/artifacts/preview') return new Response('GLB preview');
    if (path === '/api/v1/artifacts/quad%2Fone') return new Response('v 0 0 0\nf 1 2 3 4\n');
    throw new Error('Unexpected request: ' + path);
  };
  try {
    const api = new MeshHarborApi();
    await api.createMeshOperationJob('source', 'mesh.retopology');
    assert.deepEqual(JSON.parse(calls[0].init.body), {});
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


test('operation result cards expose operation-specific metrics and every report warning', () => {
  const fixtures = [
    {
      operation: 'image.generate',
      result: {
        asset_id: 'asset-result', backend_id: 'pixal3d-mlx', seed: 42, resolution: 512,
        texture_size: 1024, target_faces: 100000, elapsed_ms: 12345,
      },
      labels: ['Backend', 'Seed', 'Resolution', 'Texture size', 'Target faces', 'Elapsed time'],
      expectedWarnings: [],
      expectedWarningsState: 'unavailable',
      expectedWarningsMessage: 'Image generation does not include a warning list in its saved result.',
    },
    {
      operation: 'mesh.simplify',
      report: {
        triangles_before: 100, triangles_after: 55, target_triangles: 55,
        geometric_error: 0.0123, optimized_primitives: 2, preserved: ['positions'],
        warnings: ['Simplification note.', '<b>warning-looking markup</b>'],
      },
      labels: ['Triangles before', 'Triangles after', 'Triangle limit', 'Geometric error', 'Optimized primitives'],
      expectedWarnings: ['Simplification note.', '<b>warning-looking markup</b>'],
    },
    {
      operation: 'mesh.cleanup',
      report: {
        triangles_before: 100, triangles_after: 90, repeated_index_triangles_removed: 1,
        zero_area_triangles_removed: 2, duplicate_triangles_removed: 2, simplified_triangles_removed: 5,
        modified_primitives: 1,
        preserved: ['materials'], warnings: ['Cleanup warning.'],
      },
      labels: ['Repeated-index triangles removed', 'Zero-area triangles removed', 'Duplicate triangles removed', 'Triangles removed by simplification', 'Modified primitives'],
      expectedWarnings: ['Cleanup warning.'],
    },
    {
      operation: 'render.optimize',
      report: {
        triangles: 100, processed_primitives: 4, reordered_primitives: 3,
        skipped_translucent_primitives: 1, vertex_cache_size: 16,
        vertex_cache_misses_before: 40, vertex_cache_misses_after: 25, overdraw_threshold: 1.05,
        preserved: ['translucent order'], warnings: ['Render warning.'],
      },
      labels: ['Triangles', 'Processed primitives', 'Reordered primitives', 'Translucent primitives skipped', 'Vertex cache size', 'Cache misses before', 'Cache misses after', 'Overdraw threshold'],
      expectedWarnings: ['Render warning.'],
    },
    {
      operation: 'mesh.compress',
      report: {
        input_bytes: 4096, output_bytes: 2048, compressed_buffer_views: 2,
        uncompressed_buffer_views: 1, bytes_saved: 2048, extension: 'KHR_meshopt_compression',
        extension_required: true, warnings: ['Compression warning.'],
      },
      labels: ['Input size', 'Output size', 'Bytes saved', 'Compressed buffer views', 'Uncompressed buffer views', 'Required extension', 'Extension required'],
      expectedWarnings: ['Compression warning.'],
    },
    {
      operation: 'mesh.separate_parts',
      report: {
        triangles_before: 3, connected_parts: 2, triangles_per_part: [2, 1],
        method: 'indexed_components', warnings: ['Parts warning.'],
      },
      labels: ['Triangles before', 'Connected parts', 'Method'],
      expectedWarnings: ['Parts warning.'],
    },
    {
      operation: 'uv.unwrap',
      report: {
        source_triangles: 8, output_triangles: 8, source_vertices: 12, output_vertices: 18,
        atlas_width: 1024, atlas_height: 1024, chart_count: 4, covered_texels: 5000,
        gutter_texels: 200, preserved_attributes: ['positions'], rebaked_maps: ['baseColor'],
        warnings: ['UV warning.'],
      },
      labels: ['Source triangles', 'Output triangles', 'Source vertices', 'Output vertices', 'Atlas width', 'Atlas height', 'Charts', 'Covered texels', 'Gutter texels'],
      expectedWarnings: ['UV warning.'],
    },
    {
      operation: 'mesh.retopology',
      report: {
        algorithm: 'QuadriFlow', input_vertices: 20, input_triangles: 10, requested_quads: 5,
        output_vertices: 8, output_quads: 5, preview_triangles: 10, warnings: ['Retopology warning.'],
        appearance_projection: {
          source_triangles: 10, target_triangles: 10, output_triangles: 10, output_vertices: 8,
          atlas_width: 512, atlas_height: 512, chart_count: 2, covered_texels: 2000, gutter_texels: 50,
          projection_attempted_texels: 1000, projection_covered_texels: 875, projection_coverage: 0.875,
          ambiguous_texels: 4, maximum_projection_distance: 0.02, projection_distance_limit: 0.04,
          rebaked_maps: ['baseColor'], warnings: ['Projection warning.'],
        },
      },
      labels: ['Input vertices', 'Input triangles', 'Requested quads', 'Output vertices', 'Output quads', 'Preview triangles', 'Algorithm', 'Appearance · Projection coverage'],
      expectedWarnings: ['Retopology warning.', 'Appearance projection: Projection warning.'],
    },
  ];

  for (const fixture of fixtures) {
    const model = jobResultCardModel({
      id: 'job-result-1',
      operation: fixture.operation,
      state: 'succeeded',
      input_asset_id: 'asset-input',
      progress: 100,
      result: fixture.result ?? { asset_id: 'asset-result', report: fixture.report },
      created_at: '1',
      updated_at: '2',
    });
    assert.ok(model, fixture.operation);
    assert.equal(model.reportAvailable, true);
    assert.ok(fixture.labels.every((label) => model.metrics.some((metric) => metric.label === label)), fixture.operation);
    assert.deepEqual(model.warnings, fixture.expectedWarnings, fixture.operation);
    assert.equal(model.warningsState, fixture.expectedWarningsState ?? 'available', fixture.operation);
    if (fixture.expectedWarningsMessage) assert.equal(model.warningsMessage, fixture.expectedWarningsMessage);
    assert.equal(model.jobId, 'job-result-1');
    assert.equal(model.inputAssetId, 'asset-input');
    assert.equal(model.resultAssetId, 'asset-result');
  }
});

test('result cards fail closed for missing, malformed, non-finite, and unsafe metrics without inventing zeroes', () => {
  const malformed = jobResultCardModel({
    id: 'job-malformed',
    operation: 'mesh.cleanup',
    state: 'succeeded',
    progress: 100,
    result: {
      report: {
        triangles_before: '12',
        triangles_after: Number.POSITIVE_INFINITY,
        repeated_index_triangles_removed: Number.MAX_SAFE_INTEGER + 1,
        warnings: [],
      },
    },
    created_at: '1',
    updated_at: '2',
  });
  assert.ok(malformed);
  assert.equal(malformed.metrics.find((metric) => metric.label === 'Triangles before').value, 'Not reported');
  assert.equal(malformed.metrics.find((metric) => metric.label === 'Triangles after').value, 'Not reported');
  assert.equal(malformed.metrics.find((metric) => metric.label === 'Repeated-index triangles removed').value, 'Not reported');
  assert.equal(malformed.metrics.find((metric) => metric.label === 'Duplicate triangles removed').value, 'Not reported');
  assert.equal(malformed.warningsState, 'empty');
  assert.deepEqual(malformed.warnings, []);
  assert.equal(malformed.lists[0].state, 'unavailable');

  const missing = jobResultCardModel({
    id: 'job-missing',
    operation: 'mesh.cleanup',
    state: 'succeeded',
    progress: 100,
    result: null,
    created_at: '1',
    updated_at: '2',
  });
  assert.ok(missing);
  assert.equal(missing.reportAvailable, false);
  assert.ok(missing.metrics.every((metric) => metric.value === 'Not reported'));
  assert.equal(missing.warningsState, 'unavailable');
  assert.equal(jobResultCardModel({ id: 'active', operation: 'mesh.cleanup', state: 'running' }), null);
  assert.equal(jobResultCardModel({ id: 'unknown', operation: 'runtime.setup', state: 'succeeded' }), null);
});

test('result cards keep all warnings, bound metric lists, and preserve job asset lineage', () => {
  const warnings = Array.from({ length: 300 }, (_, index) => `Warning ${index + 1}`);
  const parts = Array.from({ length: 300 }, (_, index) => index + 1);
  const job = {
    id: 'job-parts',
    operation: 'mesh.separate_parts',
    state: 'succeeded',
    input_asset_id: 'input-1',
    progress: 100,
    result: {
      asset_id: 'result-1',
      report: { triangles_before: 300, connected_parts: 300, triangles_per_part: parts, method: 'indexed_components', warnings },
    },
    created_at: '1',
    updated_at: '2',
  };
  const model = jobResultCardModel(job);
  assert.ok(model);
  assert.equal(model.warnings.length, 300);
  assert.equal(model.warnings[299], 'Warning 300');
  assert.equal(model.lists[0].values.length, 256);
  assert.equal(model.lists[0].omittedCount, 44);
  assert.equal(model.lists[0].state, 'partial');
  assert.equal(jobReportMatchesAsset(job, 'input-1'), true);
  assert.equal(jobReportMatchesAsset(job, 'result-1'), true);
  assert.equal(jobReportMatchesAsset(job, 'another-asset'), false);
  assert.equal(jobReportMatchesAsset(job, null), false);
  assert.equal(jobReportMatchesAsset({ ...job, state: 'failed' }, 'input-1'), false);
});
