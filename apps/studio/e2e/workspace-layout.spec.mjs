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
  const inventory = page.getByRole('banner').getByRole('button', { name: 'Inventory', exact: true });
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
  await expect.poll(async () => Math.round((await toolSheet.boundingBox()).x)).toBe(0);
  expect(await page.locator('html').evaluate(element => element.scrollWidth)).toBe(1024);
  await page.keyboard.press('Escape');
  await expect(toolSheet).toBeHidden();
  await expect(optimize).toBeFocused();

  await page.setViewportSize({ width: 390, height: 700 });
  await expect(rightDock).toHaveCount(0);
  const jobs = tools.getByRole('button', { name: 'Jobs', exact: true });
  await expect(jobs.locator('[data-slot="tool-label"]')).toBeVisible();
  await expect(jobs.locator('svg')).toHaveCSS('width', '20px');
  await jobs.click();
  const jobsSheet = page.getByRole('dialog', { name: 'Jobs' });
  await expect(jobsSheet.getByText('No saved jobs yet.')).toBeVisible();
  await expect(jobsSheet).toHaveAttribute('data-side', 'left');
  await page.keyboard.press('Escape');
  await expect(jobs).toBeFocused();
  expect((await inventory.boundingBox()).y).toBeLessThan((await selected.boundingBox()).y);
  await inventory.click();
  const sheet = page.getByRole('dialog', { name: 'Inventory' });
  await expect(sheet.getByRole('button', { name: 'Open GLB' })).toBeVisible();
  await expect(sheet.getByRole('button', { name: 'Jobs', exact: true })).toHaveCount(0);
  await expect.poll(async () => {
    const { x, width } = await sheet.boundingBox();
    return Math.round(x + width);
  }).toBe(390);
  expect(await page.locator('html').evaluate(element => element.scrollWidth)).toBe(390);
});

test('generation uses its explicit reference and leaves the inspected GLB selected until review', async ({ page, studioOrigin }) => {
  const parameters = (textureOptions, seedDefault) => [
    { id: 'resolution', kind: 'fixed', label: 'Geometry resolution', description: 'Fixed by the profile.', default: 512 },
    { id: 'texture_size', kind: 'choice', label: 'Texture size', description: 'Texture resolution.', default: textureOptions[0], values: textureOptions },
    { id: 'target_faces', kind: 'integer', label: 'Soft face target', description: 'Not a hard cap.', default: 100000, min: 4, max: 1000000 },
    { id: 'seed', kind: 'integer', label: 'Seed', description: 'Repeatable sampling.', default: seedDefault, min: 0 },
  ];
  await page.route('**/api/v1/runtime', route => route.fulfill({
    json: {
      platform: 'macos', status: 'ready_for_mesh_operations',
      model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', backend_ids: ['pixal3d-mlx', 'pixal3d-cuda', 'unready'] }],
      model_backends: [
        { id: 'pixal3d-mlx', status: 'ready', installable: true, ready: true,
          generation: { id: 'pixal3d-mlx', label: 'Apple MLX', parameters: parameters([512, 1024], 42) } },
        { id: 'pixal3d-cuda', status: 'ready', installable: true, ready: true,
          generation: { id: 'pixal3d-cuda', label: 'NVIDIA CUDA', parameters: parameters([1024, 2048], 7) } },
        { id: 'unready', status: 'setup_required', ready: false,
          generation: { id: 'unready', label: 'Unready', parameters: [] } },
      ],
    },
  }));
  await page.route('**/api/v1/operations', route => route.fulfill({
    json: [{ id: 'image.generate', implemented: true }, { id: 'mesh.simplify', implemented: true }],
  }));
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const tools = page.getByRole('group', { name: 'Model tools' });
  await tools.getByRole('button', { name: 'New model' }).click();
  const generate = page.getByRole('complementary', { name: 'Generate model' });
  const model = generate.getByLabel('Generation model');
  await expect(model.locator('option')).toHaveText(['Pixal3D · Apple MLX', 'Pixal3D · NVIDIA CUDA']);
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
  await generate.getByRole('button', { name: 'Model settings' }).click();
  const seed = generate.getByLabel('Seed', { exact: true });
  await expect(seed).toHaveValue('42');
  await seed.fill('17');
  await model.selectOption('pixal3d-cuda');
  await expect(seed).toHaveValue('7');
  await expect(generate.getByLabel('Geometry resolution')).toHaveAttribute('readonly', '');
  const texture = generate.getByLabel('Texture size');
  await expect(texture.locator('option')).toHaveText(['1024', '2048']);
  await texture.selectOption('2048');
  const faces = generate.getByLabel('Soft face target');
  await faces.fill('3');
  await expect(faces).toHaveAttribute('aria-invalid', 'true');
  await expect(generate.getByRole('button', { name: 'Generate 3D' })).toBeDisabled();
  await faces.fill('50001');
  await model.selectOption('pixal3d-mlx');
  await expect(seed).toHaveValue('17');
  await model.selectOption('pixal3d-cuda');
  await expect(faces).toHaveValue('50001');
  await seed.fill('0');
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
    expect(payload.parameters).toEqual({ backend_id: 'pixal3d-cuda', resolution: 512, texture_size: 2048, target_faces: 50001, seed: 0 });
    await route.fulfill({ status: 202, json: { ...job, state: 'queued', progress: 0 } });
  });
  await page.route('**/api/v1/jobs/job_generation_layout', route => route.fulfill({ json: {
    ...job, state: 'succeeded', progress: 100, result: { asset_id: derived.id },
  } }));
  await page.route('**/api/v1/assets/asset_generation_layout', route => route.fulfill({ json: derived }));
  await generate.getByRole('button', { name: 'Generate 3D' }).click();
  await expect(page.getByRole('region', { name: /Notifications/ }).getByText('Image-to-3D generation complete.', { exact: false })).toBeVisible();
  await expect(page.locator('#asset-library').getByTitle(derived.name)).toBeVisible();
  await expect(selected.getByTitle(active.name)).toBeVisible();
  await generate.getByRole('button', { name: 'View generated model' }).click();
  await expect(selected.getByTitle(derived.name)).toBeVisible();
});
