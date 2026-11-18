import { expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { PNG } from 'pngjs';
import { test } from './studio-fixture.mjs';
import { fixtureTriangles, inspectTexturedGlb, texturedGlb } from './textured-glb.mjs';

function patchColor(red, green, blue) {
  if (red > green + 35 && red > blue + 35) return 'red';
  if (green > red + 35 && green > blue + 35) return 'green';
  if (blue > red + 35 && blue > green + 35) return 'blue';
  if (Math.min(red, green) > blue + 45) return 'yellow';
  return null;
}

function compareMaterialFrames(originalScreenshot, resultScreenshot) {
  const original = PNG.sync.read(originalScreenshot);
  const result = PNG.sync.read(resultScreenshot);
  expect([result.width, result.height]).toEqual([original.width, original.height]);
  const originalColors = { red: 0, green: 0, blue: 0, yellow: 0 };
  const resultColors = { ...originalColors };
  let coloredPixels = 0;
  let changedPixels = 0;
  for (let offset = 0; offset < original.data.length; offset += 4) {
    const originalColor = patchColor(original.data[offset], original.data[offset + 1], original.data[offset + 2]);
    const resultColor = patchColor(result.data[offset], result.data[offset + 1], result.data[offset + 2]);
    if (originalColor) originalColors[originalColor] += 1;
    if (resultColor) resultColors[resultColor] += 1;
    if (!originalColor && !resultColor) continue;
    coloredPixels += 1;
    const delta = Math.max(
      Math.abs(original.data[offset] - result.data[offset]),
      Math.abs(original.data[offset + 1] - result.data[offset + 1]),
      Math.abs(original.data[offset + 2] - result.data[offset + 2]),
    );
    if (delta > 18) changedPixels += 1;
  }
  for (const color of Object.keys(originalColors)) {
    expect(originalColors[color], `Original renders its ${color} texture patch`).toBeGreaterThan(50);
    expect(resultColors[color], `Result renders its ${color} texture patch`).toBeGreaterThan(50);
  }
  expect(changedPixels / coloredPixels, 'Texture patches match at the same camera, allowing minor rasterization differences')
    .toBeLessThan(0.03);
}

async function previewFrames(page) {
  const materialView = page.getByRole('button', { name: 'Material view' });
  const wireframeView = page.getByRole('button', { name: 'Triangle wireframe' });
  await expect(materialView).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByText('Loading preview…', { exact: true })).toBeHidden();
  const canvas = page.getByLabel('Interactive 3D model preview').locator('canvas');
  const material = await canvas.screenshot();
  await wireframeView.click();
  const wireframe = PNG.sync.read(await canvas.screenshot());
  await materialView.click();
  let edgePixels = 0;
  for (let offset = 0; offset < wireframe.data.length; offset += 4) {
    const red = wireframe.data[offset];
    const green = wireframe.data[offset + 1];
    const blue = wireframe.data[offset + 2];
    if (red > 80 && green > red + 8 && green > blue + 4) edgePixels += 1;
  }
  return { material, edgePixels };
}

async function exportSelectedGlb(page, filename) {
  const [download] = await Promise.all([
    page.waitForEvent('download'),
    page.getByRole('button', { name: 'Export selected GLB' }).click(),
  ]);
  expect(download.suggestedFilename()).toBe(filename);
  return readFile(await download.path());
}

test('Studio rebakes a supported GLB UV atlas and exports both PBR maps', async ({ page, studioOrigin }) => {
  const sourceName = 'atlas-source.glb';
  const resultName = 'atlas-source-atlas-unwrapped.glb';
  const source = texturedGlb({ singleMaterial: true });
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.getByRole('button', { name: 'Open GLB', exact: true }).click(),
  ]);
  await chooser.setFiles({ name: sourceName, mimeType: 'model/gltf-binary', buffer: source });
  const selected = page.getByRole('region', { name: 'Selected asset' });
  await expect(selected.getByTitle(sourceName, { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'More tools' }).click();
  await page.getByRole('button', { name: 'UV atlas rebake', exact: true }).click();
  await page.getByRole('button', { name: 'Run UV atlas rebake' }).click();
  await expect(selected.getByTitle(resultName, { exact: true })).toBeVisible();

  const before = inspectTexturedGlb(source);
  const after = inspectTexturedGlb(await exportSelectedGlb(page, resultName));
  expect(after.triangles).toBe(before.triangles);
  const material = after.appearance.materials[0].pbrMetallicRoughness;
  for (const role of ['baseColorTexture', 'metallicRoughnessTexture']) {
    const texture = after.appearance.textures[material[role].index];
    expect(texture.source).toBeGreaterThanOrEqual(before.appearance.images.length);
    const image = after.appearance.images[texture.source];
    expect(image.mimeType).toBe('image/png');
    const { width, height } = PNG.sync.read(image.bytes);
    expect(Math.min(width, height)).toBeGreaterThan(8);
  }
});

