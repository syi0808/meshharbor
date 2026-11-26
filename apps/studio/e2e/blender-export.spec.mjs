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
  await page.route('**/api/v1/blender/install', route => route.fulfill({ json: { version: '5.1.2' } }));
  await dialog.getByRole('button', { name: 'Install locally' }).click();
  await expect(dialog.getByText('Add-on installed for Blender 5.1.2.')).toBeVisible();
  const downloadReady = page.waitForEvent('download');
  await dialog.getByRole('button', { name: 'Download add-on' }).click();
  const download = await downloadReady;
  expect(download.suggestedFilename()).toBe('meshharbor_bridge.py');
  expect(await readFile(await download.path(), 'utf8')).toContain(studioOrigin);
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
