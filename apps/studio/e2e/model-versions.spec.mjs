import { expect } from '@playwright/test';
import { test } from './studio-fixture.mjs';
import { disconnectedPartsGlb } from './textured-glb.mjs';

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

  await page.getByRole('button', { name: 'More tools' }).click();
  const tools = page.getByRole('complementary', { name: 'Mesh tools' });
  await tools.getByRole('button', { name: 'Cleanup', exact: true }).click();
  await tools.getByRole('button', { name: 'Run Cleanup' }).click();
  await expect(selected.getByTitle('panels-cleaned.glb')).toBeVisible();
  await expect(inventory.getByTitle('panels-cleaned.glb')).toHaveCount(0);
  await expect(card).toHaveCount(1);

  await tools.getByRole('button', { name: 'Separate connected parts', exact: true }).click();
  await tools.getByRole('button', { name: 'Run Separate connected parts' }).click();
  const separationName = selected.getByTitle(/panels-cleaned-.*\.glb/);
  await expect(separationName).toBeVisible();
  await expect(card).toHaveCount(1);
  const partMode = selected.getByRole('radio', { name: 'Part highlighting' });
  const canvas = selected.locator('canvas');
  await expect(partMode).toBeVisible();
  const materialFrame = await canvas.screenshot();
  await partMode.click();
  const highlight = selected.getByRole('combobox', { name: 'Highlight part' });
  await expect(highlight.locator('option')).toHaveText(['All parts', 'Part 1', 'Part 2']);
  await expect.poll(async () => (await canvas.screenshot()).equals(materialFrame)).toBe(false);
  const bothParts = await canvas.screenshot();
  await highlight.selectOption('1');
  await expect.poll(async () => (await canvas.screenshot()).equals(bothParts)).toBe(false);
  await selected.getByRole('radio', { name: 'Material view' }).click();
  await expect(highlight).toHaveCount(0);

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
