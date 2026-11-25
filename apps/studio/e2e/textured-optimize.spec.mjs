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

async function previewMaterial(page) {
  await expect(page.getByRole('radio', { name: 'Material view' })).toBeChecked();
  await expect(page.getByText('Loading preview…', { exact: true })).toBeHidden();
  return page.getByLabel('Interactive 3D model preview').locator('canvas').screenshot();
}

async function previewFrames(page) {
  const material = await previewMaterial(page);
  const materialView = page.getByRole('radio', { name: 'Material view' });
  const wireframeView = page.getByRole('radio', { name: 'Triangle wireframe' });
  const canvas = page.getByLabel('Interactive 3D model preview').locator('canvas');
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

test('Studio renders the compressed result and reopens the exported GLB', async ({ page, studioOrigin }) => {
  const sourceName = 'meshopt-source.glb';
  const resultName = 'meshopt-source-compressed.glb';
  const source = texturedGlb();
  const selected = page.getByRole('region', { name: 'Selected asset' });
  const originalTab = page.getByRole('tab', { name: 'Original' });
  const resultTab = page.getByRole('tab', { name: 'Result' });

  async function openGlb(name, buffer) {
    const [chooser] = await Promise.all([
      page.waitForEvent('filechooser'),
      page.getByRole('button', { name: 'Open GLB', exact: true }).click(),
    ]);
    await chooser.setFiles({ name, mimeType: 'model/gltf-binary', buffer });
    await expect(selected.getByTitle(name, { exact: true })).toBeVisible();
  }

  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  await openGlb(sourceName, source);
  const originalPreview = await previewMaterial(page);

  await test.step('Render the result immediately after compression', async () => {
    await page.getByRole('button', { name: 'Compression', exact: true }).click();
    await page.getByRole('button', { name: 'Run Compression' }).click();
    await expect(resultTab).toHaveAttribute('aria-selected', 'true', { timeout: 20_000 });
    await expect(selected.getByTitle(resultName, { exact: true })).toBeVisible();
    const resultPreview = await previewMaterial(page);
    await originalTab.click();
    await expect(selected.getByTitle(sourceName, { exact: true })).toBeVisible();
    compareMaterialFrames(await previewMaterial(page), resultPreview);
  });

  await test.step('Reimport the compressed download and render it again', async () => {
    await resultTab.click();
    await expect(selected.getByTitle(resultName, { exact: true })).toBeVisible();
    const output = await exportSelectedGlb(page, resultName);
    expect(output.length).toBeLessThan(source.length);
    expect(output.toString('ascii', 0, 4)).toBe('glTF');
    const document = JSON.parse(output.toString('utf8', 20, 20 + output.readUInt32LE(12)));
    expect(document.extensionsRequired).toContain('KHR_meshopt_compression');
    await openGlb('reopened-compressed.glb', output);
    compareMaterialFrames(originalPreview, await previewMaterial(page));
  });
});

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
  await page.getByRole('button', { name: 'UV atlas rebake', exact: true }).click();
  await page.getByRole('button', { name: 'Run UV atlas rebake' }).click();
  await expect(selected.getByTitle(resultName, { exact: true })).toBeVisible();
  await selected.getByRole('button', { name: 'Operation report' }).click();
  const report = selected.getByRole('region', { name: 'Report details' });
  await expect(report.getByText('UV unwrap complete')).toBeVisible();
  await expect(report.getByRole('list', { name: 'Operation metrics' }).getByRole('listitem').first()).toBeVisible();
  await expect.poll(() => report.evaluate(element => element.scrollHeight > element.clientHeight)).toBe(true);
  await report.focus();
  await report.press('End');
  await expect.poll(() => report.evaluate(element => element.scrollTop)).toBeGreaterThan(0);

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
    const material = page.getByRole('radio', { name: 'Material view' });
    const solid = page.getByRole('radio', { name: 'Solid view' });
    await expect(material).toBeChecked();
    await material.focus();
    await material.press('ArrowRight');
    await expect(solid).toBeFocused();
    await solid.press('Space');
    await expect(solid).toBeChecked();
    await solid.click();
    await expect(solid).toBeChecked();
    await page.getByRole('button', { name: 'Fit view' }).click();
    await material.click();
    await expect(material).toBeChecked();
  });

  const original = page.getByRole('tab', { name: 'Original' });
  const result = page.getByRole('tab', { name: 'Result' });
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
    const targetInput = page.getByRole('spinbutton', { name: 'Triangle limit' });
    const run = page.getByRole('button', { name: 'Optimize mesh' });
    await expect(targetInput).toHaveCSS('height', '28px');
    await expect(page.getByRole('slider', { name: 'Triangle limit' })).toBeVisible();
    await targetInput.fill('0');
    await expect(run).toBeDisabled();
    await targetInput.fill('240');
    await run.click();
    await expect(result).toHaveAttribute('aria-selected', 'true', { timeout: 20_000 });
    await expect(selected.getByTitle(resultName, { exact: true })).toBeVisible();
    const library = page.locator('#workbench-sidebar-dock');
    await expect.poll(() => library.evaluate(element => element.scrollWidth - element.clientWidth)).toBe(0);
  });

  await test.step('Compare rendered material views and verify the original download', async () => {
    await original.click();
    await expect(original).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('tabpanel', { name: 'Original' })).toBeVisible();
    await expect(page.getByRole('banner').getByRole('button', { name: 'Export selected GLB' })).toHaveCount(0);
    await expect(selected.getByRole('group', { name: 'File actions' }).getByRole('button', { name: 'Export selected GLB' })).toBeVisible();
    await expect(selected.getByTitle(sourceName, { exact: true })).toBeVisible();
    const originalFrames = await previewFrames(page);
    expect(await exportSelectedGlb(page, sourceName)).toEqual(source);

    await result.click();
    await expect(result).toHaveAttribute('aria-selected', 'true');
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
    expect(output.triangles).toBeLessThanOrEqual(240);
    expect(output.triangles).toBeLessThan(input.triangles);
    testInfo.annotations.push({ type: 'Triangles', description: `${input.triangles} → ${output.triangles}` });
    await expect(page.getByRole('group', { name: 'Original to result comparison' }))
      .toContainText(`${input.triangles} → ${output.triangles}`);
    expect(output.appearance).toEqual(input.appearance);
  });
});

test('Studio cleanup reduces a textured GLB without a triangle limit', async ({ page, studioOrigin }) => {
  const sourceName = 'cleanup-source.glb';
  const resultName = 'cleanup-source-cleaned.glb';
  const source = texturedGlb();
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.getByRole('button', { name: 'Open GLB', exact: true }).click(),
  ]);
  await chooser.setFiles({ name: sourceName, mimeType: 'model/gltf-binary', buffer: source });
  const selected = page.getByRole('region', { name: 'Selected asset' });
  await expect(selected.getByTitle(sourceName, { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Cleanup', exact: true }).click();
  await page.getByRole('button', { name: 'Run Cleanup' }).click();
  await expect(selected.getByTitle(resultName, { exact: true })).toBeVisible();

  const input = inspectTexturedGlb(source);
  const output = inspectTexturedGlb(await exportSelectedGlb(page, resultName));
  expect(output.triangles).toBeGreaterThan(0);
  expect(output.triangles).toBeLessThan(input.triangles);
  expect(output.appearance).toEqual(input.appearance);
});
