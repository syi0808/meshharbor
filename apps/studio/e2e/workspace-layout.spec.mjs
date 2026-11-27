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

function lato2Runtime(ready) {
  return {
    status: 'ready_for_mesh_operations', platform: 'linux',
    operations: ready ? ['model.generate', 'mesh.retopology_lato2'] : [],
    model_catalog: [{ id: 'lato2', title: 'LATO.2', backend_ids: ['lato2-cuda'], output: 'geometry_glb' }],
    model_backends: [{
      id: 'lato2-cuda', manual_setup: true, ready, missing: ['Local LATO.2 checkpoints are required.'],
      generation: {
        id: 'lato2-cuda', input_modes: ['image'], view_modes: ['single'],
        parameters: [{ id: 'target_vertices', label: 'Target vertices', kind: 'integer', default: 2000, min: 200, max: 5000 }],
      },
    }],
  };
}

test('LATO.2 is visible but cannot run before operator setup', async ({ page, studioOrigin }) => {
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: lato2Runtime(false) }));
  await page.route('**/api/v1/operations', route => route.fulfill({ json: [] }));
  await page.goto(studioOrigin);
  const tools = page.getByRole('group', { name: 'Model tools' });
  await tools.getByRole('button', { name: 'Mesh to Mesh', exact: true }).click();
  const mesh = page.getByRole('complementary', { name: 'Mesh to Mesh' });
  await expect(mesh.getByLabel('Mesh to Mesh model')).toHaveValue('lato2');
  await expect(mesh.getByRole('button', { name: 'Run Mesh to Mesh' })).toBeDisabled();
  await tools.getByRole('button', { name: 'New model' }).click();
  const image = page.getByRole('complementary', { name: 'Generate model' });
  await expect(image.getByLabel('Generation model', { exact: true }).locator('option')).toHaveText('LATO.2 (setup required)');
  await expect(image.getByText('Local LATO.2 checkpoints are required.')).toBeVisible();
  await expect(image.getByRole('button', { name: 'Generate 3D' })).toHaveCount(0);
});

test('LATO.2 submits distinct Mesh to Mesh and Image to 3D jobs', async ({ page, studioOrigin }) => {
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: lato2Runtime(true) }));
  await page.route('**/api/v1/operations', route => route.fulfill({
    json: [{ id: 'model.generate', implemented: true }, { id: 'mesh.retopology_lato2', implemented: true }],
  }));
  const submissions = [];
  await page.route('**/api/v1/jobs', route => {
    if (route.request().method() !== 'POST') return route.continue();
    const request = route.request().postDataJSON();
    submissions.push(request);
    return route.fulfill({ status: 202, json: {
      id: `lato2-job-${submissions.length}`, operation: request.operation, state: 'failed', progress: 0,
      input_asset_id: request.input.asset_id, error: { message: 'fixture' },
      created_at: String(Date.now()), updated_at: String(Date.now()),
    } });
  });
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const tools = page.getByRole('group', { name: 'Model tools' });
  const reference = await chooseFile(page, page.getByRole('button', { name: 'Open GLB', exact: true }),
    'lato2-panel.glb', 'model/gltf-binary', texturedGlb({ singleMaterial: true }));
  await tools.getByRole('button', { name: 'Mesh to Mesh', exact: true }).click();
  const mesh = page.getByRole('complementary', { name: 'Mesh to Mesh' });
  await mesh.getByLabel('Target vertices').fill('220');
  await mesh.getByLabel('Maximum triangles (optional)').fill('250');
  await mesh.getByLabel('Texture atlas size').selectOption('512');
  await mesh.getByRole('button', { name: 'Run Mesh to Mesh' }).click();
  await expect.poll(() => submissions[0]).toEqual({
    operation: 'mesh.retopology_lato2', input: { asset_id: reference.id },
    parameters: { target_vertices: 220, max_triangles: 250, atlas_resolution: 512 },
  });

  await tools.getByRole('button', { name: 'New model' }).click();
  const image = page.getByRole('complementary', { name: 'Generate model' });
  await image.getByLabel('Generation model', { exact: true }).selectOption('lato2');
  await expect(image.getByText('Output: geometry-only GLB (no textures).')).toBeVisible();
  await expect(image.getByLabel('Target vertices')).toHaveValue('2000');
  const pixels = new PNG({ width: 2, height: 2 });
  pixels.data.fill(180);
  const input = await chooseFile(page, image.getByRole('button', { name: 'Choose reference' }),
    'lato2-reference.png', 'image/png', PNG.sync.write(pixels));
  await image.getByLabel('Target vertices').fill('1200');
  await image.getByRole('button', { name: 'Generate 3D' }).click();
  await expect.poll(() => submissions[1]).toEqual({
    operation: 'model.generate', input: { kind: 'image', asset_id: input.id },
    parameters: { backend_id: 'lato2-cuda', target_vertices: 1200 },
  });
});

