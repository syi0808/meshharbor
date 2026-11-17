import assert from 'node:assert/strict';
import test from 'node:test';
import { decodeSidebarPanel, formatVersionSizes, inspectorForAsset, shouldHighlightExport, toggleSidebarPanel } from './workbench-state.ts';

test('sidebar stores only known panels and can switch or close', () => {
  assert.equal(decodeSidebarPanel('assets'), 'assets');
  assert.equal(decodeSidebarPanel('jobs'), 'jobs');
  for (const value of [null, '', 'true', 'unknown']) assert.equal(decodeSidebarPanel(value), null);
  assert.equal(toggleSidebarPanel(null, 'assets'), 'assets');
  assert.equal(toggleSidebarPanel('assets', 'jobs'), 'jobs');
  assert.equal(toggleSidebarPanel('jobs', 'jobs'), null);
});

test('only tools that match the selected input can open', () => {
  assert.equal(inspectorForAsset('tools', 'mesh'), 'tools');
  assert.equal(inspectorForAsset('optimize', 'mesh'), 'optimize');
  assert.equal(inspectorForAsset('generation', 'image'), 'generation');
  assert.equal(inspectorForAsset('generation', 'mesh'), null);
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
  assert.equal(shouldHighlightExport('result', null, true, true), false);
  assert.equal(shouldHighlightExport('result', null, false, false), false);
});
