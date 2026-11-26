import { expect } from '@playwright/test';
import { test } from './studio-fixture.mjs';
import { texturedGlb } from './textured-glb.mjs';

const readyEnvironment = { ready: true, python_versions: ['3.10', '3.12'] };

test('the desktop library opens on first visit and remembers a collapsed dock', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const inventory = page.getByRole('button', { name: 'Inventory', exact: true });
  await expect(inventory).toHaveAttribute('aria-expanded', 'true');
  await page.getByRole('button', { name: 'Collapse sidebar' }).click();
  await expect(inventory).toBeFocused();
  await page.reload();
  await expect(inventory).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByRole('banner').getByRole('button', { name: 'Jobs', exact: true })).toBeVisible();
  await inventory.click();
  await expect(inventory).toHaveAttribute('aria-expanded', 'true');
});

test('Jobs and Inventory share one resizable right-hand dock', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const library = page.getByRole('complementary', { name: 'Inventory' });
  const jobs = page.getByRole('banner').getByRole('button', { name: 'Jobs', exact: true });
  await jobs.click();
  await expect(jobs).toHaveAttribute('aria-expanded', 'true');
  const activity = page.getByRole('complementary', { name: 'Jobs' });
  const viewport = page.getByRole('region', { name: 'Start a model' });
  await expect(activity.getByText('No finished jobs yet.')).toBeVisible();
  await expect(library).toHaveCount(0);
  const [center, right] = await Promise.all([viewport.boundingBox(), activity.boundingBox()]);
  expect(center.x + center.width).toBeLessThanOrEqual(right.x);
  expect(Math.round(right.x + right.width)).toBe(1280);
  await page.getByRole('banner').getByRole('button', { name: 'Inventory', exact: true }).click();
  await expect(activity).toHaveCount(0);
  await expect(library.getByRole('button', { name: 'Open GLB' })).toBeVisible();
  const panelWidth = () => library.evaluate(element => Math.round(element.getBoundingClientRect().width));
  const initialWidth = await panelWidth();
  const handle = page.getByRole('separator', { name: 'Resize library panel' });
  await handle.press('ArrowLeft');
  await expect.poll(panelWidth).toBeGreaterThan(initialWidth);
  const resizedWidth = await panelWidth();
  await page.reload();
  await expect.poll(panelWidth).toBe(resizedWidth);
  await expect(page.getByRole('complementary', { name: 'Generate model' })).toBeVisible();
});

test('Jobs and model logs show saved failures in a details popup', async ({ page, studioOrigin }) => {
  const recorded = {
    id: 'failed-download', operation: 'runtime.pixal3d.install_models', state: 'failed', progress: 0,
    created_at: '1', updated_at: '2', error: { message: 'Local cache is unavailable.' },
  };
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'macos',
    environment: readyEnvironment,
    model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', backend_ids: ['pixal3d-mlx'] }],
    model_backends: [{ id: 'pixal3d-mlx', installable: true }],
  } }));
  await page.route('**/api/v1/jobs?limit=20&include_active=true', route => route.fulfill({ json: [recorded] }));
  await page.route('**/api/v1/runtime/pixal3d-mlx/jobs', route => route.fulfill({ json: [recorded] }));
  await page.goto(studioOrigin);
  await page.getByRole('banner').getByRole('button', { name: 'Jobs' }).click();
  const jobsRow = page.getByRole('complementary', { name: 'Jobs' }).getByRole('listitem');
  await expect(jobsRow).toHaveAttribute('data-slot', 'item');
  await expect(jobsRow.getByText('Failed', { exact: true })).toBeVisible();
  await expect(jobsRow.getByText(recorded.error.message)).toHaveCount(0);
  const details = page.getByRole('dialog', { name: 'Job details' });
  const detailsButton = jobsRow.getByRole('button', { name: 'View details for Download MLX models' });
  await detailsButton.click();
  const toasts = page.getByRole('region', { name: /Notifications/ });
  await expect(details.getByText(recorded.error.message)).toBeVisible();
  await expect(toasts.getByText(recorded.error.message)).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(detailsButton).toBeFocused();
  await page.goto(studioOrigin + '/settings/storage');
  const backend = page.getByRole('article', { name: 'Pixal3D · Apple MLX' });
  await backend.getByRole('button', { name: 'Recent logs' }).click();
  const activityRow = backend.getByRole('listitem');
  await expect(activityRow).toHaveAttribute('data-slot', 'item');
  await expect(activityRow.getByText('Download MLX models')).toBeVisible();
  await expect(activityRow.getByText(recorded.error.message)).toHaveCount(0);
  await activityRow.getByRole('button', { name: 'View details for Download MLX models' }).click();
  await expect(details.getByText(recorded.error.message)).toBeVisible();
});

