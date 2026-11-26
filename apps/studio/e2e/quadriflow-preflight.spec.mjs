import { expect } from '@playwright/test';
import { quadriflowCube } from './quadriflow-fixture.mjs';
import { test } from './studio-fixture.mjs';

const preflightPath = '**/retopology-preflight';
const isPreflight = request => request.url().endsWith('/retopology-preflight');

async function importCube(page, name, textured = false) {
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.getByRole('button', { name: 'Open GLB', exact: true }).click(),
  ]);
  await chooser.setFiles({ name, mimeType: 'model/gltf-binary', buffer: quadriflowCube({ textured }) });
  await expect(page.getByRole('region', { name: 'Selected asset' }).getByTitle(name, { exact: true })).toBeVisible();
}

test('Studio automatically preflights the open retopology tool without queuing a job', async ({ page, studioOrigin }) => {
  const requests = [];
  page.on('request', request => { if (isPreflight(request)) requests.push(request.postDataJSON()); });
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  await importCube(page, 'quad-source.glb');
  const tool = page.getByRole('button', { name: 'Retopology', exact: true });
  const tools = page.getByRole('complementary', { name: 'Quad retopology (experimental)' });
  expect(requests).toEqual([]);
  const [automatic] = await Promise.all([page.waitForResponse(preflightPath), tool.click()]);
  expect(automatic.ok()).toBe(true);
  expect(await automatic.json()).toMatchObject({ requested_quads: null, input_triangles: 12 });
  await expect(tools).toContainText('the quad OBJ is geometry-only');
  await expect(tools.getByRole('status')).toHaveText('Geometry-only input passed');
  await expect(tools.getByText('This build does not include an approved QuadriFlow release artifact.')).toBeVisible();
  await expect(tools.getByRole('button', { name: 'Check retopology input' })).toHaveCount(0);
  const customTarget = tools.getByRole('checkbox', { name: 'Set target quads' });
  const target = tools.getByRole('spinbutton', { name: 'Target quads' });
  const run = page.getByRole('button', { name: 'Run Quad retopology (experimental)', exact: true });
  await expect(customTarget).not.toBeChecked();
  await expect(target).toHaveCount(0);
  await customTarget.check();
  await expect(tools.getByRole('status')).toHaveText('Geometry-only input passed');
  await target.fill('3');
  await expect(target).toHaveAttribute('aria-invalid', 'true');
  await expect(run).toBeDisabled();
  await expect(tools.getByText('Geometry-only input passed')).toHaveCount(0);
  const [response] = await Promise.all([
    page.waitForResponse(preflightPath),
    target.fill('12'),
  ]);
  expect(response.ok()).toBe(true);
  expect(await response.json()).toMatchObject({ requested_quads: 12, input_triangles: 12 });
  await expect(tools.getByRole('status')).toHaveText('Geometry-only input passed');
  const inputHelp = tools.getByRole('button', { name: 'About the input check' });
  await inputHelp.focus();
  await expect(page.getByRole('tooltip', { name: '12 triangles. The solver did not run.' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('tooltip')).toHaveCount(0);
  expect(requests).toEqual([{}, { target_quads: 10000 }, { target_quads: 12 }]);

  await importCube(page, 'textured-source.glb', true);
  await expect(tools.getByRole('status')).toHaveText('Textured input passed');
  await customTarget.uncheck();
  await expect(target).toHaveCount(0);
  await expect(tools.getByRole('status')).toHaveText('Textured input passed');
  expect(requests).toEqual([{}, { target_quads: 10000 }, { target_quads: 12 }, { target_quads: 12 }, {}]);
  await page.getByRole('button', { name: 'Jobs', exact: true }).click();
  await expect(page.getByText('No finished jobs yet.')).toBeVisible();
});

test('retopology failures use only a toast and superseded requests are cancelled', async ({ page, studioOrigin }) => {
  const detail = 'Retopology accepts either supported untextured geometry or one static embedded PNG PBR mesh';
  await page.route('**/api/v1/operations', route => route.fulfill({
    json: [{ id: 'mesh.retopology', implemented: true }],
  }));
  let releaseStale;
  const stale = new Promise(resolve => { releaseStale = resolve; });
  let interceptedAutomatic = false;
  await page.route(preflightPath, async route => {
    if (!interceptedAutomatic && route.request().postDataJSON().target_quads === undefined) {
      interceptedAutomatic = true;
      await stale;
      return route.fulfill({ status: 422, json: { error: 'Stale preflight response.' } });
    }
    if (route.request().postDataJSON().target_quads === 12) {
      return route.fulfill({ status: 422, json: { error: `operation unavailable: ${detail}` } });
    }
    return route.continue();
  });
  try {
    await page.goto(studioOrigin);
    await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
    await importCube(page, 'quad-source.glb');
    const tool = page.getByRole('button', { name: 'Retopology', exact: true });
    const tools = page.getByRole('complementary', { name: 'Quad retopology (experimental)' });
    const run = tools.getByRole('button', { name: 'Run Quad retopology (experimental)', exact: true });
    const customTarget = tools.getByRole('checkbox', { name: 'Set target quads' });
    const target = tools.getByRole('spinbutton', { name: 'Target quads' });
    await Promise.all([page.waitForRequest(preflightPath), tool.click()]);
    await expect(tools.getByRole('status')).toHaveText('Checking input…');
    await expect(run).toBeDisabled();
    await Promise.all([
      page.waitForEvent('requestfailed', request => isPreflight(request) && request.postDataJSON().target_quads === undefined),
      customTarget.check(),
    ]);
    await target.fill('12');
    releaseStale();
    const toasts = page.getByRole('region', { name: /Notifications/ });
    await expect(toasts.getByText('Retopology input check failed.')).toBeVisible();
    await expect(toasts.getByText(detail, { exact: true })).toBeVisible();
    await expect(toasts.getByText('Stale preflight response.')).toHaveCount(0);
    await expect(tools).not.toContainText(detail);
    await expect(run).toBeDisabled();
    await target.fill('24');
    await expect(tools.getByRole('status')).toHaveText('Geometry-only input passed');
    await expect(run).toBeEnabled();
    await customTarget.uncheck();
    await expect(run).toBeEnabled();
    const [submission] = await Promise.all([
      page.waitForRequest(request => request.url().endsWith('/api/v1/jobs') && request.method() === 'POST'),
      run.click(),
    ]);
    expect(submission.postDataJSON()).toMatchObject({ operation: 'mesh.retopology' });
    expect(submission.postDataJSON().parameters).toEqual({});
  } finally {
    releaseStale();
  }
});
