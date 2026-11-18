import { expect } from '@playwright/test';
import { test } from './studio-fixture.mjs';

test('the desktop library opens on first visit and remembers a collapsed dock', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.getByText('Local engine ready', { exact: true })).toBeVisible();
  const assets = page.getByRole('button', { name: 'Assets', exact: true });
  await expect(assets).toHaveAttribute('aria-expanded', 'true');
  await page.getByRole('button', { name: 'Collapse sidebar' }).click();
  await expect(assets).toBeFocused();
  await page.reload();
  await expect(assets).toHaveAttribute('aria-expanded', 'false');
  await assets.click();
  await expect(assets).toHaveAttribute('aria-expanded', 'true');
});

test('on a narrow viewport setup is accessible without an asset and never starts a model download', async ({ page, studioOrigin }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  let finishSetup = false;
  let downloads = 0;
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
    model_backends: [{
      id: 'pixal3d-mlx',
      installable: true,
      ready: false,
      environment_ready: finishSetup,
      downloads_enabled: true,
      weights_included: false,
      models_downloaded: false,
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

  await page.goto(studioOrigin);
  await expect(page.getByRole('button', { name: 'Assets', exact: true })).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByText('Local engine ready', { exact: true })).toBeVisible();
  const runtime = page.getByRole('button', { name: 'Engine and runtimes' });
  await runtime.click();
  const dialog = page.getByRole('dialog', { name: 'Engine and runtimes' });
  const setup = dialog.getByRole('button', { name: 'Set up Python environment' });
  await expect(dialog.getByText('No model files are downloaded', { exact: false })).toBeVisible();
  await setup.click();
  const progress = dialog.getByRole('progressbar', { name: 'Job progress' });
  await expect(progress).toBeVisible();
  await expect(progress).not.toHaveAttribute('aria-valuenow');
  await expect(setup).toBeDisabled();
  finishSetup = true;
  await expect(dialog.getByText('Installed', { exact: true })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Download model files' })).toBeDisabled();
  expect(downloads).toBe(0);
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(runtime).toBeFocused();
});