test('Models aligns accelerator cards and installs a catalog model after consent', async ({ page, studioOrigin }) => {
  const paths = [
    `/local/meshharbor/${'nested-runtime-directory/'.repeat(8)}trellis-text-mlx-managed/weights`,
    '/local/meshharbor/manual/trellis-text-mlx/weights',
  ];
  const notice = 'Review the terms for the local CLIP encoder.';
  let installed = false;
  const job = {
    id: 'catalog-install', operation: 'runtime.models.install', state: 'running', progress: 5,
    parameters: { backend_id: 'trellis-text-mlx' }, created_at: '1', updated_at: '1',
  };
  const unsupportedJob = { ...job, id: 'unsupported-install', state: 'succeeded', parameters: { backend_id: 'trellis-text-cuda' } };
  let unsupportedLogRequests = 0;
  await page.route('**/api/v1/jobs?limit=20&include_active=true', route => route.fulfill({ json: [unsupportedJob] }));
  await page.route('**/api/v1/runtime/trellis-text-mlx/jobs', route => route.fulfill({ json: [] }));
  await page.route('**/api/v1/runtime/trellis-text-cuda/jobs', route => {
    unsupportedLogRequests++;
    return route.fulfill({ json: [unsupportedJob] });
  });
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'macos',
    environment: readyEnvironment,
    model_catalog: [{ id: 'trellis-text', title: 'TRELLIS Text Large', backend_ids: ['trellis-text-cuda', 'trellis-text-mlx'] }],
    model_backends: [
      { id: 'trellis-text-mlx', model_paths: paths, managed_install: true, installable: true, downloads_enabled: !installed, ready: installed,
        license_notice: notice, source: { repository: 'https://github.com/example/text-model', revision: 'fixed-revision' },
        local_files: { remove_models_available: installed, model_files_managed: true } },
      { id: 'trellis-text-cuda', model_paths: ['/local/meshharbor/trellis-text-cuda/weights'], managed_install: true, installable: false },
    ],
  } }));
  await page.route('**/api/v1/runtime/trellis-text-mlx/install', route => {
    expect(route.request().method()).toBe('POST');
    expect(route.request().postDataJSON()).toEqual({ confirmed: true, confirm_gated_access: false });
    installed = true;
    return route.fulfill({ status: 202, json: job });
  });
  await page.route('**/api/v1/jobs/catalog-install', route => route.fulfill({ json: {
    ...job, state: 'succeeded', progress: 100, updated_at: '2',
  } }));
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write'], { origin: studioOrigin });
  const logsLoaded = page.waitForResponse('**/api/v1/runtime/trellis-text-mlx/jobs');
  await page.goto(studioOrigin + '/settings/models');
  await logsLoaded;
  const mlx = page.getByRole('article', { name: 'TRELLIS Text Large · Apple MLX' });
  const cuda = page.getByRole('article', { name: 'TRELLIS Text Large · NVIDIA CUDA' });
  await expect(mlx.getByText('Available to download')).toBeVisible();
  await expect(mlx.getByText('Model file locations', { exact: true })).toHaveCount(0);
  await expect(mlx.getByText(notice)).toHaveCount(0);
  await expect(mlx.getByRole('checkbox')).toHaveCount(0);
  const install = mlx.getByRole('button', { name: 'Install model' });
  await expect(install).toBeEnabled();
  await expect(cuda.getByText('Not supported here')).toBeVisible();
  await expect(cuda.getByText('Model file locations', { exact: true })).toHaveCount(0);
  await expect(cuda.getByRole('button', { name: 'Install model' })).toHaveCount(0);
  const mlxCard = await mlx.boundingBox();
  const cudaCard = await cuda.boundingBox();
  expect(mlxCard.x).toBeLessThan(cudaCard.x);
  expect(mlxCard.y).toEqual(cudaCard.y);
  await expect(page.getByRole('button', { name: 'Recent logs' })).toHaveCount(0);
  await expect(mlx.locator('[data-slot="card-footer"]')).toHaveCount(0);
  await expect(cuda.locator('[data-slot="card-footer"]')).toHaveCount(0);
  expect(unsupportedLogRequests).toBe(0);
  await install.click();
  const installDialog = page.getByRole('alertdialog', { name: 'Install model?' });
  const consent = installDialog.getByRole('checkbox', { name: 'I reviewed the terms and approve installation and download.' });
  const confirm = installDialog.getByRole('button', { name: 'Install model' });
  await expect(installDialog).toContainText('TRELLIS Text Large · Apple MLX');
  await expect(installDialog).toContainText(notice);
  await expect(installDialog.getByRole('link', { name: 'Review upstream source and terms' })).toHaveAttribute('href', 'https://github.com/example/text-model/tree/fixed-revision');
  await expect(confirm).toBeDisabled();
  await consent.check();
  await expect(confirm).toBeEnabled();
  await installDialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(install).toBeFocused();
  expect(installed).toBe(false);
  await install.click();
  await expect(confirm).toBeDisabled();
  await consent.check();
  await confirm.click();
  await expect(installDialog).toBeHidden();
  await expect(mlx.getByText('Ready', { exact: true })).toBeVisible();
  const pathLine = mlx.locator('code');
  await expect(pathLine).toHaveText(paths.join(' · '));
  await expect(pathLine).toHaveCSS('white-space', 'nowrap');
  await expect(pathLine).toHaveCSS('text-overflow', 'ellipsis');
  expect(await pathLine.evaluate(element => element.scrollWidth > element.clientWidth)).toBe(true);
  await mlx.getByRole('button', { name: 'Copy model file locations' }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(paths.join('\n'));
  await expect(mlx.getByRole('button', { name: 'Delete model files' })).toBeEnabled();
  await expect(mlx.getByRole('button', { name: 'Recent logs' })).toBeVisible();
  await expect(cuda.getByRole('button', { name: 'Recent logs' })).toHaveCount(0);
});

test('Pixal3D offers one install for runtime, weights, and MLX conversion', async ({ page, studioOrigin }) => {
  const job = { id: 'pixal-install', operation: 'runtime.models.install', state: 'queued', progress: 0,
    parameters: { backend_id: 'pixal3d-mlx' }, created_at: '1', updated_at: '1' };
  const submitted = [];
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'macos', environment: readyEnvironment,
    model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', backend_ids: ['pixal3d-mlx'] }],
    model_backends: [{ id: 'pixal3d-mlx', installable: true, downloads_enabled: true,
      gated_model_access_required: true, requires_huggingface_login: true,
      huggingface: { account: 'connected', model_access: 'verified' } }],
  } }));
  await page.route('**/api/v1/runtime/pixal3d-mlx/*', route => {
    if (route.request().method() !== 'POST') return route.fallback();
    submitted.push(new URL(route.request().url()).pathname);
    expect(route.request().postDataJSON()).toEqual({ confirmed: true, confirm_gated_access: true });
    return route.fulfill({ status: 202, json: job });
  });
  await page.route(`**/api/v1/jobs/${job.id}`, route => route.fulfill({ json: job }));

  await page.goto(studioOrigin + '/settings/models');
  const profile = page.getByRole('article', { name: 'Pixal3D · Apple MLX' });
  await expect(profile.getByRole('button', { name: /Individual Pixal3D steps|Set up Pixal3D runtime|Download Pixal3D model files|Convert NAF weights/ })).toHaveCount(0);
  await profile.getByRole('button', { name: 'Install model', exact: true }).click();
  const dialog = page.getByRole('alertdialog', { name: 'Install model?' });
  const confirm = dialog.getByRole('button', { name: 'Install model', exact: true });
  await expect(dialog).toContainText('Includes NAF conversion.');
  await expect(confirm).toBeDisabled();
  expect(submitted).toEqual([]);
  await dialog.getByRole('checkbox').check();
  await confirm.click();
  await expect.poll(() => submitted).toEqual(['/api/v1/runtime/pixal3d-mlx/install']);
});

