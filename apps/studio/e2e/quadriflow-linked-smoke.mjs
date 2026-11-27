import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';
import { quadriflowCube } from './quadriflow-fixture.mjs';

assert.equal(process.argv.length, 3, 'Pass the path to a MeshHarbor binary built with bundled-quadriflow');
const executable = resolve(process.argv[2]);
const env = { ...process.env, PATH: '' };

test('linked QuadriFlow works with a clean store and no sidecar', () => {
  const root = mkdtempSync(join(tmpdir(), 'meshharbor-linked-quad-'));
  const store = join(root, 'store');
  const run = (...args) => JSON.parse(execFileSync(executable, ['--data-dir', store, ...args], {
    encoding: 'utf8', env, timeout: 200_000,
  }));
  try {
    const doctor = run('local', 'doctor');
    const backend = doctor.quadriflow_backend;
    assert.equal(backend.status, 'ready');
    assert.equal(backend.available, true);
    assert.equal(backend.managed, false);
    assert.equal(backend.sidecar_source, null);
    assert.equal(backend.install_available, false);
    assert(doctor.operations.includes('mesh.retopology'));

    for (const textured of [false, true]) {
      const stem = textured ? 'seamed' : 'geometry';
      const sourcePath = join(root, `${stem}.glb`);
      const previewPath = join(root, `${stem}-preview.glb`);
      const objPath = join(root, `${stem}-quad.obj`);
      const source = quadriflowCube({ textured });
      writeFileSync(sourcePath, source);
      const asset = run('local', 'import', sourcePath);
      const targetArgs = textured ? ['--target-quads', '384'] : [];
      const preflight = run('local', 'retopology-preflight', asset.id, ...targetArgs);
      assert.equal(preflight.requested_quads, textured ? 384 : null);
      const result = run(
        'local', 'process-mesh', asset.id, '--operation', 'mesh.retopology', ...targetArgs,
        '--output', previewPath, '--quad-output', objPath,
      );
      assert.equal(result.report.requested_quads, preflight.requested_quads);
      assert(result.asset.artifacts.some(artifact => artifact.kind === 'quad_topology'));
      const records = readFileSync(objPath, 'utf8').trim().split(/\r?\n/);
      const faces = records.filter(line => line.startsWith('f '));
      assert(faces.length > 0);
      assert.equal(faces.length, result.report.output_quads);
      assert(faces.every(line => /^f(?: [1-9]\d*){4}$/.test(line)));
      assert(records.every(line => /^(v|f) /.test(line)));
      const preview = readFileSync(previewPath);
      assert.equal(preview.toString('ascii', 0, 4), 'glTF');
      const gltf = JSON.parse(preview.toString('utf8', 20, 20 + preview.readUInt32LE(12)));
      const [primitive] = gltf.meshes[0].primitives;
      assert.equal(gltf.accessors[primitive.indices].count, faces.length * 6);
      assert.equal(gltf.images?.length ?? 0, textured ? 2 : 0);
      if (textured) {
        assert(result.report.appearance_projection.projection_coverage >= 0.995);
      }
      assert.deepEqual(readFileSync(sourcePath), source);
    }
    assert(!existsSync(join(store, 'backends', 'quadriflow')));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('linked worker rejects unvalidated geometry', () => {
  const root = mkdtempSync(join(tmpdir(), 'meshharbor-linked-quad-invalid-'));
  try {
    const invalid = {
      protocol_version: 1,
      positions: [[0, 0, 0], [1, 0, 0], [0, 1, 0]],
      indices: [0, 1, 2],
      options: { target_edge_ratio: 0.25, iterations: 1 },
    };
    const worker = spawnSync(executable, ['--internal-quadriflow-worker', '12'], {
      cwd: root, input: JSON.stringify(invalid), encoding: 'utf8', env, timeout: 10_000,
    });
    assert.equal(worker.error, undefined);
    assert.equal(worker.status, 1);
    assert.match(worker.stderr, /closed, watertight/);
    assert(!existsSync(join(root, 'output.obj')));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
