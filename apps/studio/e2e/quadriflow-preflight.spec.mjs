import { expect } from '@playwright/test';
import { quadriflowCube } from './quadriflow-fixture.mjs';
import { test } from './studio-fixture.mjs';

test('Studio preflights a closed GLB using the selected quad target without queuing a job', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.getByRole('button', { name: 'Open GLB', exact: true }).click(),
  ]);
  await chooser.setFiles({ name: 'quad-source.glb', mimeType: 'model/gltf-binary', buffer: quadriflowCube() });
  await page.getByRole('button', { name: 'More tools' }).click();
  await page.getByRole('button', { name: 'Quad retopology (experimental)', exact: true }).click();
  const target = page.getByRole('spinbutton', { name: 'Target quads' });
  const check = page.getByRole('button', { name: 'Check retopology input' });
  await target.fill('3');
  await expect(check).toBeDisabled();
  await target.fill('12');
  const [response] = await Promise.all([
    page.waitForResponse(response => response.url().endsWith('/retopology-preflight')),
    check.click(),
  ]);
  expect(response.ok()).toBe(true);
  expect(await response.json()).toMatchObject({ requested_quads: 12, input_triangles: 12 });
  await expect(page.getByText('Geometry-only input passed', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Jobs', exact: true }).click();
  await expect(page.getByText('No saved jobs yet.')).toBeVisible();
});
