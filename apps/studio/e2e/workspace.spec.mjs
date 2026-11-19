import { expect } from '@playwright/test';
import { test } from './studio-fixture.mjs';
import { texturedGlb } from './textured-glb.mjs';

test('the desktop library opens on first visit and remembers a collapsed dock', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const inventory = page.getByRole('button', { name: 'Inventory', exact: true });
  await expect(inventory).toHaveAttribute('aria-expanded', 'true');
  await page.getByRole('button', { name: 'Collapse sidebar' }).click();
  await expect(inventory).toBeFocused();
  await page.reload();
  await expect(inventory).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByRole('group', { name: 'Model tools' }).getByRole('button', { name: 'Jobs', exact: true })).toBeVisible();
  await inventory.click();
  await expect(inventory).toHaveAttribute('aria-expanded', 'true');
});

test('Jobs opens on the left without replacing the resizable Inventory', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const library = page.getByRole('complementary', { name: 'Inventory' });
  const jobs = page.getByRole('group', { name: 'Model tools' }).getByRole('button', { name: 'Jobs', exact: true });
  await jobs.click();
  await expect(jobs).toHaveAttribute('aria-current', 'true');
  const activity = page.getByRole('complementary', { name: 'Jobs' });
  const viewport = page.getByRole('region', { name: 'Start a model' });
  await expect(activity.getByText('No saved jobs yet.')).toBeVisible();
  await expect(library.getByRole('button', { name: 'Open GLB' })).toBeVisible();
  const [left, center, right] = await Promise.all([activity.boundingBox(), viewport.boundingBox(), library.boundingBox()]);
  expect(left.x + left.width).toBeLessThanOrEqual(center.x);
  expect(Math.round(right.x + right.width)).toBe(1280);
  await page.keyboard.press('Escape');
  await expect(activity).toBeVisible();
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

test('Jobs and storage activity share accessible item rows with failure details', async ({ page, studioOrigin }) => {
  const recorded = {
    id: 'failed-download', operation: 'runtime.pixal3d.install_models', state: 'failed', progress: 0,
    created_at: '1', updated_at: '2', error: { message: 'Local cache is unavailable.' },
  };
  await page.route('**/api/v1/jobs?limit=20&include_active=true', route => route.fulfill({ json: [recorded] }));
  await page.goto(studioOrigin);
  await page.getByRole('group', { name: 'Model tools' }).getByRole('button', { name: 'Jobs' }).click();
  const jobsRow = page.getByRole('complementary', { name: 'Jobs' }).getByRole('listitem');
  await expect(jobsRow).toHaveAttribute('data-slot', 'item');
  await expect(jobsRow.getByText('Failed', { exact: true })).toBeVisible();
  await expect(jobsRow.getByText(recorded.error.message)).toBeVisible();
  await expect(jobsRow.getByRole('button', { name: 'View' })).toBeVisible();
  await page.goto(studioOrigin + '/settings/storage');
  const activityRow = page.getByRole('region', { name: 'Recent activity' }).getByRole('listitem');
  await expect(activityRow).toHaveAttribute('data-slot', 'item');
  await expect(activityRow.getByText('Download MLX models')).toBeVisible();
  await expect(activityRow.getByText(recorded.error.message)).toBeVisible();
});

test('the icon-only rail always shows one panel and exposes mesh tools without a model', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  const rail = page.getByRole('group', { name: 'Model tools' });
  const job = rail.getByRole('button', { name: 'Jobs' });
  const icon = job.locator('svg');
  const generation = rail.getByRole('button', { name: 'New model' });
  const viewport = page.getByRole('region', { name: 'Start a model' });
  await expect(generation).toHaveAttribute('aria-current', 'true');
  await expect(job).toHaveText('');
  await expect(icon).toHaveCSS('width', '20px');
  await expect(page.getByRole('complementary', { name: 'Generate model' })).toBeVisible();
  const initialRail = await rail.boundingBox();
  const initialViewport = await viewport.boundingBox();
  const iconBox = await job.boundingBox();
  expect(iconBox.width).toBe(iconBox.height);
  expect(initialRail.height).toBe(initialViewport.height);
  const highlight = await generation.evaluate(element => getComputedStyle(element).backgroundColor);
  expect(highlight).not.toBe('rgba(0, 0, 0, 0)');
  await job.hover();
  await expect(job).not.toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
  expect(await rail.boundingBox()).toEqual(initialRail);
  expect(await job.boundingBox()).toEqual(iconBox);
  expect(await viewport.boundingBox()).toEqual(initialViewport);
  await expect(page.getByRole('tooltip')).toHaveCount(0);
  await page.mouse.move(600, 20);
  await job.focus();
  expect(await rail.boundingBox()).toEqual(initialRail);
  await job.press('Enter');
  const activity = page.getByRole('complementary', { name: 'Jobs' });
  await expect(activity).toBeVisible();
  await job.click();
  await expect(activity).toBeVisible();
  await job.press('Space');
  await expect(job).toHaveAttribute('aria-current', 'true');
  await expect(rail.locator('[aria-current=true]')).toHaveCount(1);
  await rail.getByRole('button', { name: 'Optimize', exact: true }).click();
  const optimize = page.getByRole('complementary', { name: 'Optimize mesh' });
  await expect(optimize.getByRole('button', { name: 'Choose a 3D model' })).toBeVisible();
  await expect(optimize.getByRole('button', { name: 'Optimize mesh' })).toBeDisabled();
  await rail.getByRole('button', { name: 'More tools' }).click();
  const meshTools = page.getByRole('complementary', { name: 'Mesh tools' });
  await meshTools.getByRole('button', { name: 'Cleanup', exact: true }).click();
  await expect(meshTools.getByRole('button', { name: 'Run cleanup', exact: false })).toBeDisabled();
  await generation.click();
  await expect(generation).toHaveAttribute('aria-current', 'true');
  await expect(activity).toHaveCount(0);
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
    await expect(item.getByText(/tris · Original/)).toBeVisible();
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
  await page.getByRole('button', { name: 'Downloads & storage', exact: true }).click();
  await expect(page.getByLabel('Backend')).toHaveCSS('height', '28px');
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
  await page.getByRole('group', { name: 'Model tools' }).getByRole('button', { name: 'Jobs' }).click();
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
    operation: 'runtime.pixal3d.setup',
    state: 'queued',
    progress: 0,
    created_at: createdAt,
    updated_at: createdAt,
  };
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'macos',
    status: 'ready_for_mesh_operations',
    model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', status: 'experimental_opt_in', backend_ids: ['pixal3d-mlx', 'pixal3d-cuda'] }],
    model_backends: [{
      id: 'pixal3d-mlx',
      installable: true,
      ready: false,
      environment_ready: finishSetup,
      downloads_enabled: true,
      weights_included: false,
      models_downloaded: false,
      local_files: { token_saved: tokenSaved, credentials_managed: true },
      pixal3d_cache: { local_checkpoints: 0, reusable_checkpoints: 7, total_checkpoints: 7 },
    }, {
      id: 'pixal3d-cuda',
      installable: false,
      weights_included: false,
    }],
  } }));
  await page.route('**/api/v1/runtime/pixal3d-mlx/setup', route => {
    expect(route.request().method()).toBe('POST');
    return route.fulfill({ status: 202, json: setupJob });
  });
  await page.route('**/api/v1/jobs/setup-mlx', route => route.fulfill({ json: {
    ...setupJob,
    state: finishSetup ? 'succeeded' : 'running',
    progress: finishSetup ? 100 : 5,
  } }));
  await page.route('**/api/v1/runtime/pixal3d-mlx/models', route => {
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
  const setup = main.getByRole('button', { name: 'Set up Python environment' });
  await expect(main.getByText('Not supported here')).toBeVisible();
  await setup.click();
  const progress = main.getByRole('progressbar', { name: 'Job progress' });
  await expect(progress).toBeVisible();
  await expect(progress).not.toHaveAttribute('aria-valuenow');
  await expect(setup).toBeDisabled();
  finishSetup = true;
  await expect(main.getByText('Download required')).toBeVisible();
  await main.getByRole('button', { name: 'View downloads' }).click();
  const storage = page.getByRole('main', { name: 'Downloads & storage' });
  const toasts = page.getByRole('region', { name: /Notifications/ });
  const download = storage.getByRole('button', { name: 'Download model files' });
  const consent = storage.getByRole('checkbox', { name: 'I reviewed the model terms and approve this download.' });
  await expect(storage.getByRole('link', { name: 'Review terms and request DINOv3 access' })).toHaveAttribute('href', 'https://huggingface.co/facebook/dinov3-vitl16-pretrain-lvd1689m');
  await expect(download).toBeDisabled();
  await expect(consent).toBeDisabled();
  await expect(storage.getByText(/7 more can be reused/)).toBeVisible();
  expect(modelAccessChecks).toBe(0);
  await storage.getByRole('button', { name: 'Manage Hugging Face connection' }).click();
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
  await page.getByRole('button', { name: 'Downloads & storage', exact: true }).click();
  await expect(storage.getByRole('button', { name: 'Check DINOv3 access' })).toHaveCount(0);
  await expect(storage.getByText('DINOv3 file access verified.')).toBeVisible();
  expect(modelAccessChecks).toBeGreaterThan(0);
  await expect(download).toBeDisabled();
  await consent.check();
  await expect(download).toBeEnabled();
  expect(downloads).toBe(0);
  expect(await page.locator('html').evaluate(element => element.scrollWidth)).toBe(390);
  await page.getByRole('button', { name: 'Workbench', exact: true }).click();
  await expect(page).toHaveURL(studioOrigin + '/');
  await expect(settings).toBeFocused();
});

