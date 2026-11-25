import { expect } from '@playwright/test';
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
  await chooser.setFiles({ name: 'panels.glb', mimeType: 'model/gltf-binary', buffer: disconnectedPartsGlb() });
  await expect(card).toBeVisible();
  await expect(selected.getByTitle('panels.glb')).toBeVisible();

  await page.getByRole('button', { name: 'Cleanup', exact: true }).click();
  await page.getByRole('button', { name: 'Run Cleanup' }).click();
  await expect(selected.getByTitle('panels-cleaned.glb')).toBeVisible();
  await expect(inventory.getByTitle('panels-cleaned.glb')).toHaveCount(0);
  await expect(card).toHaveCount(1);

  await page.getByRole('button', { name: 'Separate parts', exact: true }).click();
  await page.getByRole('button', { name: 'Run Separate connected parts' }).click();
  const separationName = selected.getByTitle(/panels-cleaned-.*\.glb/);
  await expect(separationName).toBeVisible();
  await expect(card).toHaveCount(1);
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
  await expect(selected.getByTitle('panels.glb')).toBeVisible();
  await expect(partMode).toHaveCount(0);
  await openHistory.click();
  await expect(historyRow(1)).toContainText('Viewing');
  await expect(historyRow(3)).toContainText('Current');
  await history.getByRole('button', { name: 'Restore version 1' }).click();
  await expect(page.getByText('Version 1 restored. Other versions are unchanged.')).toBeVisible();
  await page.reload();
  await expect(selected.getByTitle('panels.glb')).toBeVisible();
  await expect(card).toHaveCount(1);
  await openHistory.click();
  await expect(historyRow(1)).toContainText('Current');
  await history.getByRole('button', { name: 'Restore version 3' }).click();
  await expect(separationName).toBeVisible();
  await expect(partMode).toBeVisible();
  await expect(card).toHaveCount(1);
});
