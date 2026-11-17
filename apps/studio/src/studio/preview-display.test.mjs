import assert from 'node:assert/strict';
import test from 'node:test';
import { applyTriangleWireframe, isLocalPreviewResource, restorePreviewMaterials } from './preview-display.ts';

test('triangle wireframe swaps preview materials and restores original references without changing geometry', () => {
  const sharedMaterial = { name: 'shared material', wireframe: false };
  const materialOne = { name: 'first material', wireframe: false };
  const materialTwo = { name: 'second material', wireframe: false };
  const multiMaterials = [materialOne, materialTwo];
  const geometryOne = { positions: [0, 1, 2], indices: [0, 1, 2] };
  const geometryTwo = { positions: [3, 4, 5], indices: [0, 1, 2] };
  const geometryThree = { positions: [6, 7, 8], indices: [0, 1, 2] };
  const firstMesh = { geometry: geometryOne, material: sharedMaterial };
  const groupedMesh = { geometry: geometryTwo, material: multiMaterials };
  const secondSharedMesh = { geometry: geometryThree, material: sharedMaterial };
  const meshes = [firstMesh, groupedMesh, secondSharedMesh];
  const wireframeMaterial = { name: 'temporary triangle wireframe', wireframe: true };
  const originals = new Map();

  applyTriangleWireframe(meshes, wireframeMaterial, originals);

  assert.strictEqual(firstMesh.material, wireframeMaterial);
  assert.deepEqual(groupedMesh.material, [wireframeMaterial, wireframeMaterial]);
  assert.equal(groupedMesh.material.length, multiMaterials.length);
  assert.strictEqual(secondSharedMesh.material, wireframeMaterial);
  assert.strictEqual(originals.get(firstMesh), sharedMaterial);
  assert.strictEqual(originals.get(groupedMesh), multiMaterials);
  assert.strictEqual(originals.get(secondSharedMesh), sharedMaterial);
  assert.equal(sharedMaterial.wireframe, false);
  assert.equal(materialOne.wireframe, false);
  assert.equal(materialTwo.wireframe, false);
  assert.strictEqual(firstMesh.geometry, geometryOne);
  assert.strictEqual(groupedMesh.geometry, geometryTwo);
  assert.strictEqual(secondSharedMesh.geometry, geometryThree);

  const appliedGroupedMaterials = groupedMesh.material;
  applyTriangleWireframe(meshes, wireframeMaterial, originals);
  assert.strictEqual(groupedMesh.material, appliedGroupedMaterials, 'repeated toggles must not replace the captured original');

  restorePreviewMaterials(originals);
  assert.strictEqual(firstMesh.material, sharedMaterial);
  assert.strictEqual(groupedMesh.material, multiMaterials);
  assert.strictEqual(secondSharedMesh.material, sharedMaterial);
  assert.equal(originals.size, 0);

  applyTriangleWireframe(meshes, wireframeMaterial, originals);
  restorePreviewMaterials(originals);
  assert.strictEqual(groupedMesh.material, multiMaterials, 'a later toggle cycle still restores the same original array');
});


test('preview can resolve embedded image bytes without allowing remote resources', () => {
  const origin = 'http://127.0.0.1:7432';
  assert.equal(isLocalPreviewResource('data:image/png;base64,AA==', origin), true);
  assert.equal(isLocalPreviewResource(`blob:${origin}/72bad282-a191-447f-9e56-05d26a461671`, origin), true);
  for (const url of ['/image.png', `${origin}/image.png`, 'https://example.com/image.png',
    'blob:https://example.com/id', `blob:${origin}0/wrong-port`]) {
    assert.equal(isLocalPreviewResource(url, origin), false);
  }
});