test('Studio optimizes a textured GLB without changing its materials or textures', async ({ page, studioOrigin }, testInfo) => {
  const sourceName = 'textured-panels.glb';
  const resultName = 'textured-panels-simplified.glb';
  const source = texturedGlb();
  const selected = page.getByRole('region', { name: 'Selected asset' });
  await test.step('Import the textured GLB through the Studio file chooser', async () => {
    await page.goto(studioOrigin);
    await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
    const [chooser] = await Promise.all([
      page.waitForEvent('filechooser'),
      page.getByRole('button', { name: 'Open GLB', exact: true }).click(),
    ]);
    await chooser.setFiles({ name: sourceName, mimeType: 'model/gltf-binary', buffer: source });
    await expect(selected.getByTitle(sourceName, { exact: true })).toBeVisible();
    const material = page.getByRole('button', { name: 'Material view' });
    const solid = page.getByRole('button', { name: 'Solid view' });
    await expect(material).toHaveAttribute('aria-pressed', 'true');
    await solid.click();
    await expect(solid).toHaveAttribute('aria-pressed', 'true');
    await page.getByRole('button', { name: 'Fit view' }).click();
    await material.click();
    await expect(material).toHaveAttribute('aria-pressed', 'true');
  });

  const original = page.getByRole('radio', { name: 'Original' });
  const result = page.getByRole('radio', { name: 'Result' });
  await test.step('Run Optimize against the local Rust engine', async () => {
    await page.getByRole('button', { name: 'Optimize', exact: true }).click();
    const inspector = page.getByRole('complementary', { name: 'Optimize mesh' });
    const inspectorWidth = () => inspector.evaluate(element => element.getBoundingClientRect().width);
    const initialWidth = await inspectorWidth();
    const handle = await page.getByRole('separator', { name: 'Resize settings panel' }).boundingBox();
    expect(handle).not.toBeNull();
    const x = handle.x + handle.width / 2;
    const y = handle.y + handle.height / 2;
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x + 56, y, { steps: 6 });
    await page.mouse.up();
    await expect.poll(inspectorWidth).toBeGreaterThan(initialWidth);
    const targetInput = page.getByRole('spinbutton', { name: 'Triangle target' });
    await expect(targetInput).toHaveCSS('height', '28px');
    await expect(page.getByRole('slider', { name: 'Triangle target' })).toBeVisible();
    await targetInput.fill('240');
    await page.getByRole('button', { name: 'Optimize mesh' }).click();
    await expect(result).toBeChecked({ timeout: 20_000 });
    await expect(selected.getByTitle(resultName, { exact: true })).toBeVisible();
    const library = page.locator('#workbench-sidebar-dock');
    await expect.poll(() => library.evaluate(element => element.scrollWidth - element.clientWidth)).toBe(0);
  });

  await test.step('Compare rendered material views and verify the original download', async () => {
    await original.click();
    await expect(original).toBeChecked();
    await expect(selected.getByTitle(sourceName, { exact: true })).toBeVisible();
    const originalFrames = await previewFrames(page);
    expect(await exportSelectedGlb(page, sourceName)).toEqual(source);

    await result.click();
    await expect(result).toBeChecked();
    await expect(selected.getByTitle(resultName, { exact: true })).toBeVisible();
    const resultFrames = await previewFrames(page);
    await testInfo.attach('original-material-preview', { body: originalFrames.material, contentType: 'image/png' });
    await testInfo.attach('result-material-preview', { body: resultFrames.material, contentType: 'image/png' });
    compareMaterialFrames(originalFrames.material, resultFrames.material);
    expect(resultFrames.edgePixels, 'Result renders triangle edges').toBeGreaterThan(100);
    expect(resultFrames.edgePixels, 'Result shows the simplified topology, not the same original mesh')
      .toBeLessThan(originalFrames.edgePixels * 0.9);
  });

  await test.step('Inspect the exported result GLB for topology reduction and preserved appearance', async () => {
    const input = inspectTexturedGlb(source);
    const output = inspectTexturedGlb(await exportSelectedGlb(page, resultName));
    expect(input.triangles).toBe(fixtureTriangles);
    expect(output.triangles).toBeGreaterThan(0);
    expect(output.triangles).toBeLessThan(input.triangles);
    testInfo.annotations.push({ type: 'Triangles', description: `${input.triangles} → ${output.triangles}` });
    await expect(page.getByRole('group', { name: 'Original to result comparison' }))
      .toContainText(`${input.triangles} → ${output.triangles}`);
    expect(output.appearance).toEqual(input.appearance);
  });
});
