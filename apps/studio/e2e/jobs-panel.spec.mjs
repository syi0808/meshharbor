import { expect } from '@playwright/test';
import { test } from './studio-fixture.mjs';

test('running and failed jobs put their status and actions below full-width details', async ({ page, studioOrigin }) => {
  const timestamp = String(Date.now());
  const job = {
    operation: 'model.generate', input_asset_id: 'reference',
    input_asset_name: 'Screenshot of a model reference with a long filename.png',
    created_at: timestamp, started_at: timestamp, updated_at: timestamp, stage: 'shape_hr',
  };
  await page.route('**/api/v1/jobs?limit=20&include_active=true', route => route.fulfill({ json: [
    { ...job, id: 'running-layout', state: 'running', progress: 5 },
    { ...job, id: 'failed-layout', state: 'failed', progress: 100 },
  ] }));
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(studioOrigin + '/jobs');
  const running = page.getByRole('region', { name: /In progress/ }).getByRole('listitem');
  const failed = page.getByRole('region', { name: 'History' }).getByRole('listitem');
  const assertLayout = async (card, status, lastAction, expectSameRow) => {
    await expect(card).toBeVisible();
    const [content, badge, actions, detailsButton, lastButton] = await Promise.all([
      card.locator('[data-slot="item-content"]').boundingBox(),
      card.getByText(status, { exact: true }).boundingBox(),
      card.locator('[data-slot="item-actions"]').boundingBox(),
      card.getByRole('button', { name: 'View details for Generate model' }).boundingBox(),
      card.getByRole('button', { name: lastAction }).boundingBox(),
    ]);
    expect(badge.y).toBeGreaterThanOrEqual(content.y + content.height);
    expect(actions.y).toBeGreaterThanOrEqual(content.y + content.height);
    expect(badge.x).toBeCloseTo(content.x, 0);
    expect(actions.x + actions.width).toBeCloseTo(content.x + content.width, 0);
    expect(lastButton.x + lastButton.width).toBeCloseTo(content.x + content.width, 0);
    expect(badge.x + badge.width).toBeLessThan(detailsButton.x);
    if (expectSameRow) {
      expect(lastButton.y).toBeLessThan(badge.y + badge.height);
    }
  };
  await assertLayout(running, 'Running', 'Stop job', true);
  await assertLayout(failed, 'Failed', 'View input', false);
  await page.setViewportSize({ width: 1280, height: 844 });
  const dock = page.getByRole('complementary', { name: 'Jobs' });
  await expect(dock).toBeVisible();
  const divider = await page.getByRole('separator', { name: 'Resize Jobs panel' }).boundingBox();
  await page.mouse.move(divider.x + divider.width / 2, divider.y + divider.height / 2);
  await page.mouse.down();
  await page.mouse.move(divider.x - 140, divider.y + divider.height / 2);
  await page.mouse.up();
  await expect.poll(async () => (await dock.boundingBox()).width).toBeGreaterThan(400);
  await assertLayout(running, 'Running', 'Stop job', true);
  await assertLayout(failed, 'Failed', 'View input', true);
});

