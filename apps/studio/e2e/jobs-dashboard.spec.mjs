import { expect } from '@playwright/test';
import { test } from './studio-fixture.mjs';

test('the dashboard polls engine activity, keeps stable completed durations, and loads older records', async ({ page, studioOrigin }) => {
  const now = Date.now();
  const lastFinished = Array.from({ length: 20 }, (_, index) => ({
    id: `job_finished_${index}`, operation: 'mesh.compress', state: 'succeeded', progress: 100,
    created_at: String(now - 45000 - index), started_at: String(now - 30000 - index),
    updated_at: String(now - 5000 - index),
  }));
  const external = {
    id: 'external_generation', operation: 'image.generate', state: 'running', progress: 5, input_asset_id: 'reference',
    input_asset_name: 'external-reference.png',
    started_at: String(now - 10000), created_at: String(now - 20000), updated_at: String(now),
    stage: 'shape_hr',
  };
  const asset = {
    id: 'external_result', kind: 'mesh', name: 'external-generated.glb', source_asset_id: 'reference',
    mime_type: 'model/gltf-binary', byte_size: 128, sha256: '0'.repeat(64), metadata: {}, artifacts: [],
  };
  let jobs = lastFinished;
  await page.route('**/api/v1/jobs?limit=20&include_active=true', route => route.fulfill({ json: jobs }));
  await page.route('**/api/v1/jobs?limit=20&before=job_finished_19', route => route.fulfill({ json: [{
    ...lastFinished.at(-1), id: 'older_job', state: 'failed', operation: 'runtime.pixal3d.install_models',
    error: { message: 'Local cache is unavailable.' },
  }] }));
  await page.route('**/api/v1/assets/external_result', route => route.fulfill({ json: asset }));

  await page.goto(studioOrigin + '/jobs');
  const active = page.getByRole('region', { name: /In progress/ });
  const history = page.getByRole('region', { name: 'History' });
  await expect(active.getByText('Nothing in progress')).toBeVisible();
  await expect(history.getByText('Elapsed: 25s')).toHaveCount(20);
  await history.getByRole('button', { name: 'Load older jobs' }).click();
  await history.getByRole('button', { name: /Show error for/ }).click();
  await expect(history.getByText('Local cache is unavailable.')).toHaveCount(0);
  await expect(page.getByRole('region', { name: /Notifications/ }).getByText('Local cache is unavailable.')).toBeVisible();

  jobs = [external, ...lastFinished];
  await expect(active.getByText('Refining geometry')).toBeVisible();
  const elapsed = active.getByText(/^Elapsed: /);
  const firstTick = await elapsed.textContent();
  await expect(elapsed).not.toHaveText(firstTick);
  await expect(active.getByText('Queued for: 10s')).toBeVisible();
  await expect(active.getByText(external.input_asset_name)).toBeVisible();
  await expect(history.getByText('Elapsed: 25s')).toHaveCount(21);
  await page.getByRole('button', { name: 'Studio', exact: true }).click();
  const inventory = page.getByRole('complementary', { name: 'Inventory' });
  await expect(inventory.getByRole('region', { name: 'Models in progress' }).getByText('Refining geometry')).toBeVisible();
  jobs = [{
    ...external, state: 'succeeded', progress: 100, updated_at: String(now + 2000), result: { asset_id: asset.id },
  }, ...lastFinished.slice(0, -1)];
  await expect(inventory.getByTitle(asset.name)).toBeVisible();
  await expect(inventory.getByRole('region', { name: 'Models in progress' })).toHaveCount(0);
  await expect(page.getByRole('region', { name: 'Start a model' })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Jobs dashboard' })).toBeVisible();
  await expect(history.getByText(external.input_asset_name)).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(390);
});

test('stopping a running generation requires confirmation and cannot be resumed', async ({ page, studioOrigin }) => {
  const timestamp = Date.now();
  const record = {
    id: 'job_stop', operation: 'image.generate', state: 'running', progress: 5,
    stage: 'shape_hr', created_at: String(timestamp - 3000), started_at: String(timestamp - 2000), updated_at: String(timestamp),
  };
  let requests = 0;
  await page.route('**/api/v1/jobs?limit=20&include_active=true', route => route.fulfill({ json: [record] }));
  await page.route('**/api/v1/jobs/job_stop/cancel', async route => {
    expect(route.request().method()).toBe('POST');
    requests += 1;
    record.state = 'cancelling';
    record.updated_at = String(timestamp + 1);
    await route.fulfill({ json: record });
  });
  await page.goto(studioOrigin + '/jobs');
  const active = page.getByRole('region', { name: /In progress/ });
  const stop = active.getByRole('button', { name: 'Stop job' });
  await stop.click();
  const confirm = page.getByRole('alertdialog', { name: 'Stop this job?' });
  await expect(confirm).toContainText('cannot be resumed');
  await confirm.getByRole('button', { name: 'Keep job' }).click();
  expect(requests).toBe(0);
  await stop.click();
  await confirm.getByRole('button', { name: 'Stop job' }).click();
  await expect(active.getByText('Stopping generation worker')).toBeVisible();
  await expect(stop).toHaveCount(0);
  record.state = 'cancelled';
  record.updated_at = String(timestamp + 2);
  await expect(page.getByRole('region', { name: 'History' }).getByText('Cancelled', { exact: true }).first()).toBeVisible();
  await expect(active.getByText('Nothing in progress')).toBeVisible();
  await expect(page.getByRole('button', { name: /resume/i })).toHaveCount(0);
  expect(requests).toBe(1);
});
