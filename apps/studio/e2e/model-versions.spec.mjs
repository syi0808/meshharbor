import { expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { PNG } from 'pngjs';
import { test } from './studio-fixture.mjs';
import { disconnectedPartsGlb } from './textured-glb.mjs';

function canvasPixel(image, { x, y }) {
  const offset = (image.width * y + x) * 4;
  return [...image.data.subarray(offset, offset + 3)];
}

function partCenter(image, matches) {
  let count = 0;
  let sumX = 0;
  let sumY = 0;
  for (let y = 0; y < image.height - 64; y += 1) {
    for (let x = 0; x < image.width; x += 1) {
      const offset = (image.width * y + x) * 4;
      if (!matches(image.data[offset], image.data[offset + 1], image.data[offset + 2])) continue;
      count += 1;
      sumX += x;
      sumY += y;
    }
  }
  expect(count).toBeGreaterThan(10);
  return { x: Math.round(sumX / count), y: Math.round(sumY / count) };
}

test('version history load errors use a toast and leave a retry action', async ({ page, studioOrigin }) => {
  let unavailable = true;
  await page.route('**/api/v1/models/*/versions?limit=20', route => unavailable
    ? route.fulfill({ status: 503, json: { error: 'Version store unavailable.' } })
    : route.continue());
  await page.goto(studioOrigin);
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.getByRole('button', { name: 'Open GLB', exact: true }).click(),
  ]);
  await chooser.setFiles({ name: 'panels.glb', mimeType: 'model/gltf-binary', buffer: disconnectedPartsGlb() });

  await page.getByRole('button', { name: 'Version history', exact: true }).click();
  const history = page.getByRole('dialog', { name: 'Version history' });
  const retry = history.getByRole('button', { name: 'Retry loading versions' });
  await expect(retry).toBeVisible();
  await expect(history).not.toContainText('Version store unavailable.');
  await expect(page.getByRole('region', { name: /Notifications/ }).getByText('Version store unavailable.')).toBeVisible();

  unavailable = false;
  await retry.click();
  await expect(history.getByRole('button', { name: 'Preview version 1' })).toBeVisible();
});

test('edits stay on one inventory card and restored versions survive reload', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  const inventory = page.getByRole('complementary', { name: 'Inventory' });
  const selected = page.getByRole('region', { name: 'Selected asset' });
  const card = inventory.getByRole('button', { name: /^panels\.glb/ });
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    inventory.getByRole('button', { name: 'Open GLB' }).click(),
  ]);
  const sourceBytes = disconnectedPartsGlb();
  const upload = page.waitForResponse('**/api/v1/assets?filename=panels.glb');
  await chooser.setFiles({ name: 'panels.glb', mimeType: 'model/gltf-binary', buffer: sourceBytes });
  const sourceAsset = await (await upload).json();
  await expect(card).toBeVisible();
  await expect(selected.getByTitle('panels.glb')).toBeVisible();

  await page.getByRole('button', { name: 'Cleanup', exact: true }).click();
  await page.getByRole('button', { name: 'Run Cleanup' }).click();
  await expect(selected.getByTitle('panels-cleaned.glb')).toBeVisible();
  await expect(inventory.getByTitle('panels-cleaned.glb')).toHaveCount(0);
  await expect(card).toHaveCount(1);

  await page.getByRole('button', { name: 'Render optimization', exact: true }).click();
  const threshold = page.getByRole('spinbutton', { name: 'Overdraw threshold' });
  await threshold.fill('0.9');
  await expect(page.getByRole('button', { name: 'Run Render optimization' })).toBeDisabled();
  await threshold.fill('1.2');
  const renderRequest = page.waitForRequest(request => request.method() === 'POST' && request.url().endsWith('/api/v1/jobs'));
  await page.getByRole('button', { name: 'Run Render optimization' }).click();
  expect((await renderRequest).postDataJSON().parameters).toEqual({ overdraw_threshold: 1.2 });
  const optimizedName = selected.getByTitle('panels-cleaned-render-optimized.glb', { exact: true });
  await expect(optimizedName).toBeVisible();
  await expect(card).toHaveCount(1);

  const history = page.getByRole('dialog', { name: 'Version history' });
  const openHistory = selected.getByRole('button', { name: 'Version history' });
  const historyRow = number => history.getByRole('listitem').filter({
    has: page.getByRole('button', { name: `Preview version ${number}`, exact: true }),
  });
  await openHistory.click();
  await expect(historyRow(3)).toContainText('Current');
  await expect(historyRow(3)).toContainText('From v2');
  await expect(historyRow(2)).toContainText('Cleanup');
  await history.getByRole('button', { name: 'Preview version 1' }).click();
  await expect(selected.getByTitle('panels.glb', { exact: true })).toBeVisible();
  await openHistory.click();
  await expect(historyRow(1)).toContainText('Viewing');
  await expect(historyRow(3)).toContainText('Current');
  await history.getByRole('button', { name: 'Restore version 1' }).click();
  await expect(page.getByText('Version 1 restored. Other versions are unchanged.')).toBeVisible();
  await page.reload();
  await expect(selected.getByTitle('panels.glb', { exact: true })).toBeVisible();
  await expect(card).toHaveCount(1);
  await openHistory.click();
  await expect(historyRow(1)).toContainText('Current');
  await history.getByRole('button', { name: 'Restore version 3' }).click();
  await expect(optimizedName).toBeVisible();
  await expect(card).toHaveCount(1);

  await inventory.getByRole('button', { name: 'Filters', exact: true }).click();
  await inventory.getByRole('checkbox', { name: 'Show all mesh versions' }).check();
  await expect(inventory.getByTitle('panels-cleaned.glb', { exact: true })).toBeVisible();
  await expect(inventory.getByTitle('panels-cleaned-render-optimized.glb', { exact: true })).toBeVisible();
  await inventory.getByRole('button', { name: 'Find by ID' }).click();
  const id = inventory.getByLabel('Asset or artifact ID');
  await id.fill(sourceAsset.id);
  await inventory.getByRole('button', { name: 'Open asset', exact: true }).click();
  await expect(selected.getByTitle('panels.glb', { exact: true })).toBeVisible();
  const artifactId = sourceAsset.artifacts[0].id;
  await id.fill(artifactId);
  const [download] = await Promise.all([
    page.waitForEvent('download'),
    inventory.getByRole('button', { name: 'Download artifact' }).click(),
  ]);
  expect(download.suggestedFilename()).toBe(`${artifactId}.glb`);
  expect(await readFile(await download.path())).toEqual(sourceBytes);
});