test('the tool panel stays docked on desktop and stacks on mobile while inventory uses a drawer', async ({ page, studioOrigin }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const tools = page.getByRole('group', { name: 'Model tools' });
  const inventory = page.getByRole('banner').getByRole('button', { name: 'Inventory', exact: true });
  await expect(tools.getByRole('button', { name: 'Optimize' })).toBeEnabled();
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
  await expect(inspector).toBeVisible();
  await expect(rightDock).toBeVisible();
  expect(await page.locator('html').evaluate(element => element.scrollWidth)).toBe(1024);
  await page.keyboard.press('Escape');
  await expect(inspector).toBeVisible();

  await page.setViewportSize({ width: 390, height: 700 });
  await expect(rightDock).toHaveCount(0);
  const jobs = page.getByRole('banner').getByRole('button', { name: 'Jobs', exact: true });
  await expect(tools.getByRole('button', { name: 'Jobs', exact: true })).toHaveCount(0);
  await jobs.click();
  const jobsPanel = page.getByRole('dialog', { name: 'Jobs' });
  await expect(jobsPanel.getByText('No finished jobs yet.')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(jobsPanel).toHaveCount(0);
  await expect(jobs).toBeFocused();
  const panel = await inspector.boundingBox();
  expect(panel.y + panel.height).toBeLessThanOrEqual((await selected.boundingBox()).y);
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

test('a backend can advertise text input without requiring an image asset', async ({ page, studioOrigin }) => {
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'macos', status: 'ready_for_mesh_operations',
    model_catalog: [
      { id: 'image-fixture', title: 'Image fixture', backend_ids: ['image-fixture'] },
      { id: 'dual-fixture', title: 'Dual fixture', backend_ids: ['dual-fixture'] },
      { id: 'trellis-text', title: 'TRELLIS Text Large', backend_ids: ['trellis-text-mlx', 'trellis-text-cuda'], output: 'geometry_glb' },
    ],
    model_backends: [
      { id: 'image-fixture', ready: true, generation: { id: 'image-fixture', input_modes: ['image'], parameters: [] } },
      { id: 'dual-fixture', ready: true, generation: { id: 'dual-fixture', input_modes: ['image', 'text'], parameters: [] } },
      { id: 'trellis-text-mlx', ready: true, generation: { id: 'trellis-text-mlx', input_modes: ['text'], parameters: [] } },
      { id: 'trellis-text-cuda', ready: false, generation: { id: 'trellis-text-cuda', input_modes: ['text'], parameters: [] } },
    ],
  } }));
  await page.route('**/api/v1/operations', route => route.fulfill({ json: [{ id: 'model.generate', implemented: true }] }));
  await page.route('**/api/v1/jobs', route => {
    expect(route.request().postDataJSON()).toEqual({
      operation: 'model.generate',
      input: { kind: 'text', prompt: 'a ceramic cup' },
      parameters: { backend_id: 'trellis-text-mlx' },
    });
    return route.fulfill({ status: 202, json: {
      id: 'text-fixture-job', operation: 'model.generate', state: 'queued', progress: 0,
      parameters: { input_mode: 'text' }, input_asset_id: null,
      created_at: String(Date.now()), updated_at: String(Date.now()),
    } });
  });
  await page.goto(studioOrigin);
  const form = page.getByRole('complementary', { name: 'Generate model' });
  const model = form.getByLabel('Generation model', { exact: true });
  const input = form.getByLabel('Input type');
  const prompt = form.getByLabel('Text prompt');
  const submit = form.getByRole('button', { name: 'Generate 3D' });
  await expect(model).toHaveValue('image-fixture');
  await expect(input).toHaveCount(0);
  await expect(prompt).toHaveCount(0);
  await model.selectOption('dual-fixture');
  await expect(input.locator('option')).toHaveText(['Image', 'Text']);
  await input.selectOption('text');
  await expect(form.getByRole('button', { name: 'Choose reference' })).toHaveCount(0);
  await expect(prompt).toBeVisible();
  await model.selectOption('trellis-text');
  await expect(input).toHaveCount(0);
  await expect(form.getByRole('button', { name: 'Choose reference' })).toHaveCount(0);
  await expect(submit).toBeDisabled();
  await prompt.fill('가'.repeat(1366));
  await expect(prompt).toHaveAttribute('aria-invalid', 'true');
  await expect(submit).toBeDisabled();
  await prompt.fill('  a ceramic cup  ');
  await page.setViewportSize({ width: 390, height: 680 });
  await submit.click();
  const inventory = page.getByRole('dialog', { name: 'Inventory' });
  await expect(inventory.getByRole('button', { name: 'Select pending model New model' })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByRole('region', { name: /Notifications/ }).getByText('Text-to-3D generation queued.')).toBeVisible();
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const models = page.getByRole('main', { name: 'Models' });
  await expect(models.getByText('Text to 3D', { exact: true })).toHaveCount(2);
  await expect(models.getByText('Image to 3D', { exact: true })).toHaveCount(2);
});

