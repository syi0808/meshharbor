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
  const jobs = tools.getByRole('button', { name: 'Jobs', exact: true });
  await expect(jobs).toHaveText('');
  await expect(jobs.locator('svg')).toHaveCSS('width', '20px');
  await jobs.click();
  const jobsPanel = page.getByRole('complementary', { name: 'Jobs' });
  await expect(jobsPanel.getByText('No saved jobs yet.')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(jobsPanel).toBeVisible();
  const panel = await jobsPanel.boundingBox();
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
    json: [{ id: 'image.generate', implemented: true }, { id: 'mesh.simplify', implemented: true }],
  }));
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const tools = page.getByRole('group', { name: 'Model tools' });
  await tools.getByRole('button', { name: 'New model' }).click();
  const generate = page.getByRole('complementary', { name: 'Generate model' });
  const model = generate.getByLabel('Generation model');
  await expect(model.locator('option')).toHaveText(['Pixal3D', 'Second model']);
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
  await expect(generate.getByLabel('Geometry resolution')).toHaveAttribute('readonly', '');
  const texture = generate.getByLabel('Texture size');
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
  const faces = generate.getByLabel('Soft face target');
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
    operation: 'image.generate',
    input_asset_id: reference.id,
    created_at: String(Date.now()),
    updated_at: String(Date.now()),
  };
  let generationState = 'queued';
  await page.route('**/api/v1/jobs', async route => {
    const payload = route.request().postDataJSON();
    expect(payload.operation).toBe('image.generate');
    expect(payload.input.asset_id).toBe(reference.id);
    expect(payload.parameters).toEqual({ backend_id: 'pixal3d-cuda', resolution: 512, texture_size: 2048, target_faces: 50001, seed: 0, tuning: { texture_steps: 18 } });
    await route.fulfill({ status: 202, json: { ...job, state: 'queued', progress: 0 } });
  });
  await page.route('**/api/v1/jobs/job_generation_layout', route => route.fulfill({ json: {
    ...job,
    state: generationState,
    progress: generationState === 'succeeded' ? 100 : generationState === 'running' ? 5 : 0,
    ...(generationState === 'running' ? { started_at: job.created_at, stage: 'texture_flow' } : {}),
    ...(generationState === 'succeeded' ? { result: { asset_id: derived.id }, started_at: job.created_at } : {}),
  } }));
  await page.route('**/api/v1/assets/asset_generation_layout', route => route.fulfill({ json: derived }));
  await generateAction.click();
  const inventory = page.getByRole('complementary', { name: 'Inventory' });
  const pending = inventory.getByRole('region', { name: 'Models in progress' });
  await expect(pending.getByText('Waiting for an engine slot')).toBeVisible();
  await expect(pending.getByText(/^Queue time:/)).toBeVisible();
  await inventory.getByRole('tab', { name: 'Images' }).click();
  await expect(pending).toHaveCount(0);
  await inventory.getByRole('tab', { name: '3D models' }).click();
  generationState = 'running';
  await expect(pending.getByText('Generating textures', { exact: true })).toBeVisible();
  await pending.getByRole('button', { name: 'View progress for reference.png' }).click();
  await expect(page.getByRole('heading', { name: 'Jobs dashboard' })).toBeFocused();
  const inProgress = page.getByRole('region', { name: /In progress/ });
  await expect(inProgress.getByText('Generating textures', { exact: true })).toBeVisible();
  await expect(inProgress.getByText(/^Elapsed: /)).toBeVisible();
  await page.getByRole('button', { name: 'Studio', exact: true }).click();
  generationState = 'succeeded';
  await expect(page.getByRole('region', { name: /Notifications/ }).getByText('Image-to-3D generation complete.', { exact: false })).toBeVisible();
  await expect(pending).toHaveCount(0);
  await expect(page.locator('#asset-library').getByTitle(derived.name)).toBeVisible();
  await expect(selected.getByTitle(active.name)).toBeVisible();
  await generate.getByRole('button', { name: 'View generated model' }).click();
  await expect(selected.getByTitle(derived.name)).toBeVisible();
});

test('generation switches between posed presets and maps each custom image to its camera', async ({ page, studioOrigin }) => {
  let multiviewInstalled = false;
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'linux',
    status: 'ready_for_mesh_operations',
    model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', backend_ids: ['pixal3d-cuda'] }],
    model_backends: [{
      id: 'pixal3d-cuda', ready: true, multiview_ready: multiviewInstalled,
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
  await page.route('**/api/v1/operations', route => route.fulfill({ json: [{ id: 'image.generate', implemented: true }] }));
  await page.goto(studioOrigin);
  await page.getByRole('group', { name: 'Model tools' }).getByRole('button', { name: 'New model' }).click();
  const form = page.getByRole('complementary', { name: 'Generate model' });
  const mode = form.getByLabel('View mode');
  const submit = form.getByRole('button', { name: 'Generate 3D' });
  await expect(mode.locator('option')).toHaveText(['1 view', '4 views', '6 views', 'Custom views']);
  await expect(mode.locator('option[value="four"]')).toBeDisabled();
  await form.getByRole('button', { name: 'Set up multiview' }).click();
  const install = page.getByRole('button', { name: 'Install multiview weights' });
  await expect(install).toBeDisabled();
  await page.getByRole('checkbox', { name: 'I approve installing the additional model files.' }).check();
  await install.click();
  await expect(page.getByText('Installed. Multiple reference views are available in Generate model.')).toBeVisible();
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
    operation: 'image.generate',
    input: { asset_id: uploads[0].id },
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
