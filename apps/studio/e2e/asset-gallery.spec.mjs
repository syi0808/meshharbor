import { expect } from '@playwright/test';
import { PNG } from 'pngjs';
import { test } from './studio-fixture.mjs';
import { texturedGlb } from './textured-glb.mjs';

test('Inventory captures material thumbnails and reuses them after reload', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const inventory = page.getByRole('complementary', { name: 'Inventory' });
  const reference = new PNG({ width: 8, height: 8 });
  reference.data.fill(255);
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    inventory.getByRole('button', { name: 'Import assets' }).click(),
  ]);
  await chooser.setFiles([
    { name: 'painted.glb', mimeType: 'model/gltf-binary', buffer: texturedGlb() },
    { name: 'coated.glb', mimeType: 'model/gltf-binary', buffer: texturedGlb({ singleMaterial: true }) },
    { name: 'reference.png', mimeType: 'image/png', buffer: PNG.sync.write(reference) },
  ]);

  const painted = inventory.getByRole('button', { name: /^painted\.glb/ });
  const coated = inventory.getByRole('button', { name: /^coated\.glb/ });
  const photo = inventory.getByRole('button', { name: /^reference\.png/ });
  const thumbnail = card => card.locator('img[data-asset-thumbnail]');
  for (const card of [painted, coated, photo]) {
    await expect(thumbnail(card)).toHaveJSProperty('naturalWidth', 320, { timeout: 15_000 });
  }
  const colors = await thumbnail(painted).evaluate(image => {
    const canvas = document.createElement('canvas');
    canvas.width = image.naturalWidth;
    canvas.height = image.naturalHeight;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
    const counts = { red: 0, green: 0, blue: 0 };
    for (let offset = 0; offset < data.length; offset += 4) {
      const [red, green, blue] = data.subarray(offset, offset + 3);
      if (red > green * 1.5 && red > blue * 1.5) counts.red++;
      if (green > red * 1.5 && green > blue * 1.5) counts.green++;
      if (blue > red * 1.5 && blue > green * 1.5) counts.blue++;
    }
    return counts;
  });
  for (const pixels of Object.values(colors)) expect(pixels).toBeGreaterThan(100);

  const bounds = await Promise.all([painted.boundingBox(), coated.boundingBox(), photo.boundingBox()]);
  expect(new Set(bounds.map(card => Math.round(card.x))).size).toBe(2);
  await painted.click();
  await expect(painted).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByRole('region', { name: 'Selected asset' }).getByTitle('painted.glb')).toBeVisible();

  await page.route('**/api/v1/artifacts/**', route => route.abort());
  await page.reload();
  for (const card of [painted, coated, photo]) {
    await expect(thumbnail(card)).toHaveJSProperty('naturalWidth', 320);
  }
  await page.unroute('**/api/v1/artifacts/**');
  await painted.click();
  await expect(painted).toHaveAttribute('aria-pressed', 'true');
  const search = inventory.getByRole('searchbox', { name: 'Search assets by filename' });
  await search.fill('painted');
  await search.press('Enter');
  await expect(painted).toBeVisible();
  await expect(coated).toHaveCount(0);
  await expect(photo).toHaveCount(0);
});