test('generation uses its explicit reference and opens and selects the new inventory card', async ({ page, studioOrigin }) => {
  const parameters = (textureOptions, seedDefault) => [
    { id: 'resolution', kind: 'fixed', label: 'Geometry resolution', description: 'Fixed by the profile.', default: 512 },
    { id: 'texture_size', kind: 'choice', label: 'Texture size', description: 'Texture resolution.', default: textureOptions[0], values: textureOptions },
    { id: 'target_faces', kind: 'integer', label: 'Soft face target', description: 'Not a hard cap.', default: 100000, min: 4, max: 1000000 },
    { id: 'seed', kind: 'integer', label: 'Seed', description: 'Repeatable sampling.', default: seedDefault, min: 0 },
  ];
  await page.route('**/api/v1/runtime', route => route.fulfill({
    json: {
      platform: 'macos', status: 'ready_for_mesh_operations',
      model_catalog: [
        { id: 'pixal3d', title: 'Pixal3D', backend_ids: ['pixal3d-mlx', 'pixal3d-cuda'] },
        { id: 'second-model', title: 'Second model', backend_ids: ['pixal3d-cuda'] },
        { id: 'unready', title: 'Unavailable model', backend_ids: ['unready'] },
      ],
      model_backends: [
        { id: 'pixal3d-mlx', status: 'ready', installable: true, ready: true,
          generation: { id: 'pixal3d-mlx', label: 'Apple MLX', parameters: parameters([512, 1024], 42) } },
        { id: 'pixal3d-cuda', status: 'ready', installable: true, ready: true,
          generation: { id: 'pixal3d-cuda', label: 'NVIDIA CUDA', parameters: [
            ...parameters([1024, 2048], 7),
            { id: 'texture_steps', kind: 'integer', tuning: true, section: 'Sampling', label: 'Texture sampling steps', min: 1, max: 100, default: 12 },
          ] } },
        { id: 'unready', status: 'setup_required', ready: false,
          generation: { id: 'unready', label: 'Unready', parameters: [] } },
      ],
    },
  }));
  await page.route('**/api/v1/operations', route => route.fulfill({
    json: [{ id: 'model.generate', implemented: true }, { id: 'mesh.simplify', implemented: true }],
  }));
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const tools = page.getByRole('group', { name: 'Model tools' });
  await tools.getByRole('button', { name: 'New model' }).click();
  const generate = page.getByRole('complementary', { name: 'Generate model' });
  const model = generate.getByRole('combobox', { name: 'Generation model', exact: true });
  await expect(model.locator('option')).toHaveText(['Pixal3D', 'Second model']);
  await expect(generate.getByLabel('Text prompt')).toHaveCount(0);
  expect((await model.boundingBox()).y).toBeLessThan((await generate.getByRole('region', { name: 'Reference image' }).boundingBox()).y);
  await expect(generate.getByRole('button', { name: 'Generate 3D' })).toBeDisabled();
  const active = await chooseFile(page, page.getByRole('button', { name: 'Open GLB', exact: true }), 'inspected.glb',
    'model/gltf-binary', texturedGlb());
  const selected = page.getByRole('region', { name: 'Selected asset' });
  await expect(selected.getByTitle(active.name)).toBeVisible();

  const image = new PNG({ width: 2, height: 2 });
  image.data.fill(160);
  const reference = await chooseFile(page, generate.getByRole('button', { name: 'Choose reference' }), 'reference.png',
    'image/png', PNG.sync.write(image));
  const preview = generate.getByRole('img', { name: 'Reference preview: reference.png' });
  await expect(preview).toBeVisible();
  await expect.poll(() => preview.evaluate(image => image.naturalWidth)).toBe(2);
  await expect(selected.getByTitle(active.name)).toBeVisible();
  const library = page.getByRole('complementary', { name: 'Inventory' });
  await library.getByRole('tab', { name: 'Images' }).click();
  await generate.getByRole('button', { name: 'Clear', exact: true }).click();
  await library.getByRole('button', { name: /^reference\.png/ }).dragTo(generate.getByRole('group', { name: 'Drop a reference image' }));
  await expect(preview).toBeVisible();
  await expect(selected.getByTitle(active.name)).toBeVisible();
  await library.getByRole('tab', { name: '3D models' }).click();
  const seed = generate.getByLabel('Seed', { exact: true });
  await expect(seed).toHaveValue('42');
  await seed.fill('17');
  await model.selectOption('second-model');
  await expect(seed).toHaveValue('7');
  await expect(generate.getByRole('spinbutton', { name: 'Geometry resolution', exact: true })).toHaveAttribute('readonly', '');
  await generate.getByRole('button', { name: 'About Geometry resolution', exact: true }).focus();
  await expect(page.getByRole('tooltip', { name: 'Fixed by the profile.' })).toBeVisible();
  await page.keyboard.press('Escape');
  const texture = generate.getByRole('combobox', { name: 'Texture size', exact: true });
  await expect(texture.locator('option')).toHaveText(['1024', '2048']);
  await texture.selectOption('2048');
  const sampling = generate.getByRole('button', { name: 'Sampling', exact: true });
  const output = generate.getByRole('button', { name: 'Output', exact: true });
  const samplingSteps = generate.getByLabel('Texture sampling steps');
  await samplingSteps.fill('18');
  await sampling.click();
  await expect(sampling).toHaveAttribute('aria-expanded', 'false');
  await expect(samplingSteps).toBeHidden();
  await expect(output).toHaveAttribute('aria-expanded', 'true');
  await expect(seed).toBeVisible();
  await sampling.press('Enter');
  await expect(samplingSteps).toHaveValue('18');
  const faces = generate.getByRole('spinbutton', { name: 'Soft face target', exact: true });
  await faces.fill('3');
  await expect(faces).toHaveAttribute('aria-invalid', 'true');
  await expect(generate.getByRole('button', { name: 'Generate 3D' })).toBeDisabled();
  await faces.fill('50001');
  await model.selectOption('pixal3d');
  await expect(seed).toHaveValue('17');
  await model.selectOption('second-model');
  await expect(faces).toHaveValue('50001');
  await seed.fill('0');
  await page.setViewportSize({ width: 1280, height: 600 });
  const scrollBody = generate.getByRole('region', { name: 'Generate model content' });
  const generateAction = generate.getByRole('button', { name: 'Generate 3D' });
  await expect(generateAction).toBeInViewport({ ratio: 1 });
  const footerPosition = await generateAction.boundingBox();
  await scrollBody.focus();
  await scrollBody.press('End');
  await expect.poll(() => scrollBody.evaluate(element => element.scrollTop)).toBeGreaterThan(0);
  expect(await generateAction.boundingBox()).toEqual(footerPosition);
  await expect(generate.getByRole('group', { name: 'Sampling' }).getByLabel('Texture sampling steps')).toHaveValue('18');
  const derived = {
    ...active, id: 'asset_generation_layout', name: 'new-model.glb', source_asset_id: reference.id, model_version: 1,
    model: { id: 'asset_generation_layout', name: 'new-model.glb', generated: true, current_asset_id: 'asset_generation_layout' },
  };
  const job = {
    id: 'job_generation_layout',
    operation: 'model.generate',
    input_asset_id: reference.id,
    created_at: String(Date.now()),
    updated_at: String(Date.now()),
  };
  let generationState = 'running';
  await page.route('**/api/v1/jobs', async route => {
    const payload = route.request().postDataJSON();
    expect(payload.operation).toBe('model.generate');
    expect(payload.input).toEqual({ kind: 'image', asset_id: reference.id });
    expect(payload.parameters).toEqual({ backend_id: 'pixal3d-cuda', resolution: 512, texture_size: 2048, target_faces: 50001, seed: 0, tuning: { texture_steps: 18 } });
    await route.fulfill({ status: 202, json: { ...job, state: 'running', progress: 5, started_at: job.created_at, stage: 'texture_flow' } });
  });
  await page.route('**/api/v1/jobs/job_generation_layout', route => route.fulfill({ json: {
    ...job,
    state: generationState,
    progress: generationState === 'succeeded' ? 100 : generationState === 'running' ? 5 : 0,
    ...(generationState === 'running' ? { started_at: job.created_at, stage: 'texture_flow' } : {}),
    ...(generationState === 'succeeded' ? { result: { asset_id: derived.id }, started_at: job.created_at } : {}),
  } }));
  await page.route('**/api/v1/jobs/job_generation_layout/steps', route => route.fulfill({ json: [
    { id: 1, state: 'running', stage: 'texture_flow', at: job.created_at, message: null, commands: [] },
  ] }));
  await page.route('**/api/v1/assets/asset_generation_layout', route => route.fulfill({ json: derived }));
  const inventoryToggle = page.getByRole('button', { name: 'Inventory', exact: true });
  const inventory = page.getByRole('complementary', { name: 'Inventory' });
  await inventory.getByRole('tab', { name: 'Images' }).click();
  await inventoryToggle.click();
  await expect(inventoryToggle).toHaveAttribute('aria-expanded', 'false');
  await generateAction.click();
  await expect(inventoryToggle).toHaveAttribute('aria-expanded', 'true');
  await expect(inventory.getByRole('tab', { name: '3D models' })).toHaveAttribute('aria-selected', 'true');
  const pending = inventory.getByRole('region', { name: 'Models in progress' });
  await expect(pending.getByText('Generating textures', { exact: true })).toBeVisible();
  await expect(pending.getByRole('button', { name: /Select pending model/ })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByRole('region', { name: /Notifications/ }).getByText('Image-to-3D generation started.')).toBeVisible();
  await expect(generate.getByRole('progressbar', { name: 'Job progress' })).toHaveCount(0);
  await expect(generateAction).toBeEnabled();
  await inventory.getByRole('tab', { name: 'Images' }).click();
  await expect(pending).toHaveCount(0);
  await inventory.getByRole('tab', { name: '3D models' }).click();
  await expect(pending.getByText('Generating textures', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Jobs', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Jobs', exact: true })).toBeVisible();
  const inProgress = page.getByRole('region', { name: /In progress/ });
  await expect(inProgress.getByText('Generating textures', { exact: true })).toBeVisible();
  await expect(inProgress.getByText(/^Elapsed: /)).toBeVisible();
  await inProgress.getByRole('button', { name: 'View details for Generate model' }).click();
  await expect(page.getByRole('dialog', { name: 'Job details' }).locator('summary').filter({ hasText: 'Generating textures' })).toBeVisible();
  await page.keyboard.press('Escape');
  await inventoryToggle.click();
  generationState = 'succeeded';
  await expect(page.getByRole('region', { name: /Notifications/ }).getByText('Image-to-3D generation complete.', { exact: false })).toBeVisible();
  await expect(pending).toHaveCount(0);
  await expect(inventory.getByRole('button', { name: /new-model\.glb/ })).toHaveAttribute('aria-pressed', 'true');
  await expect(selected.getByTitle(derived.name)).toBeVisible();
});

