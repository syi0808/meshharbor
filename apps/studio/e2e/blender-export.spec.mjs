import { randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { expect } from '@playwright/test';
import { test } from './studio-fixture.mjs';
import { fixtureTriangles, inspectTexturedGlb, texturedGlb } from './textured-glb.mjs';

test('Studio downloads the Blender add-on and sends the selected compressed version', async ({ page, studioOrigin }) => {
  const source = texturedGlb();
  await page.goto(studioOrigin);
  await page.getByLabel('Choose GLB files').setInputFiles({
    name: 'panels.glb', mimeType: 'model/gltf-binary', buffer: source,
  });
  const preview = page.getByRole('region', { name: 'Selected asset' });
  const send = preview.getByRole('button', { name: 'Send to Blender' });
  await expect(send.locator('img')).toHaveJSProperty('naturalWidth', 32);
  await send.click();
  const dialog = page.getByRole('dialog', { name: 'Send to Blender' });
  const instructions = dialog.locator('ol');
  const setupStatus = dialog.locator('ol + [role="status"]');
  await expect(instructions).toContainText('Install locally (recommended)');
  await expect(instructions).toContainText('Download add-on (manual)');
  await expect(instructions).toContainText('Install from Disk');
  await expect(setupStatus).toContainText('Waiting for Blender to connect.');
  const dots = setupStatus.locator('[aria-hidden="true"]');
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await expect(dots).toHaveCSS('animation-name', 'blender-waiting-dots');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await expect(dots).toHaveCSS('animation-name', 'none');

  const install = dialog.getByRole('button', { name: 'Install locally' });
  const blenderRunning = 'Blender is running. Quit Blender, then click Install locally again.';
  await page.route('**/api/v1/blender/install', route => route.fulfill({
    status: 409, json: { error: blenderRunning },
  }), { times: 1 });
  await install.click();
  await expect(dialog.getByRole('alert')).toHaveText(blenderRunning);
  await expect(install).toBeEnabled();

  let finishInstall;
  const installInProgress = new Promise(resolve => { finishInstall = resolve; });
  await page.route('**/api/v1/blender/install', async route => {
    await installInProgress;
    await route.fulfill({ json: { version: '5.1.2' } });
  });
  try {
    await install.click();
    await expect(dialog.getByRole('button', { name: 'Installing…' })).toBeDisabled();
    await expect(setupStatus).toContainText('Checking Blender and preparing the add-on…');
  } finally {
    finishInstall();
  }
  await expect(setupStatus).toContainText('Add-on installed and enabled for Blender 5.1.2. Open Blender to start the bridge.');
  const downloadReady = page.waitForEvent('download');
  await dialog.getByRole('button', { name: 'Download add-on' }).click();
  const download = await downloadReady;
  expect(download.suggestedFilename()).toBe('meshharbor_bridge.py');
  expect(await readFile(await download.path(), 'utf8')).toContain(studioOrigin);
  await expect(setupStatus).toContainText('Download started. Install the downloaded file in Blender to continue.');
  await dialog.getByRole('button', { name: 'Close' }).click();

  await page.getByRole('button', { name: 'Compression', exact: true }).click();
  await page.getByRole('button', { name: 'Run Compression' }).click();
  await expect(preview.getByTitle('panels-compressed.glb')).toBeVisible();

  const request = (path, body) => fetch(`${studioOrigin}/api/v1/blender/client/${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-meshharbor-blender': 'local-addon-v1' },
    body: JSON.stringify(body),
  });
  const client_id = randomUUID();
  expect((await request('poll', { client_id })).status).toBe(204);
  const sending = page.waitForResponse(response => response.request().method() === 'POST' && response.url().endsWith('/api/v1/blender/send'));
  await send.click();
  const sent = await sending;
  expect(sent.status()).toBe(202);
  const requestBody = sent.request().postDataJSON();
  const selectedAsset = await page.evaluate(async assetId => {
    const response = await fetch(`/api/v1/assets/${assetId}`, { headers: { 'x-meshharbor-dashboard': 'local-ui-v1' } });
    return response.json();
  }, requestBody.asset_id);
  expect(selectedAsset.name).toBe('panels-compressed.glb');
  const result = await request('poll', { client_id });
  expect(result.status).toBe(200);
  const decoded = inspectTexturedGlb(Buffer.from(await result.arrayBuffer()));
  expect(decoded.triangles).toBe(fixtureTriangles);
  expect(decoded.appearance).toEqual(inspectTexturedGlb(source).appearance);
  const transferId = result.headers.get('x-meshharbor-transfer');
  expect((await request(`result/${transferId}`, { client_id: randomUUID() })).status).toBe(404);
  expect((await request(`result/${transferId}`, { client_id })).status).toBe(204);
  await expect(page.getByText('Model added to Blender.')).toBeVisible();
});