test('browser sign-in leaves DINOv3 to storage, which rechecks access on return', async ({ page, studioOrigin }) => {
  let sessionNumber = 0;
  let cancelled = false;
  let approved = false;
  let tokenSaved = false;
  let modelAccessChecks = 0;
  await page.route('**/api/v1/runtime', route => route.fulfill({ json: {
    platform: 'macos',
    status: 'ready_for_mesh_operations',
    model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', status: 'experimental_opt_in', backend_ids: ['pixal3d-mlx'] }],
    model_backends: [{
      id: 'pixal3d-mlx', installable: true, environment_ready: true, downloads_enabled: true,
      local_files: { token_saved: tokenSaved, disconnect_available: tokenSaved, credentials_managed: true },
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
    return route.fulfill({ json: ++modelAccessChecks === 1 ? 'access_denied' : 'verified' });
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
  await connections.getByRole('button', { name: 'Check sign-in' }).click();
  await expect(toasts.getByText('Hugging Face account verified.')).toBeVisible();
  expect(modelAccessChecks).toBe(0);
  await expect(connections).not.toContainText('DINOv3');
  await page.getByRole('button', { name: 'Downloads & storage', exact: true }).click();
  const storage = page.getByRole('main', { name: 'Downloads & storage' });
  const download = storage.getByRole('button', { name: 'Download model files' });
  await expect(storage.getByRole('button', { name: 'Check DINOv3 access' })).toHaveCount(0);
  await expect(storage.getByText('DINOv3 access denied.', { exact: false })).toBeVisible();
  await expect(download).toBeDisabled();
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(storage.getByText('DINOv3 file access verified.')).toBeVisible();
  await expect(download).toBeDisabled();
});

test('settings deep links and confirmations manage one local profile at a time', async ({ page, studioOrigin }) => {
  let tokenSaved = true;
  let modelsInstalled = true;
  const calls = [];
  let finishModelRemoval;
  const runtime = () => ({
    platform: 'macos',
    status: 'ready_for_mesh_operations',
    model_catalog: [{ id: 'pixal3d', title: 'Pixal3D', status: 'experimental_opt_in', backend_ids: ['pixal3d-mlx', 'pixal3d-cuda'] }],
    model_backends: [{
      id: 'pixal3d-mlx', installable: true, ready: modelsInstalled,
      environment_ready: true, downloads_enabled: !modelsInstalled, models_downloaded: modelsInstalled,
      local_files: { token_saved: tokenSaved, disconnect_available: tokenSaved, remove_models_available: modelsInstalled, credentials_managed: true, model_files_managed: true },
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
  const storage = page.getByRole('main', { name: 'Downloads & storage' });
  const toasts = page.getByRole('region', { name: /Notifications/ });
  const uninstall = storage.getByRole('button', { name: 'Uninstall model files' });
  await expect(storage.getByText('Installed', { exact: true })).toBeVisible();
  await uninstall.click();
  const remove = page.getByRole('alertdialog', { name: 'Remove model files?' });
  await expect(remove).toContainText('all Studio assets stay in place');
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
  await expect(toasts.getByText('Pixal3D model files removed for Apple MLX.')).toBeVisible();
  await expect(storage.getByText('Not installed', { exact: true })).toBeVisible();
  await expect(storage.getByText('DINOv3 file access verified.')).toBeVisible();
  await page.getByRole('button', { name: 'Connections', exact: true }).click();
  const connections = page.getByRole('main', { name: 'Connections' });
  await expect(connections.getByText('Token saved', { exact: true })).toBeVisible();
  await expect(connections.getByLabel('Backend', { exact: true })).toHaveCount(0);
  await connections.getByRole('button', { name: 'Disconnect locally' }).click();
  const disconnect = page.getByRole('alertdialog', { name: 'Disconnect Hugging Face?' });
  await expect(disconnect).toContainText('does not revoke the token');
  await disconnect.getByRole('button', { name: 'Disconnect', exact: true }).click();
  await expect(toasts.getByText('Hugging Face disconnected on this computer.')).toBeVisible();
  await expect(connections.getByText('Not connected', { exact: true })).toBeVisible();
  expect(calls).toEqual(['model-files', 'huggingface']);
  await page.goBack();
  await expect(storage.getByText('Not installed', { exact: true })).toBeVisible();
  await expect(storage.getByText('Sign in to Hugging Face to check DINOv3 file access.')).toBeVisible();
  await expect(storage.getByRole('button', { name: 'Download model files' })).toBeDisabled();
});
