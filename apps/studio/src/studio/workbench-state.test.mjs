import assert from 'node:assert/strict';
import test from 'node:test';
import { decodeInventoryOpen, formatVersionSizes, runtimeSetupStage, shouldHighlightExport } from './workbench-state.ts';

test('the shared dock keeps existing open and closed preferences', () => {
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

test('exports make file size changes visible even if bounds round to the same number', () => {
  assert.match(formatVersionSizes(3_325_012, 3_349_188), /3\.17 → 3\.19 MiB/);
  assert.equal(formatVersionSizes(4096, 4097), '4.00 → 4.00 KiB (+1 B)');
  assert.equal(formatVersionSizes(4097, 4096), '4.00 → 4.00 KiB (−1 B)');
  assert.equal(formatVersionSizes(22, 17), '22 → 17 B');
  assert.equal(formatVersionSizes(NaN, 17), null);
  assert.equal(formatVersionSizes(12, -1), null);
});

test('only a connected, idle result review emphasizes export', () => {
  assert.equal(shouldHighlightExport('result', false, true), true);
  assert.equal(shouldHighlightExport('source', false, true), false);
  assert.equal(shouldHighlightExport('result', true, true), false);
  assert.equal(shouldHighlightExport('result', false, false), false);
});