test('Repart installs the GeoSAM2 backend and weights in one job', async ({ page, studioOrigin }) => {
  let requests = 0;
  let backendInstalled = false;
  let weightsDownloaded = false;
  let saved;
  const checkpoint = '/local/runtimes/geosam2-managed/weights/geosam2.pt';
  const job = {
    id: 'geosam2-install', operation: 'runtime.models.install', state: 'queued', progress: 0,
    parameters: { backend_id: 'geosam2-mlx' }, created_at: '1', updated_at: '1',
  };
  const runtime = () => ({
    platform: 'macos',
    environment: readyEnvironment,
    model_catalog: [
      { id: 'pixal3d', title: 'Pixal3D', category: 'generation', backend_ids: ['pixal3d-mlx'] },
      { id: 'geosam2', title: 'GeoSAM2', category: 'repart', backend_ids: ['geosam2-mlx', 'geosam2-cuda'], status: 'experimental_opt_in' },
    ],
    model_backends: [
      { id: 'pixal3d-mlx', installable: false },
      { id: 'geosam2-mlx', ready: false, installable: true, environment_ready: backendInstalled,
        models_downloaded: weightsDownloaded, model_paths: weightsDownloaded ? [checkpoint] : [],
        local_files: { remove_models_available: weightsDownloaded } },
      { id: 'geosam2-cuda', ready: false, installable: false },
    ],
  });
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: runtime() }));
  await page.route('**/api/v1/runtime/geosam2-mlx/jobs', route => route.fulfill({ json: saved ? [saved] : [] }));
  await page.route('**/api/v1/runtime/geosam2-mlx/install', route => {
    expect(route.request().method()).toBe('POST');
    expect(route.request().postDataJSON()).toEqual({ confirmed: true, confirm_gated_access: false });
    requests++;
    return route.fulfill({ status: 202, json: job });
  });
  await page.route(`**/api/v1/jobs/${job.id}`, route => {
    backendInstalled = true;
    weightsDownloaded = true;
    saved = { ...job, state: 'succeeded', progress: 100, updated_at: '2' };
    return route.fulfill({ json: saved });
  });
  await page.route('**/api/v1/runtime/geosam2-mlx/model-files', route => {
    expect(route.request().method()).toBe('DELETE');
    weightsDownloaded = false;
    return route.fulfill({ json: runtime() });
  });
  await page.goto(studioOrigin + '/settings/models');
  const generation = page.getByRole('region', { name: '3D Generation' });
  const repart = page.getByRole('region', { name: 'Repart', exact: true });
  await expect(generation.getByRole('heading', { name: 'Pixal3D' })).toBeVisible();
  await expect(generation.getByRole('heading', { name: 'GeoSAM2' })).toHaveCount(0);
  await expect(repart.getByRole('heading', { name: 'GeoSAM2' })).toBeVisible();
  await expect(repart.getByText('Image to 3D')).toHaveCount(0);
  await expect(repart.getByText(/Studio does not run Repart yet/)).toBeVisible();
  const mlx = repart.getByRole('article', { name: 'GeoSAM2 · Apple MLX' });
  const cuda = repart.getByRole('article', { name: 'GeoSAM2 · NVIDIA CUDA' });
  await expect(cuda.getByText('Not supported here')).toBeVisible();
  await expect(cuda.getByRole('button', { name: 'Install model' })).toHaveCount(0);
  const install = mlx.getByRole('button', { name: 'Install model' });
  await expect(mlx.getByRole('button')).toHaveCount(1);
  await install.click();
  const dialog = page.getByRole('alertdialog', { name: 'Install model?' });
  const confirm = dialog.getByRole('button', { name: 'Install model' });
  await expect(dialog).toContainText('installs the GeoSAM2 backend');
  await expect(dialog).toContainText('616 MB');
  await expect(confirm).toBeDisabled();
  expect(requests).toBe(0);
  await dialog.getByRole('checkbox').check();
  await confirm.click();
  await expect(mlx.getByText('Files installed')).toBeVisible();
  await expect(mlx.locator('code')).toHaveText(checkpoint);
  await expect(install).toHaveCount(0);
  await expect(mlx.getByText('Ready', { exact: true })).toHaveCount(0);
  await expect(page.getByText('GeoSAM2 backend and weights installed for the command-line prototype.')).toBeVisible();
  expect(requests).toBe(1);
  await mlx.getByRole('button', { name: 'Delete model files' }).click();
  const remove = page.getByRole('alertdialog', { name: 'Remove model files?' });
  await expect(remove).toContainText('weights shared by MLX and CUDA');
  await remove.getByRole('button', { name: 'Remove files' }).click();
  await expect(mlx.getByText('Backend installed')).toBeVisible();
  await expect(install).toBeEnabled();
});