test('Studio can still highlight and isolate parts on a legacy split asset', async ({ page, studioOrigin }) => {
  const name = 'legacy-parts.glb';
  await page.route(`**/api/v1/assets?filename=${name}`, async route => {
    const response = await route.fetch();
    const asset = await response.json();
    asset.metadata.operation = 'mesh.separate_parts';
    asset.metadata.report = { output_primitives: 2, grouped_components: 0 };
    await route.fulfill({ response, json: asset });
  });
  await page.goto(studioOrigin);
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.getByRole('button', { name: 'Open GLB', exact: true }).click(),
  ]);
  await chooser.setFiles({ name, mimeType: 'model/gltf-binary', buffer: disconnectedPartsGlb({ separated: true }) });
  const selected = page.getByRole('region', { name: 'Selected asset' });
  await expect(selected.getByTitle(name, { exact: true })).toBeVisible();
  const partMode = selected.getByRole('radio', { name: 'Part highlighting' });
  const canvas = selected.locator('canvas');
  await expect(partMode).toBeVisible();
  const materialFrame = await canvas.screenshot();
  await partMode.click();
  const highlight = selected.getByRole('combobox', { name: 'Highlight part' });
  const isolate = selected.getByRole('button', { name: 'Isolate selected part' });
  await expect(highlight.locator('option')).toHaveText(['All parts', 'Part 1', 'Part 2']);
  await expect(isolate).toBeDisabled();
  await expect.poll(async () => (await canvas.screenshot()).equals(materialFrame)).toBe(false);
  const bothParts = await canvas.screenshot();
  const bothColors = PNG.sync.read(await canvas.screenshot({ scale: 'css' }));
  const orange = partCenter(bothColors, (red, green, blue) => red > green * 1.2 && green > blue * 1.15);
  const blue = partCenter(bothColors, (red, green, blue) => blue > red * 1.2 && blue > green * 1.2);
  const readPixel = async point => canvasPixel(PNG.sync.read(await canvas.screenshot({ scale: 'css' })), point);
  const background = canvasPixel(bothColors, { x: 0, y: 0 });
  await canvas.click({ position: orange });
  await expect(highlight).toHaveValue('0');
  await expect.poll(async () => (await canvas.screenshot()).equals(bothParts)).toBe(false);
  await expect.poll(() => readPixel(blue)).not.toEqual(background);
  await isolate.click();
  await expect(isolate).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(() => readPixel(blue)).toEqual(background);
  await expect.poll(() => readPixel(orange)).not.toEqual(background);

  await highlight.selectOption('1');
  await expect.poll(() => readPixel(orange)).toEqual(background);
  await expect.poll(() => readPixel(blue)).not.toEqual(background);
  await canvas.click({ position: orange });
  await expect(highlight).toHaveValue('all');
  await expect(isolate).toBeDisabled();
  await expect.poll(() => readPixel(orange)).not.toEqual(background);
  await canvas.click({ position: blue });
  await expect(highlight).toHaveValue('1');
  await isolate.click();
  await selected.getByRole('radio', { name: 'Material view' }).click();
  await expect(highlight).toHaveCount(0);
  await expect.poll(() => readPixel(orange)).not.toEqual(background);

  await partMode.click();
  await highlight.selectOption('all');
  const box = await canvas.boundingBox();
  await page.mouse.move(box.x + orange.x, box.y + orange.y);
  await page.mouse.down();
  await page.mouse.move(box.x + orange.x + 24, box.y + orange.y, { steps: 3 });
  await page.mouse.move(box.x + orange.x, box.y + orange.y, { steps: 3 });
  await page.mouse.up();
  await expect(highlight).toHaveValue('all');

});