test('generation can be queued, reordered and selected before its preview is ready', async ({ page, studioOrigin }) => {
  const jobs = [];
  const steps = [];
  let order = [];
  const references = new Map();
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'macos', status: 'ready_for_mesh_operations',
    model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', backend_ids: ['pixal3d-mlx'] }],
    model_backends: [{ id: 'pixal3d-mlx', ready: true, installable: true, generation: { id: 'pixal3d-mlx', parameters: [] } }],
  } }));
  await page.route('**/api/v1/operations', route => route.fulfill({ json: [{ id: 'model.generate', implemented: true }] }));
  await page.route('**/api/v1/jobs?limit=20&include_active=true', route => route.fulfill({ json: jobs }));
  await page.route('**/api/v1/jobs/generation-2/steps', route => route.fulfill({ json: steps }));
  await page.route('**/api/v1/jobs/generation-queue', route => {
    if (route.request().method() === 'POST') {
      const { job_id, swap_with } = route.request().postDataJSON();
      expect([job_id, swap_with]).toEqual(['generation-2', 'generation-1']);
      order = ['generation-2', 'generation-1', 'generation-3'];
    }
    return route.fulfill({ json: order });
  });
  await page.route('**/api/v1/jobs', route => {
    const request = route.request().postDataJSON();
    const timestamp = String(Date.now());
    const job = {
      id: `generation-${jobs.length + 1}`, operation: 'model.generate', state: 'queued', progress: 0,
      input_asset_id: request.input.asset_id, input_asset_name: references.get(request.input.asset_id),
      created_at: timestamp, updated_at: timestamp,
    };
    jobs.push(job);
    order.push(job.id);
    return route.fulfill({ status: 202, json: job });
  });
  await page.route('**/api/v1/jobs/generation-3/cancel?queued_only=true', route => {
    order = order.filter(id => id !== 'generation-3');
    jobs[2].state = 'cancelled';
    jobs[2].updated_at = String(Date.now());
    return route.fulfill({ json: jobs[2] });
  });
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const original = await chooseFile(page, page.getByRole('button', { name: 'Open GLB', exact: true }), 'original.glb', 'model/gltf-binary', texturedGlb());
  const image = new PNG({ width: 2, height: 2 });
  const form = page.getByRole('complementary', { name: 'Generate model' });
  const submit = form.getByRole('button', { name: 'Generate 3D', exact: true });
  const notifications = page.getByRole('region', { name: /Notifications/ });
  for (const [index, name] of ['first.png', 'second.png', 'third.png'].entries()) {
    image.data.fill(120 + index);
    const reference = await chooseFile(page, form.getByRole('button', { name: /Choose reference|Replace reference/ }), name, 'image/png', PNG.sync.write(image));
    references.set(reference.id, name);
    await submit.click();
    await expect(submit).toBeEnabled();
  }
  await expect(notifications.getByText('Image-to-3D generation queued.').last()).toBeVisible();
  const pending = page.getByRole('button', { name: 'Select pending model second.png' });
  await pending.click();
  await expect(pending).toHaveAttribute('aria-pressed', 'true');
  const preview = page.getByRole('region', { name: 'Pending model preview' });
  await expect(preview.getByText('Waiting for an engine slot')).toBeVisible();

  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const models = page.getByRole('main', { name: 'Models' });
  await expect(models.getByRole('button', { name: 'Recent logs' })).toHaveCount(0);
  await expect(models.getByText(/Generate model|Waiting for an engine slot/)).toHaveCount(0);
  await page.getByRole('button', { name: 'Workbench', exact: true }).click();
  await preview.getByRole('button', { name: 'View in Jobs' }).click();
  const queue = page.getByRole('region', { name: /Generation queue/ });
  const second = queue.getByRole('listitem').filter({ hasText: 'second.png' });
  await expect(second).toContainText('#2 in queue');
  await second.getByRole('button', { name: 'Move earlier' }).click();
  await expect(second).toContainText('#1 in queue');
  await queue.getByRole('listitem').filter({ hasText: 'third.png' }).getByRole('button', { name: 'Remove from queue' }).click();
  await page.getByRole('alertdialog', { name: 'Remove this queued job?' }).getByRole('button', { name: 'Remove from queue' }).click();
  await expect(queue.getByText('third.png')).toHaveCount(0);

  order.shift();
  Object.assign(jobs[1], { state: 'running', stage: 'shape_hr', progress: 5, started_at: String(Date.now()), updated_at: String(Date.now()) });
  const saveStep = () => steps.push({
    id: steps.length + 1, state: jobs[1].state, stage: jobs[1].state === 'running' ? jobs[1].stage : null,
    at: jobs[1].updated_at, message: null, commands: [],
  });
  saveStep();
  await expect(notifications.getByText('Image-to-3D generation started.')).toBeVisible();
  const active = page.getByRole('region', { name: /In progress/ });
  await active.getByRole('button', { name: 'View details for Generate model' }).click();
  const details = page.getByRole('dialog', { name: 'Job details' });
  const step = label => details.locator('summary').filter({ hasText: label });
  await expect(step('Refining geometry')).toBeVisible();
  Object.assign(jobs[1], { stage: 'texture_flow', updated_at: String(Date.now()) });
  saveStep();
  await expect(step('Generating textures')).toBeVisible();
  const model = { ...original, id: 'completed-second', name: 'second.glb', source_asset_id: jobs[1].input_asset_id, model: undefined };
  await page.route('**/api/v1/assets/completed-second', route => route.fulfill({ json: model }));
  Object.assign(jobs[1], { state: 'succeeded', progress: 100, result: { asset_id: model.id }, updated_at: String(Date.now()) });
  saveStep();
  await expect(step('Complete')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(notifications.getByText('Image-to-3D generation complete.')).toBeVisible();
  await page.getByRole('button', { name: 'Inventory', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Selected asset' }).getByTitle('second.glb')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Select pending model first.png' })).toBeVisible();
});