test('a gated catalog install reads server access state and supports an explicit recheck', async ({ page, studioOrigin }) => {
  let checks = 0;
  let modelAccess = 'access_denied';
  let finishCheck;
  const job = {
    id: 'gated-install', operation: 'runtime.models.install', state: 'queued', progress: 0,
    parameters: { backend_id: 'trellis2-mlx' }, created_at: '1', updated_at: '1',
  };
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'macos',
    environment: readyEnvironment,
    model_catalog: [{ id: 'trellis2', title: 'TRELLIS.2', backend_ids: ['trellis2-mlx'] }],
    model_backends: [
      { id: 'trellis2-mlx', managed_install: true, installable: true, downloads_enabled: true, gated_model_access_required: true },
      { id: 'pixal3d-mlx', installable: true, huggingface: { account: 'connected', model_access: modelAccess } },
    ],
  } }));
  await page.route('**/api/v1/runtime/pixal3d-mlx/model-access', async route => {
    expect(route.request().method()).toBe('POST');
    ++checks;
    await new Promise(resolve => { finishCheck = resolve; });
    modelAccess = 'verified';
    return route.fulfill({ json: modelAccess });
  });
  await page.route('**/api/v1/runtime/trellis2-mlx/install', route => {
    expect(route.request().postDataJSON()).toEqual({ confirmed: true, confirm_gated_access: true });
    return route.fulfill({ status: 202, json: job });
  });
  await page.route('**/api/v1/jobs/gated-install', route => route.fulfill({ json: job }));
  await page.goto(studioOrigin + '/settings/models');
  const install = page.getByRole('article', { name: 'TRELLIS.2 · Apple MLX' }).getByRole('button', { name: 'Install model' });
  await expect(install).toBeEnabled();
  expect(checks).toBe(0);
  await install.click();
  const dialog = page.getByRole('alertdialog', { name: 'Install model?' });
  const title = dialog.getByRole('heading', { name: 'Install model?' });
  const confirm = dialog.getByRole('button', { name: 'Install model' });
  const consent = dialog.getByRole('checkbox', { name: /I have DINOv3 access/ });
  await expect(title).toHaveCSS('font-size', '18px');
  await expect(dialog.getByText('Access required', { exact: true })).toBeVisible();
  await expect(consent).toBeDisabled();
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(install).toBeFocused();
  await install.click();
  expect(checks).toBe(0);
  await dialog.getByRole('button', { name: 'Recheck access' }).click();
  await expect(dialog.getByText('Checking DINOv3 file access…')).toBeVisible();
  await expect(confirm).toBeDisabled();
  await expect.poll(() => typeof finishCheck).toBe('function');
  finishCheck();
  await expect(dialog.getByText('DINOv3 file access verified.')).toBeVisible();
  await expect(confirm).toBeDisabled();
  await consent.check();
  const request = page.waitForRequest('**/api/v1/runtime/trellis2-mlx/install');
  await confirm.click();
  await request;
  await expect(dialog).toBeHidden();
});

test('the icon-only rail exposes individual mesh tools without a model', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  const rail = page.getByRole('group', { name: 'Model tools' });
  const cleanup = rail.getByRole('button', { name: 'Cleanup', exact: true });
  const icon = cleanup.locator('svg');
  const generation = rail.getByRole('button', { name: 'New model' });
  const viewport = page.getByRole('region', { name: 'Start a model' });
  await expect(generation).toHaveAttribute('aria-current', 'true');
  await expect(cleanup).toHaveText('');
  await expect(icon).toHaveCSS('width', '20px');
  await expect(page.getByRole('complementary', { name: 'Generate model' })).toBeVisible();
  const initialRail = await rail.boundingBox();
  const initialViewport = await viewport.boundingBox();
  const iconBox = await cleanup.boundingBox();
  expect(iconBox.width).toBe(iconBox.height);
  expect(initialRail.height).toBe(initialViewport.height);
  const highlight = await generation.evaluate(element => getComputedStyle(element).backgroundColor);
  expect(highlight).not.toBe('rgba(0, 0, 0, 0)');
  await cleanup.hover();
  await expect(cleanup).not.toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
  const tooltip = page.getByRole('tooltip', { name: 'Cleanup', exact: true });
  await expect(tooltip).toBeVisible();
  expect(await rail.boundingBox()).toEqual(initialRail);
  expect(await cleanup.boundingBox()).toEqual(iconBox);
  expect(await viewport.boundingBox()).toEqual(initialViewport);
  await page.mouse.move(600, 20, { steps: 8 });
  await expect(tooltip).toBeHidden();
  await cleanup.focus();
  await expect(tooltip).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(tooltip).toBeHidden();
  expect(await rail.boundingBox()).toEqual(initialRail);
  await cleanup.press('Enter');
  const cleanupPanel = page.getByRole('complementary', { name: 'Cleanup' });
  await expect(cleanupPanel.getByRole('button', { name: 'Run Cleanup' })).toBeDisabled();
  await expect(cleanup).toHaveAttribute('aria-current', 'true');
  await expect(rail.locator('[aria-current=true]')).toHaveCount(1);
  for (const tool of ['Retopology', 'UV atlas rebake', 'Render optimization', 'Compression']) {
    await expect(rail.getByRole('button', { name: tool, exact: true })).toBeVisible();
  }
  await expect(rail.getByRole('button', { name: 'Separate parts', exact: true })).toHaveCount(0);
  await expect(rail.getByRole('button', { name: 'Jobs' })).toHaveCount(0);
  await rail.getByRole('button', { name: 'Optimize', exact: true }).click();
  const optimize = page.getByRole('complementary', { name: 'Optimize mesh' });
  await expect(optimize.getByRole('button', { name: 'Choose a 3D model' })).toBeVisible();
  await expect(optimize.getByRole('button', { name: 'Optimize mesh' })).toBeDisabled();
  await generation.click();
  await expect(generation).toHaveAttribute('aria-current', 'true');
  await expect(cleanupPanel).toHaveCount(0);
  await expect(page.getByRole('complementary', { name: 'Generate model' })).toBeVisible();
});

