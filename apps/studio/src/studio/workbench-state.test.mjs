import assert from 'node:assert/strict';
import test from 'node:test';
import { decodeInventoryOpen, formatVersionSizes, inspectorForAsset, runtimeSetupStage, shouldHighlightExport } from './workbench-state.ts';

test('inventory keeps existing open and closed preferences after Jobs moves left', () => {
  for (const value of ['assets', 'jobs', null]) assert.equal(decodeInventoryOpen(value), true);
  for (const value of ['closed', '', 'true', 'unknown']) assert.equal(decodeInventoryOpen(value), false);
});

test('runtime setup distinguishes the environment, model files, and inference readiness', () => {
  assert.equal(runtimeSetupStage({ installable: false }), 'unsupported');
  assert.equal(runtimeSetupStage({ installable: true }), 'environment');
  assert.equal(runtimeSetupStage({ installable: true, environment_ready: true }), 'models');
  assert.equal(runtimeSetupStage({ installable: true, environment_ready: true, models_downloaded: true, naf_conversion_required: true }), 'conversion');
  assert.equal(runtimeSetupStage({ installable: true, environment_ready: true, models_downloaded: true }), 'incomplete');
  assert.equal(runtimeSetupStage({ installable: true, ready: true }), 'ready');
});

test('mesh tools follow the selected input and generation is independent', () => {
  assert.equal(inspectorForAsset('tools', 'mesh'), 'tools');
  assert.equal(inspectorForAsset('optimize', 'mesh'), 'optimize');
  assert.equal(inspectorForAsset('generation', 'image'), 'generation');
  assert.equal(inspectorForAsset('generation', 'mesh'), 'generation');
  assert.equal(inspectorForAsset('generation', undefined), 'generation');
  assert.equal(inspectorForAsset('jobs', undefined), 'jobs');
  assert.equal(inspectorForAsset('jobs', 'image'), 'jobs');
  assert.equal(inspectorForAsset('optimize', 'image'), null);
  assert.equal(inspectorForAsset('tools', undefined), null);
});

test('exports make file size changes visible even if bounds round to the same number', () => {
  assert.match(formatVersionSizes(3_325_012, 3_349_188), /3\.17 → 3\.19 MiB/);
  assert.equal(formatVersionSizes(4096, 4097), '4.00 → 4.00 KiB (+1 B)');
  assert.equal(formatVersionSizes(4097, 4096), '4.00 → 4.00 KiB (−1 B)');
  assert.equal(formatVersionSizes(22, 17), '22 → 17 B');
  assert.equal(formatVersionSizes(NaN, 17), null);
  assert.equal(formatVersionSizes(12, -1), null);
});

test('only a connected, idle result review emphasizes export', () => {
  assert.equal(shouldHighlightExport('result', null, false, true), true);
  assert.equal(shouldHighlightExport('source', null, false, true), false);
  assert.equal(shouldHighlightExport('result', 'optimize', false, true), false);
  assert.equal(shouldHighlightExport('result', 'jobs', false, true), true);
  assert.equal(shouldHighlightExport('result', null, true, true), false);
  assert.equal(shouldHighlightExport('result', null, false, false), false);
});