test('the Jobs panel polls engine activity, keeps stable completed durations, and loads older records', async ({ page, studioOrigin }) => {
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
  await history.getByRole('button', { name: 'View details for Download MLX models' }).click();
  const details = page.getByRole('dialog', { name: 'Job details' });
  await expect(history.getByText('Local cache is unavailable.')).toHaveCount(0);
  await expect(details.getByText('Local cache is unavailable.')).toBeVisible();
  await expect(page.getByRole('region', { name: /Notifications/ }).getByText('Local cache is unavailable.')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(details).toHaveCount(0);

  jobs = [external, ...lastFinished];
  await expect(active.getByText('Refining geometry')).toBeVisible();
  const elapsed = active.getByText(/^Elapsed: /);
  const firstTick = await elapsed.textContent();
  await expect(elapsed).not.toHaveText(firstTick);
  await expect(active.getByText('Queued for: 10s')).toBeVisible();
  await expect(active.getByText(external.input_asset_name)).toBeVisible();
  await expect(history.getByText('Elapsed: 25s')).toHaveCount(21);
  await page.getByRole('button', { name: 'Inventory', exact: true }).click();
  const inventory = page.getByRole('complementary', { name: 'Inventory' });
  await expect(inventory.getByRole('region', { name: 'Models in progress' }).getByText('Refining geometry')).toBeVisible();
  jobs = [{
    ...external, state: 'succeeded', progress: 100, updated_at: String(now + 2000), result: { asset_id: asset.id },
  }, ...lastFinished.slice(0, -1)];
  await expect(inventory.getByTitle(asset.name)).toBeVisible();
  await expect(inventory.getByRole('region', { name: 'Models in progress' })).toHaveCount(0);
  await expect(page.getByRole('region', { name: 'Start a model' })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('button', { name: 'Jobs', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Jobs' })).toBeVisible();
  await expect(history.getByText(external.input_asset_name)).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(390);
});

test('a generation moves from Stopping to Stopped and can start a new attempt', async ({ page, studioOrigin }) => {
  const timestamp = Date.now();
  const record = {
    id: 'job_stop', operation: 'model.generate', state: 'running', progress: 5,
    stage: 'shape_hr', input_asset_id: 'reference', created_at: String(timestamp - 3000), started_at: String(timestamp - 2000), updated_at: String(timestamp),
  };
  let requests = 0;
  let retried;
  await page.route('**/api/v1/jobs?limit=20&include_active=true', route => route.fulfill({ json: retried ? [record, retried] : [record] }));
  await page.route('**/api/v1/jobs/job_stop/cancel', async route => {
    expect(route.request().method()).toBe('POST');
    requests += 1;
    record.state = 'cancelling';
    record.updated_at = String(timestamp + 1);
    await route.fulfill({ json: record });
  });
  await page.route('**/api/v1/jobs/job_stop/retry', route => {
    expect(route.request().method()).toBe('POST');
    expect(route.request().postDataJSON()).toEqual({ confirm_download: false });
    expect(record.state).toBe('cancelled');
    retried = { ...record, id: 'new_attempt', state: 'queued', updated_at: String(timestamp + 3) };
    return route.fulfill({ status: 202, json: retried });
  });
  await page.goto(studioOrigin + '/jobs');
  const active = page.getByRole('region', { name: /In progress/ });
  const stop = active.getByRole('button', { name: 'Stop job' });
  await stop.click();
  const confirm = page.getByRole('alertdialog', { name: 'Stop this job?' });
  await confirm.getByRole('button', { name: 'Keep job' }).click();
  expect(requests).toBe(0);
  await stop.click();
  await confirm.getByRole('button', { name: 'Stop job' }).click();
  await expect(active.getByText('Stopping generation worker')).toBeVisible();
  await expect(stop).toHaveCount(0);
  record.state = 'cancelled';
  record.updated_at = String(timestamp + 2);
  const history = page.getByRole('region', { name: 'History' });
  await expect(history.getByText('Stopped', { exact: true }).first()).toBeVisible();
  await expect(active.getByText('Nothing in progress')).toBeVisible();
  expect(requests).toBe(1);
  await history.getByRole('button', { name: 'Retry job' }).click();
  const retry = page.getByRole('alertdialog', { name: 'Retry this job?' });
  await expect(retry).toContainText('keeps the previous record');
  await retry.getByRole('button', { name: 'Retry job' }).click();
  await expect(active.getByText('Queued', { exact: true })).toBeVisible();
  await expect(history.getByText('Stopped', { exact: true }).first()).toBeVisible();
});

test('job details expand saved commands and update logs without closing the step', async ({ page, studioOrigin }) => {
  const timestamp = String(Date.now());
  const job = {
    id: 'job_setup_logs', operation: 'runtime.pixal3d.setup', state: 'running', progress: 5,
    stage: 'sync_dependencies', created_at: timestamp, started_at: timestamp, updated_at: timestamp,
  };
  const command = {
    id: 1, argv: ['uv', 'sync', '--locked'], cwd: '/local/runtime', started_at: timestamp,
    ended_at: null, outcome: null, stdout: 'Starting sync', stderr: '', truncated: false,
  };
  const steps = [
    { id: 1, state: 'running', stage: null, at: timestamp, message: null, commands: [] },
    { id: 2, state: 'running', stage: 'prepare_python', at: timestamp, message: 'Reusing the existing private Python 3.12 environment.', commands: [] },
    { id: 3, state: 'running', stage: 'sync_dependencies', at: timestamp, message: null, commands: [command] },
  ];
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route('**/api/v1/jobs?limit=20&include_active=true', route => route.fulfill({ json: [job] }));
  await page.route('**/api/v1/jobs/job_setup_logs/steps', route => route.fulfill({ json: steps }));
  await page.goto(studioOrigin + '/jobs');
  const openDetails = () => page.getByRole('button', { name: 'View details for Set up MLX' }).click();
  await openDetails();
  const dialog = page.getByRole('dialog', { name: 'Job details' });
  const step = text => dialog.locator('summary').filter({ hasText: text });
  await expect(dialog.getByText('uv sync --locked')).toBeHidden();
  await step('Checking existing Python environment').click();
  await expect(dialog.getByText('Reusing the existing private Python 3.12 environment.')).toBeVisible();
  await step('Synchronizing dependencies').click();
  await expect(dialog.getByText('uv sync --locked')).toBeVisible();
  await expect(dialog.getByText('Starting sync')).toBeVisible();

  command.stderr = 'fixture dependency failure';
  await expect(dialog.getByText(command.stderr)).toBeVisible();
  command.outcome = 'exit status: 17';
  command.ended_at = timestamp;
  job.state = 'failed';
  job.updated_at = String(Number(timestamp) + 1);
  job.error = { message: 'Could not sync locked dependencies.' };
  steps.push({ id: 4, state: 'failed', stage: null, at: job.updated_at, message: null, commands: [] });
  await expect(dialog.getByText('exit status: 17', { exact: false })).toBeVisible();
  await expect(dialog.getByText(job.error.message)).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(390);

  await page.reload();
  await openDetails();
  await step('Synchronizing dependencies').click();
  await expect(dialog.getByText(command.stderr)).toBeVisible();
});
