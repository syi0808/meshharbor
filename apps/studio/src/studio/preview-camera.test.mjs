import assert from 'node:assert/strict';
import test from 'node:test';
import {
  previewFrameFromBounds,
  shouldPreserveComparisonCamera,
  shouldPreserveSourceTransition,
} from './preview-camera.ts';

test('canonical source bounds center both sides in the same source-space frame', () => {
  const frame = previewFrameFromBounds({
    min: [10, -2, 5],
    max: [14, 2, 9],
  });

  assert.deepEqual(frame, {
    origin: [12, 0, 7],
    target: [0, 0, 0],
    size: [4, 4, 4],
    maxDimension: 4,
  });
});

test('an explicit world origin is shared by source and result rather than independently recentered', () => {
  const sourceFrame = previewFrameFromBounds({
    min: [10, -2, 5],
    max: [14, 2, 9],
  }, [0, 0, 0]);

  assert.deepEqual(sourceFrame?.origin, [0, 0, 0]);
  assert.deepEqual(sourceFrame?.target, [12, 0, 7]);
});

test('camera persists only within a comparison group, including late source-to-pair discovery', () => {
  assert.equal(shouldPreserveComparisonCamera('source-a:result-a', 'source-a:result-a'), true);
  assert.equal(shouldPreserveComparisonCamera(null, 'source-a:result-a'), false);
  assert.equal(shouldPreserveComparisonCamera('source-a:result-a', 'source-a:result-b'), false);
  assert.equal(shouldPreserveSourceTransition('source-a', 'source-a'), true);
  assert.equal(shouldPreserveSourceTransition('other-asset', 'source-a'), false);
  assert.equal(shouldPreserveSourceTransition(null, 'source-a'), false);
});

test('malformed bounds are rejected rather than producing a broken camera fit', () => {
  assert.equal(previewFrameFromBounds({ min: [2, 0, 0], max: [1, 1, 1] }), null);
  assert.equal(previewFrameFromBounds({ min: [0, 0, Number.NaN], max: [1, 1, 1] }), null);
});