test('long model names do not overflow gallery cards at narrow panel and mobile widths', async ({ page, studioOrigin }) => {
  const name = 'MeshHarbor-reference-asset-with-an-extraordinarily-long-name-generated-simplified.glb';
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.getByRole('button', { name: 'Open GLB', exact: true }).click(),
  ]);
  await chooser.setFiles({ name, mimeType: 'model/gltf-binary', buffer: texturedGlb() });

  const assertNoOverflow = async (library) => {
    const item = library.getByRole('button', { name: new RegExp(name.replace('.', '\\.')) });
    await expect(item).toBeVisible();
    await expect(item.getByText(/tris · Imported/)).toBeVisible();
    await expect(item.getByTitle(name, { exact: true })).toHaveCSS('text-overflow', 'ellipsis');
    const bounds = await item.evaluate((card) => {
      const filename = card.querySelector('[title]');
      return {
        truncated: filename.scrollWidth > filename.clientWidth,
        titleRight: filename.getBoundingClientRect().right,
        cardRight: card.getBoundingClientRect().right,
        overflow: card.scrollWidth - card.clientWidth,
      };
    });
    expect(bounds.truncated).toBe(true);
    expect(bounds.titleRight).toBeLessThan(bounds.cardRight);
    expect(bounds.overflow).toBe(0);
  };

  const dock = page.locator('#workbench-sidebar-dock');
  await assertNoOverflow(dock);
  const handle = page.getByRole('separator', { name: 'Resize library panel' });
  const defaultWidth = await dock.evaluate(element => element.clientWidth);
  await handle.press('ArrowRight');
  await expect.poll(() => dock.evaluate(element => element.clientWidth)).toBeLessThan(defaultWidth);
  await assertNoOverflow(dock);

  await page.setViewportSize({ width: 320, height: 720 });
  const inventory = page.getByRole('button', { name: 'Inventory', exact: true });
  await expect(inventory).toHaveAttribute('aria-expanded', 'false');
  await inventory.click();
  await assertNoOverflow(page.getByRole('dialog', { name: 'Inventory' }));
  expect(await page.locator('html').evaluate(element => element.scrollWidth)).toBe(320);
});

test('workbench and settings controls keep the official Mira dimensions', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  await expect(page.getByRole('banner').getByText('meshharbor', { exact: true })).toBeVisible();
  await expect(page.getByRole('banner')).not.toContainText(/Studio|Local engine ready/);
  await expect(page.getByRole('banner').getByRole('button', { name: 'Open GLB' })).toHaveCount(0);
  await expect(page.getByRole('complementary', { name: 'Inventory' }).getByRole('button', { name: 'Open GLB' })).toHaveCSS('height', '24px');
  const settings = page.getByRole('button', { name: 'Settings', exact: true });
  await expect(settings).toHaveCSS('height', '24px');
  await expect(settings).toHaveCSS('column-gap', '4px');
  await settings.hover();
  const settingsBox = await settings.boundingBox();
  await page.mouse.down();
  expect(await settings.evaluate(element => element.matches(':active'))).toBe(true);
  await expect(settings).toHaveCSS('translate', 'none');
  expect(await settings.boundingBox()).toEqual(settingsBox);
  await page.mouse.up();
  await expect(page.getByRole('heading', { name: 'Pixal3D' })).toBeVisible();
  await expect(page.getByRole('banner')).not.toContainText('Engine connected');
  await page.getByRole('button', { name: 'Connections', exact: true }).click();
  await expect(page.getByLabel('Backend', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Downloads & storage', exact: true })).toHaveCount(0);
  expect(await page.locator('html').evaluate(element => getComputedStyle(element).getPropertyValue('--primary').trim())).toBe('#82d7b8');
});

test('preferences can switch the theme and remember it across reloads', async ({ page, studioOrigin }) => {
  await page.route('**/api/v1/health', route => route.abort());
  await page.goto(studioOrigin + '/settings/preferences');
  const preferences = page.getByRole('main', { name: 'Preferences' });
  const theme = preferences.getByLabel('Theme');
  const html = page.locator('html');
  const toasts = page.getByRole('region', { name: /Notifications/ });
  await expect(theme).toHaveValue('dark');
  await theme.selectOption('light');
  await expect(html).toHaveCSS('color-scheme', 'light');
  await expect(toasts.getByText('Light theme enabled.')).toBeVisible();
  await page.reload();
  await expect(theme).toHaveValue('light');
  await expect(html).toHaveCSS('color-scheme', 'light');
  await theme.selectOption('dark');
  await expect(html).toHaveCSS('color-scheme', 'dark');
});

test('refresh results use one global toast across settings and workbench', async ({ page, studioOrigin }) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  let failRefresh = false;
  await page.route('**/api/v1/jobs?limit=20&include_active=true', route => failRefresh
    ? route.fulfill({ status: 503, json: { error: 'Job list unavailable.' } })
    : route.continue());
  await page.goto(studioOrigin + '/settings/models');
  const settings = page.getByRole('main', { name: 'Models' });
  await expect(settings).toHaveAttribute('data-connection', 'connected');
  const toasts = page.getByRole('region', { name: /Notifications/ });
  const refresh = settings.getByRole('button', { name: 'Refresh status' });
  await refresh.click();
  const refreshed = toasts.getByText('Local status refreshed.');
  await expect(refreshed).toBeVisible();
  const notification = toasts.locator('[data-sonner-toast][data-type=success]');
  await expect(notification).toHaveAttribute('data-mounted', 'true');
  expect(await notification.evaluate(element => getComputedStyle(element).transitionProperty)).toContain('opacity');
  await expect(settings.getByText('Local status refreshed.')).toHaveCount(0);
  await toasts.getByRole('button', { name: 'Dismiss notification' }).click();
  await expect(refreshed).toHaveCount(0);
  failRefresh = true;
  await refresh.click();
  const failed = toasts.getByText('Could not refresh recent job records. Job list unavailable.');
  await expect(failed).toBeVisible();
  await expect(refreshed).toHaveCount(0);
  await page.getByRole('button', { name: 'Workbench', exact: true }).click();
  await expect(failed).toBeVisible();
  await page.getByRole('button', { name: 'Refresh assets' }).click();
  await expect(toasts.getByText('Asset library refreshed.')).toBeVisible();
  failRefresh = false;
  await page.getByRole('banner').getByRole('button', { name: 'Jobs' }).click();
  await page.getByRole('complementary', { name: 'Jobs' }).getByRole('button', { name: 'Refresh' }).click();
  await expect(toasts.getByText('Job history refreshed.')).toBeVisible();
});

