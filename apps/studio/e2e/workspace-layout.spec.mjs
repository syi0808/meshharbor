import { expect } from '@playwright/test';
import { PNG } from 'pngjs';
import { test } from './studio-fixture.mjs';
import { texturedGlb } from './textured-glb.mjs';

async function chooseFile(page, button, name, mimeType, buffer) {
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    button.click(),
  ]);
  const upload = page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).searchParams.get('filename') === name);
  await chooser.setFiles({ name, mimeType, buffer });
  const response = await upload;
  expect(response.ok()).toBe(true);
  return response.json();
}

test('desktop docks flank the viewport, then become drawers without horizontal overflow', async ({ page, studioOrigin }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const tools = page.getByRole('group', { name: 'Model tools' });
  const inventory = page.getByRole('group', { name: 'Asset inventory' });
  await expect(tools.getByRole('button', { name: 'Optimize' })).toBeDisabled();
  await chooseFile(page, page.getByRole('button', { name: 'Open GLB', exact: true }), 'layout.glb',
    'model/gltf-binary', texturedGlb());
  const selected = page.getByRole('region', { name: 'Selected asset' });
  const rightDock = page.locator('#workbench-sidebar-dock');
  const optimize = tools.getByRole('button', { name: 'Optimize', exact: true });
  await optimize.click();
  const inspector = page.getByRole('complementary', { name: 'Optimize mesh' });
  const [settings, view, library] = await Promise.all([inspector.boundingBox(), selected.boundingBox(), rightDock.boundingBox()]);
  expect(settings.x + settings.width).toBeLessThanOrEqual(view.x);
  expect(view.x + view.width).toBeLessThanOrEqual(library.x);

  await page.setViewportSize({ width: 1024, height: 768 });
  const toolSheet = page.getByRole('dialog', { name: 'Optimize mesh' });
  await expect(toolSheet).toBeVisible();
  await expect(rightDock).toBeVisible();
  expect((await toolSheet.boundingBox()).x).toBeCloseTo(0, 0);
  expect(await page.locator('html').evaluate(element => element.scrollWidth)).toBe(1024);
  await page.keyboard.press('Escape');
  await expect(toolSheet).toBeHidden();
  await expect(optimize).toBeFocused();

  await page.setViewportSize({ width: 390, height: 700 });
  await expect(rightDock).toHaveCount(0);
  const assets = inventory.getByRole('button', { name: 'Assets' });
  expect((await assets.boundingBox()).y).toBeGreaterThanOrEqual((await selected.boundingBox()).y + (await selected.boundingBox()).height);
  await assets.click();
  const sheet = page.getByRole('dialog', { name: 'Assets' });
  await expect(sheet.getByRole('button', { name: 'Import assets' })).toBeVisible();
  await expect.poll(async () => {
    const { x, width } = await sheet.boundingBox();
    return Math.round(x + width);
  }).toBe(390);
  expect(await page.locator('html').evaluate(element => element.scrollWidth)).toBe(390);
});

test('generation uses its explicit reference and leaves the inspected GLB selected until review', async ({ page, studioOrigin }) => {
  await page.route('**/api/v1/runtime', route => route.fulfill({
    json: { platform: 'macos', status: 'ready_for_mesh_operations', model_backends: [
      { id: 'pixal3d-mlx', status: 'ready', installable: true, ready: true },
    ] },
  }));
  await page.route('**/api/v1/operations', route => route.fulfill({
    json: [{ id: 'image.generate', implemented: true }, { id: 'mesh.simplify', implemented: true }],
  }));
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const tools = page.getByRole('group', { name: 'Model tools' });
  await tools.getByRole('button', { name: 'New model' }).click();
  const generate = page.getByRole('complementary', { name: 'Generate model' });
  await expect(generate.getByRole('button', { name: 'Generate 3D' })).toBeDisabled();
  const active = await chooseFile(page, page.getByRole('button', { name: 'Open GLB', exact: true }), 'inspected.glb',
    'model/gltf-binary', texturedGlb());
  const selected = page.getByRole('region', { name: 'Selected asset' });
  await expect(selected.getByTitle(active.name)).toBeVisible();

  const image = new PNG({ width: 2, height: 2 });
  image.data.fill(160);
  const reference = await chooseFile(page, generate.getByRole('button', { name: 'Choose reference' }), 'reference.png',
    'image/png', PNG.sync.write(image));
  await expect(generate.getByTitle(reference.name)).toBeVisible();
  await expect(selected.getByTitle(active.name)).toBeVisible();
  const derived = { ...active, id: 'asset_generation_layout', name: 'new-model.glb', source_asset_id: reference.id };
  const job = {
    id: 'job_generation_layout',
    operation: 'image.generate',
    input_asset_id: reference.id,
    created_at: String(Date.now()),
    updated_at: String(Date.now()),
  };
  await page.route('**/api/v1/jobs', async route => {
    const payload = route.request().postDataJSON();
    expect(payload.operation).toBe('image.generate');
    expect(payload.input.asset_id).toBe(reference.id);
    await route.fulfill({ status: 202, json: { ...job, state: 'queued', progress: 0 } });
  });
  await page.route('**/api/v1/jobs/job_generation_layout', route => route.fulfill({ json: {
    ...job, state: 'succeeded', progress: 100, result: { asset_id: derived.id },
  } }));
  await page.route('**/api/v1/assets/asset_generation_layout', route => route.fulfill({ json: derived }));
  await generate.getByRole('button', { name: 'Generate 3D' }).click();
  await expect(page.locator('#asset-library').getByTitle(derived.name)).toBeVisible();
  await expect(selected.getByTitle(active.name)).toBeVisible();
  await generate.getByRole('button', { name: 'View generated model' }).click();
  await expect(selected.getByTitle(derived.name)).toBeVisible();
});