test('generation switches between posed presets and maps each custom image to its camera', async ({ page, studioOrigin }) => {
  let multiviewInstalled = false;
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'linux',
    status: 'ready_for_mesh_operations',
    environment: { ready: true, python_versions: ['3.10', '3.12'] },
    model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', backend_ids: ['pixal3d-cuda'] }],
    model_backends: [{
      id: 'pixal3d-cuda', installable: true, ready: true, multiview_ready: multiviewInstalled,
      requires_huggingface_login: true, huggingface: { account: 'connected', model_access: 'verified' },
      generation: {
        id: 'pixal3d-cuda', view_modes: ['single', 'four', 'six', 'custom'], max_views: 8,
        parameters: [
          { id: 'fov_degrees', kind: 'number', tuning: true, label: 'Camera field of view (°)', min: 1, max: 179, default: 49.13, single_view_only: true },
          { id: 'structure_steps', kind: 'integer', tuning: true, label: 'Structure sampling steps', min: 1, max: 100, default: 12 },
        ],
      },
    }],
  } }));
  const installJob = {
    id: 'fixture-multiview-install', operation: 'runtime.pixal3d.cuda_install_multiview',
    state: 'queued', progress: 0, created_at: '0', updated_at: '0',
  };
  await page.route('**/api/v1/runtime/pixal3d-cuda/multiview', route => {
    expect(route.request().postDataJSON()).toEqual({ confirm_large_download: true });
    return route.fulfill({ status: 202, json: installJob });
  });
  await page.route('**/api/v1/jobs/fixture-multiview-install', route => {
    multiviewInstalled = true;
    return route.fulfill({ json: { ...installJob, state: 'succeeded', progress: 100 } });
  });
  await page.route('**/api/v1/operations', route => route.fulfill({ json: [{ id: 'model.generate', implemented: true }] }));
  await page.goto(studioOrigin);
  await page.getByRole('group', { name: 'Model tools' }).getByRole('button', { name: 'New model' }).click();
  const form = page.getByRole('complementary', { name: 'Generate model' });
  const mode = form.getByLabel('View mode');
  const submit = form.getByRole('button', { name: 'Generate 3D' });
  await expect(mode.locator('option')).toHaveText(['1 view', '4 views', '6 views', 'Custom views']);
  await expect(mode.locator('option[value="four"]')).toBeDisabled();
  await form.getByRole('button', { name: 'Set up multiview' }).click();
  await page.getByRole('button', { name: 'Install multiview weights' }).click();
  const dialog = page.getByRole('alertdialog', { name: 'Install multiview weights?' });
  const install = dialog.getByRole('button', { name: 'Install multiview weights' });
  await expect(install).toBeDisabled();
  await dialog.getByRole('checkbox', { name: 'I approve installing the additional model files.' }).check();
  await install.click();
  await expect(page.getByText('Multiview installed', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Workbench', exact: true }).click();
  await expect(form.getByLabel('Camera field of view (°)')).toBeVisible();
  await mode.selectOption('four');
  await expect(form.getByRole('group', { name: 'Front', exact: true })).toBeVisible();
  await expect(form.getByRole('group', { name: 'Top', exact: true })).toHaveCount(0);
  await expect(submit).toBeDisabled();
  await mode.selectOption('six');
  await expect(form.getByRole('group', { name: 'Top', exact: true })).toBeVisible();
  await expect(form.getByRole('group', { name: 'Bottom', exact: true })).toBeVisible();
  await mode.selectOption('custom');
  await expect(form.getByLabel('Camera field of view (°)')).toHaveCount(0);
  const uploads = [];
  await page.route('**/api/v1/assets?**', async route => {
    const response = await route.fetch();
    if (route.request().method() === 'POST') uploads.push(await response.json());
    await route.fulfill({ response });
  });
  const [chooser] = await Promise.all([page.waitForEvent('filechooser'), form.getByRole('button', { name: 'Choose multiple images' }).click()]);
  await chooser.setFiles(['front.png', 'right.png'].map((name, index) => {
    const image = new PNG({ width: 2, height: 2 });
    image.data.fill(100 + index);
    return { name, mimeType: 'image/png', buffer: PNG.sync.write(image) };
  }));
  await expect(form.getByRole('img', { name: 'Reference preview: front.png' })).toBeVisible();
  await expect(form.getByRole('img', { name: 'Reference preview: right.png' })).toBeVisible();
  const right = form.getByRole('group', { name: 'View 2', exact: true });
  await right.getByLabel('Distance').fill('0');
  await expect(submit).toBeDisabled();
  await right.getByLabel('Distance').fill('3.5');
  await right.getByLabel('Azimuth (°)').fill('90');
  await form.getByLabel('Structure sampling steps').fill('16');
  const request = page.waitForRequest(request => new URL(request.url()).pathname === '/api/v1/jobs' && request.method() === 'POST');
  await page.route('**/api/v1/jobs', route => route.fulfill({ status: 409, json: { error: 'Fixture stops before executing generation.' } }));
  await submit.click();
  const payload = (await request).postDataJSON();
  expect(payload).toEqual({
    operation: 'model.generate',
    input: { kind: 'image', asset_id: uploads[0].id },
    parameters: {
      backend_id: 'pixal3d-cuda',
      tuning: { structure_steps: 16 },
      view_input: {
        mode: 'custom',
        references: [
          { asset_id: uploads[0].id, camera: { azimuth_degrees: 0, elevation_degrees: 0, distance: 3.119, fov_degrees: 20 } },
          { asset_id: uploads[1].id, camera: { azimuth_degrees: 90, elevation_degrees: 0, distance: 3.5, fov_degrees: 20 } },
        ],
      },
    },
  });
  await page.setViewportSize({ width: 390, height: 640 });
  const mobilePanel = page.getByRole('complementary', { name: 'Generate model' });
  const mobileForm = mobilePanel.getByRole('region', { name: 'Generate model content' });
  const mobileAction = mobilePanel.getByRole('button', { name: 'Generate 3D' });
  await expect(mobileAction).toBeInViewport({ ratio: 1 });
  const mobileFooter = await mobileAction.boundingBox();
  await mobileForm.focus();
  await mobileForm.press('End');
  await expect.poll(() => mobileForm.evaluate(element => element.scrollTop)).toBeGreaterThan(0);
  expect((await mobileAction.boundingBox()).y).toBe(mobileFooter.y);
  expect(await mobileForm.evaluate(element => element.scrollWidth)).toBe(await mobileForm.evaluate(element => element.clientWidth));
  await expect(mobilePanel.getByRole('group', { name: 'View 2', exact: true }).getByLabel('Distance')).toHaveValue('3.5');
});