test('on a narrow viewport setup is accessible without an asset and never starts a model download', async ({ page, studioOrigin }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  let finishSetup = false;
  let tokenSaved = false;
  let downloads = 0;
  let modelAccessChecks = 0;
  const createdAt = String(Date.now());
  const setupJob = {
    id: 'setup-mlx',
    operation: 'runtime.environment.setup',
    state: 'queued',
    progress: 0,
    created_at: createdAt,
    updated_at: createdAt,
  };
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'macos',
    status: 'ready_for_mesh_operations',
    environment: { ...readyEnvironment, ready: finishSetup, uv_found: true, python_versions_found: ['3.12'] },
    model_catalog: [
      { id: 'pixal3d', title: 'Pixal3D', status: 'experimental_opt_in', backend_ids: ['pixal3d-mlx', 'pixal3d-cuda'] },
      { id: 'geosam2', title: 'GeoSAM2', category: 'repart', backend_ids: ['geosam2-mlx'] },
    ],
    model_backends: [{
      id: 'pixal3d-mlx',
      installable: true,
      ready: false,
      account_tools_ready: finishSetup,
      downloads_enabled: true,
      requires_huggingface_login: true,
      gated_model_access_required: true,
      weights_included: false,
      models_downloaded: false,
      local_files: { token_saved: tokenSaved, credentials_managed: true },
      huggingface: { account: tokenSaved ? 'connected' : 'not_connected', model_access: tokenSaved ? 'verified' : 'not_connected' },
      pixal3d_cache: { local_checkpoints: 0, reusable_checkpoints: 7, total_checkpoints: 7 },
    }, {
      id: 'pixal3d-cuda',
      installable: false,
      weights_included: false,
    }, {
      id: 'geosam2-mlx', installable: true, ready: false, requires_huggingface_login: true,
    }],
  } }));
  await page.route('**/api/v1/runtime/environment/setup', route => {
    expect(route.request().method()).toBe('POST');
    expect(route.request().postDataJSON()).toEqual({ confirmed: true });
    return route.fulfill({ status: 202, json: setupJob });
  });
  await page.route('**/api/v1/jobs/setup-mlx', route => route.fulfill({ json: {
    ...setupJob,
    state: finishSetup ? 'succeeded' : 'running',
    progress: finishSetup ? 100 : 5,
  } }));
  await page.route('**/api/v1/runtime/*/install', route => {
    downloads += 1;
    return route.abort();
  });
  await page.route('**/api/v1/runtime/pixal3d-mlx/huggingface', route => {
    const { token } = route.request().postDataJSON();
    expect(route.request().method()).toBe('POST');
    expect(route.request().headers()['x-meshharbor-dashboard']).toBe('local-ui-v1');
    if (token === 'hf_testapproved') tokenSaved = true;
    return route.fulfill({ json: token === 'hf_testapproved'
      ? 'connected' : token ? 'invalid_token' : 'not_connected' });
  });
  await page.route('**/api/v1/runtime/pixal3d-mlx/model-access', route => {
    modelAccessChecks++;
    return route.fulfill({ json: 'verified' });
  });

  await page.goto(studioOrigin);
  const inventory = page.getByRole('button', { name: 'Inventory', exact: true });
  await expect(inventory).toHaveAttribute('aria-expanded', 'false');
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  await inventory.click();
  const library = page.getByRole('dialog', { name: 'Inventory' });
  await expect(library).toBeVisible();
  await expect(page.getByRole('separator', { name: 'Resize library panel' })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(library).toBeHidden();
  await expect(inventory).toBeFocused();
  const settings = page.getByRole('button', { name: 'Settings', exact: true });
  await settings.click();
  await expect(page).toHaveURL(/\/settings\/models$/);
  const main = page.getByRole('main', { name: 'Models' });
  const storage = main.getByRole('article', { name: 'Pixal3D · Apple MLX' });
  const repart = main.getByRole('article', { name: 'GeoSAM2 · Apple MLX' });
  for (const card of [storage, repart]) {
    await expect(card).toHaveAttribute('aria-disabled', 'true');
    await expect(card.getByRole('button', { name: 'Install model' })).toBeDisabled();
  }
  await expect(main.getByText('Not supported here')).toBeVisible();
  await main.getByRole('button', { name: 'Go to Environment' }).click();
  await expect(page).toHaveURL(/\/settings\/environment$/);
  const environment = page.getByRole('main', { name: 'Environment' });
  const setup = environment.getByRole('button', { name: 'Set up environment' });
  await expect(environment).toContainText('Python 3.10 and 3.12');
  await expect(environment.getByText('Detected: uv, Python 3.12')).toBeVisible();
  await setup.click();
  const setupDialog = page.getByRole('alertdialog', { name: 'Set up environment?' });
  const confirmSetup = setupDialog.getByRole('button', { name: 'Set up environment' });
  await expect(confirmSetup).toBeDisabled();
  await setupDialog.getByRole('checkbox').check();
  await confirmSetup.click();
  await expect(page.getByRole('region', { name: /Notifications/ }).getByText('Set up environment queued.')).toBeVisible();
  await expect(setup).toBeDisabled();
  finishSetup = true;
  await expect(environment.getByText('Ready', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Models', exact: true }).click();
  const toasts = page.getByRole('region', { name: /Notifications/ });
  const download = storage.getByRole('button', { name: 'Install model' });
  await expect(download).toBeDisabled();
  await expect(repart.getByRole('button', { name: 'Install model' })).toBeDisabled();
  await expect(repart.getByText('Sign-in required')).toBeVisible();
  await storage.getByRole('button', { name: 'Go to Connections' }).click();
  const connections = page.getByRole('main', { name: 'Connections' });
  await expect(connections).not.toContainText('DINOv3');
  await expect(connections.getByRole('button', { name: 'Sign in with browser' })).toBeEnabled();
  await connections.getByRole('button', { name: 'Check sign-in' }).click();
  await expect(toasts.getByText('No local token is saved', { exact: false })).toBeVisible();
  await connections.getByText('Use an access token instead').click();
  await expect(connections.getByRole('link', { name: 'Create a Hugging Face token' })).toHaveAttribute('href', 'https://huggingface.co/settings/tokens');
  const token = connections.getByLabel('Hugging Face access token');
  const connect = connections.getByRole('button', { name: 'Connect token' });
  await expect(token).toHaveAttribute('type', 'password');
  await token.fill('hf_testdenied');
  await connect.click();
  await expect(toasts.getByText('Hugging Face did not accept this token', { exact: false })).toBeVisible();
  await expect(token).toHaveValue('');
  await token.fill('hf_testapproved');
  await connect.click();
  await expect(toasts.getByText('Hugging Face account verified.')).toBeVisible();
  await expect(token).toHaveValue('');
  await page.getByRole('button', { name: 'Models', exact: true }).click();
  await expect(download).toBeEnabled();
  await expect(repart.getByRole('button', { name: 'Install model' })).toBeEnabled();
  expect(modelAccessChecks).toBe(0);
  await download.click();
  const downloadDialog = page.getByRole('alertdialog', { name: 'Install model?' });
  const confirm = downloadDialog.getByRole('button', { name: 'Install model' });
  const consent = downloadDialog.getByRole('checkbox');
  await expect(downloadDialog).toContainText('up to 24 GB');
  await expect(downloadDialog).toContainText('7 more can be reused');
  await expect(downloadDialog.getByText('DINOv3 file access verified.')).toBeVisible();
  expect(modelAccessChecks).toBe(0);
  await expect(confirm).toBeDisabled();
  await consent.check();
  await expect(confirm).toBeEnabled();
  expect(downloads).toBe(0);
  expect(await page.locator('html').evaluate(element => element.scrollWidth)).toBe(390);
  await downloadDialog.getByRole('button', { name: 'Cancel' }).click();
  await page.getByRole('button', { name: 'Workbench', exact: true }).click();
  await expect(page).toHaveURL(studioOrigin + '/');
  await expect(settings).toBeFocused();
});

test('a browser sign-in updates settings and DINOv3 can be rechecked manually', async ({ page, studioOrigin }) => {
  let sessionNumber = 0;
  let cancelled = false;
  let approved = false;
  let tokenSaved = false;
  let modelAccessChecks = 0;
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'macos',
    status: 'ready_for_mesh_operations',
    environment: readyEnvironment,
    model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', status: 'experimental_opt_in', backend_ids: ['pixal3d-mlx'] }],
    model_backends: [{
      id: 'pixal3d-mlx', installable: true, environment_ready: true, downloads_enabled: true,
      gated_model_access_required: true, requires_huggingface_login: true,
      local_files: { token_saved: tokenSaved, disconnect_available: tokenSaved, credentials_managed: true },
      huggingface: { account: tokenSaved ? 'connected' : 'not_connected', model_access: !tokenSaved ? 'not_connected' : modelAccessChecks ? 'verified' : 'access_denied' },
    }],
  } }));
  await page.route('**/api/v1/runtime/pixal3d-mlx/huggingface/browser', route => {
    expect(route.request().method()).toBe('POST');
    cancelled = false;
    return route.fulfill({ status: 202, json: { id: String(++sessionNumber), state: 'preparing' } });
  });
  await page.route('**/api/v1/runtime/pixal3d-mlx/huggingface/browser/*', route => {
    const id = String(sessionNumber);
    if (route.request().method() === 'DELETE') cancelled = true;
    if (cancelled) return route.fulfill({ json: { id, state: 'cancelled' } });
    if (approved) {
      tokenSaved = true;
      return route.fulfill({ json: { id, state: 'signed_in' } });
    }
    return route.fulfill({ json: { id, state: 'awaiting_authorization', user_code: 'TEST-CODE', expires_in: 300 } });
  });
  await page.route('**/api/v1/runtime/pixal3d-mlx/huggingface', route => {
    expect(route.request().postDataJSON()).toEqual({});
    return route.fulfill({ json: 'connected' });
  });
  await page.route('**/api/v1/runtime/pixal3d-mlx/model-access', route => {
    expect(route.request().method()).toBe('POST');
    expect(route.request().postData()).toBeNull();
    ++modelAccessChecks;
    return route.fulfill({ json: 'verified' });
  });
  await page.goto(studioOrigin + '/settings/connections');
  const connections = page.getByRole('main', { name: 'Connections' });
  const toasts = page.getByRole('region', { name: /Notifications/ });
  const signIn = connections.getByRole('button', { name: 'Sign in with browser' });
  await expect(connections.getByLabel('Hugging Face access token')).toBeHidden();
  await signIn.click();
  await expect(connections.getByText('TEST-CODE')).toBeVisible();
  await expect(connections.getByRole('link', { name: 'Open Hugging Face' })).toHaveAttribute('href', 'https://huggingface.co/oauth/device');
  await connections.getByRole('button', { name: 'Cancel sign-in' }).click();
  await expect(toasts.getByText('Hugging Face sign-in cancelled.')).toBeVisible();
  await signIn.click();
  await expect(connections.getByText('TEST-CODE')).toBeVisible();
  approved = true;
  await expect(toasts.getByText('Signed in to Hugging Face.')).toBeVisible();
  await expect(connections.getByText('Signed in', { exact: true })).toBeVisible();
  expect(modelAccessChecks).toBe(0);
  await expect(connections).not.toContainText('DINOv3');
  await page.getByRole('button', { name: 'Models', exact: true }).click();
  const storage = page.getByRole('article', { name: 'Pixal3D · Apple MLX' });
  await expect(storage.getByText('Available to download')).toBeVisible();
  expect(modelAccessChecks).toBe(0);
  await storage.getByRole('button', { name: 'Install model' }).click();
  const downloadDialog = page.getByRole('alertdialog', { name: 'Install model?' });
  const confirm = downloadDialog.getByRole('button', { name: 'Install model' });
  await expect(downloadDialog.getByText('Access required', { exact: true })).toBeVisible();
  await expect(confirm).toBeDisabled();
  await downloadDialog.getByRole('button', { name: 'Recheck access' }).click();
  await expect(downloadDialog.getByText('DINOv3 file access verified.')).toBeVisible();
  await expect(confirm).toBeDisabled();
});

test('settings deep links and confirmations manage one local profile at a time', async ({ page, studioOrigin }) => {
  let tokenSaved = true;
  let modelsInstalled = true;
  const modelPath = '/meshharbor/runtimes/pixal3d-mlx/home/.cache/huggingface/hub/models--fixture--a-long-model-directory';
  const calls = [];
  let finishModelRemoval;
  const runtime = () => ({
    platform: 'macos',
    status: 'ready_for_mesh_operations',
    environment: readyEnvironment,
    model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', status: 'experimental_opt_in', backend_ids: ['pixal3d-mlx', 'pixal3d-cuda'] }],
    model_backends: [{
      id: 'pixal3d-mlx', installable: true, ready: modelsInstalled,
      gated_model_access_required: true, requires_huggingface_login: true,
      environment_ready: true, downloads_enabled: !modelsInstalled, models_downloaded: modelsInstalled,
      model_paths: [modelPath],
      local_files: { token_saved: tokenSaved, disconnect_available: tokenSaved, remove_models_available: modelsInstalled, credentials_managed: true, model_files_managed: true },
      huggingface: { account: tokenSaved ? 'unavailable' : 'not_connected', model_access: tokenSaved ? 'verified' : 'not_connected' },
    }, { id: 'pixal3d-cuda', installable: false }],
  });
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: runtime() }));
  await page.route('**/api/v1/runtime/pixal3d-mlx/model-access', route => route.fulfill({ json: 'verified' }));
  for (const scope of ['model-files', 'huggingface']) {
    await page.route(`**/api/v1/runtime/pixal3d-mlx/${scope}`, async route => {
      expect(route.request().method()).toBe('DELETE');
      expect(route.request().postDataJSON()).toEqual({ confirmed: true });
      calls.push(scope);
      if (scope === 'model-files') {
        await new Promise(resolve => { finishModelRemoval = resolve; });
        modelsInstalled = false;
      } else tokenSaved = false;
      await route.fulfill({ json: runtime() });
    });
  }
  await page.goto(studioOrigin + '/settings/storage');
  const storage = page.getByRole('main', { name: 'Models' }).getByRole('article', { name: 'Pixal3D · Apple MLX' });
  const toasts = page.getByRole('region', { name: /Notifications/ });
  const uninstall = storage.getByRole('button', { name: 'Delete model files' });
  await expect(storage.getByText('Ready', { exact: true })).toBeVisible();
  const path = storage.getByText(modelPath, { exact: true });
  const tooltip = page.getByRole('tooltip');
  await path.focus();
  await expect(path).toBeFocused();
  await expect(tooltip).toContainText(modelPath);
  await page.keyboard.press('Escape');
  await expect(tooltip).toBeHidden();
  await page.keyboard.press('Tab');
  await expect(storage.getByRole('button', { name: 'Copy model file locations' })).toBeFocused();
  await path.hover();
  await expect(tooltip).toContainText(modelPath);
  await page.keyboard.press('Escape');
  await expect(tooltip).toBeHidden();
  await uninstall.click();
  const remove = page.getByRole('alertdialog', { name: 'Remove model files?' });
  await expect(remove).toContainText('assets stay intact');
  await expect(remove.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await page.mouse.click(8, 8);
  await expect(remove).toBeVisible();
  await remove.getByRole('button', { name: 'Cancel' }).click();
  await expect(uninstall).toBeFocused();
  expect(calls).toEqual([]);
  await uninstall.click();
  await remove.getByRole('button', { name: 'Remove files' }).click();
  await expect.poll(() => typeof finishModelRemoval).toBe('function');
  await expect(remove.getByRole('button', { name: 'Removing…' })).toBeDisabled();
  await page.keyboard.press('Escape');
  await expect(remove).toBeVisible();
  finishModelRemoval();
  await expect(toasts.getByText('Model files removed for Pixal3D · Apple MLX.')).toBeVisible();
  await expect(storage.getByText('Sign-in required', { exact: true })).toBeVisible();
  await expect(storage.getByText('Model file locations', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Connections', exact: true }).click();
  const connections = page.getByRole('main', { name: 'Connections' });
  await expect(connections.getByText('Token saved', { exact: true })).toBeVisible();
  await expect(connections.getByLabel('Backend', { exact: true })).toHaveCount(0);
  await connections.getByRole('button', { name: 'Disconnect locally' }).click();
  const disconnect = page.getByRole('alertdialog', { name: 'Disconnect Hugging Face?' });
  await expect(disconnect).toContainText('Your Hugging Face token and downloaded models stay intact.');
  await disconnect.getByRole('button', { name: 'Disconnect', exact: true }).click();
  await expect(toasts.getByText('Hugging Face disconnected on this computer.')).toBeVisible();
  await expect(connections.getByText('Not connected', { exact: true })).toBeVisible();
  expect(calls).toEqual(['model-files', 'huggingface']);
  await page.goBack();
  await expect(storage.getByText('Sign-in required', { exact: true })).toBeVisible();
  await expect(storage.getByRole('button', { name: 'Install model' })).toBeDisabled();
  await expect(storage.getByRole('button', { name: 'Go to Connections' })).toBeVisible();
});
