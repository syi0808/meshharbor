import { expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { PNG } from 'pngjs';
import { quadriflowCube } from './quadriflow-fixture.mjs';
import { test } from './studio-fixture.mjs';

async function openCube(page, name, bytes) {
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.getByRole('button', { name: 'Open GLB', exact: true }).click(),
  ]);
  await chooser.setFiles({ name, mimeType: 'model/gltf-binary', buffer: bytes });
  const selected = page.getByRole('region', { name: 'Selected asset' });
  await expect(selected.getByTitle(name, { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Retopology', exact: true }).click();
  return selected;
}

async function download(page, button, filename) {
  const [file] = await Promise.all([
    page.waitForEvent('download'),
    page.getByRole('button', { name: button, exact: true }).click(),
  ]);
  expect(file.suggestedFilename()).toBe(filename);
  return readFile(await file.path());
}

function assertQuadObj(bytes) {
  let vertices = 0;
  const faces = [];
  for (const line of new TextDecoder('utf-8', { fatal: true }).decode(bytes).split(/\r?\n/)) {
    if (!line.trim() || line.trimStart().startsWith('#')) continue;
    const [type, ...fields] = line.trim().split(/\s+/);
    if (type === 'v') {
      expect(fields).toHaveLength(3);
      expect(fields.every(value => Number.isFinite(Number(value)))).toBe(true);
      vertices++;
    } else {
      expect(type).toBe('f');
      expect(fields).toHaveLength(4);
      expect(fields.every(field => /^[1-9]\d*$/.test(field))).toBe(true);
      expect(new Set(fields).size).toBe(4);
      faces.push(fields.map(Number));
    }
  }
  expect(vertices).toBeGreaterThan(0);
  expect(faces.length).toBeGreaterThan(0);
  expect(faces.every(face => face.every(vertex => vertex <= vertices))).toBe(true);
  return faces.length;
}

function assertPreview(bytes, quadCount, textured) {
  expect(bytes.toString('ascii', 0, 4)).toBe('glTF');
  expect(bytes.readUInt32LE(4)).toBe(2);
  expect(bytes.readUInt32LE(8)).toBe(bytes.length);
  expect(bytes.toString('ascii', 16, 20)).toBe('JSON');
  const jsonEnd = 20 + bytes.readUInt32LE(12);
  const gltf = JSON.parse(bytes.toString('utf8', 20, jsonEnd));
  expect(bytes.toString('ascii', jsonEnd + 4, jsonEnd + 8)).toBe('BIN\0');
  expect(jsonEnd + 8 + bytes.readUInt32LE(jsonEnd)).toBe(bytes.length);
  const [mesh] = gltf.meshes;
  const [primitive] = mesh.primitives;
  expect(mesh.primitives).toHaveLength(1);
  expect(primitive.mode ?? 4).toBe(4);
  expect(gltf.accessors[primitive.indices].count).toBe(quadCount * 6);
  if (!textured) {
    expect(gltf.images ?? []).toHaveLength(0);
    return;
  }
  expect(primitive.attributes.TEXCOORD_0).toEqual(expect.any(Number));
  const material = gltf.materials[primitive.material].pbrMetallicRoughness;
  expect(gltf.images).toHaveLength(2);
  for (const role of ['baseColorTexture', 'metallicRoughnessTexture']) {
    const image = gltf.images[gltf.textures[material[role].index].source];
    expect(image.mimeType).toBe('image/png');
    const view = gltf.bufferViews[image.bufferView];
    const start = jsonEnd + 8 + (view.byteOffset ?? 0);
    const png = PNG.sync.read(bytes.subarray(start, start + view.byteLength));
    expect(Math.min(png.width, png.height)).toBeGreaterThan(2);
  }
}

test('reviewed QuadriFlow release installs, retopologizes and exports GLB and OBJ', async ({ page, studioOrigin }, testInfo) => {
  const releaseTag = process.env.MESHHARBOR_QUADRIFLOW_RELEASE_TAG;
  expect(releaseTag, 'Set the expected, reviewed GitHub Release tag before running this acceptance test')
    .toMatch(/^quadriflow-v[0-9][A-Za-z0-9._-]*$/);
  const [runtime] = await Promise.all([
    page.waitForResponse(response => response.url() === `${studioOrigin}/api/v1/runtime` && response.ok()),
    page.goto(studioOrigin),
  ]);
  const { quadriflow_backend: backend } = await runtime.json();
  expect(backend, 'A clean store must have an approved signed release for this host; do not substitute a candidate ZIP')
    .toMatchObject({ status: 'install_available', install_available: true, available: false });
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');

  for (const textured of [false, true]) {
    const sourceStem = textured ? 'signed-cube-pbr' : 'signed-cube-geometry';
    const source = quadriflowCube({ textured });
    const selected = await openCube(page, `${sourceStem}.glb`, source);
    if (!textured) {
      await page.getByText('Retopology availability', { exact: true }).click();
      const install = page.getByRole('button', { name: 'Install QuadriFlow', exact: true });
      await expect(install, 'Use a binary with an approved, verified release bundled for this host; no candidate ZIPs or manifest overrides')
        .toBeEnabled();
      const [response] = await Promise.all([
        page.waitForResponse(response => response.url() === `${studioOrigin}/api/v1/runtime/quadriflow/install`, { timeout: 150_000 }),
        install.click(),
      ]);
      expect(response.ok(), await response.text()).toBe(true);
      expect(await response.json()).toMatchObject({ release_tag: releaseTag, already_installed: false });
      const availability = page.getByRole('complementary', { name: 'Quad retopology (experimental)' });
      await availability.getByRole('button', { name: 'About the QuadriFlow backend' }).hover();
      await expect(page.getByRole('tooltip')).toContainText('A verified, manifest-matched managed QuadriFlow sidecar is available.');
    }

    if (textured) {
      await page.getByRole('checkbox', { name: 'Set target quads' }).check();
      await page.getByRole('spinbutton', { name: 'Target quads' }).fill('384');
    }
    await expect(page.getByText(textured ? 'Textured input passed' : 'Geometry-only input passed', { exact: false })).toBeVisible();
    const [completed] = await Promise.all([
      page.waitForResponse(async response => {
        if (!response.ok() || !response.url().startsWith(`${studioOrigin}/api/v1/jobs/job_`)
          || response.request().method() !== 'GET') return false;
        const job = await response.json();
        return job.operation === 'mesh.retopology' && ['succeeded', 'failed'].includes(job.state);
      }, { timeout: 200_000 }),
      page.getByRole('button', { name: 'Run Quad retopology (experimental)', exact: true }).click(),
    ]);
    const job = await completed.json();
    expect(job.state, job.error?.message).toBe('succeeded');
    expect(job.result.report.requested_quads).toBe(textured ? 384 : null);
    if (textured) {
      expect(job.result.report.appearance_projection.projection_coverage).toBeGreaterThanOrEqual(0.995);
    }
    const resultStem = `${sourceStem}-quad-retopologized`;
    await expect(selected.getByTitle(`${resultStem}.glb`, { exact: true })).toBeVisible();
    await expect(selected.locator('canvas')).toBeVisible();
    const glbName = `${resultStem}.glb`;
    const objName = `${resultStem}-quads.obj`;
    const glb = await download(page, 'Export selected GLB', glbName);
    const obj = await download(page, 'Quad OBJ', objName);
    const quadCount = assertQuadObj(obj);
    expect(job.result.report.output_quads).toBe(quadCount);
    assertPreview(glb, quadCount, textured);
    await testInfo.attach(glbName, { body: glb, contentType: 'model/gltf-binary' });
    await testInfo.attach(objName, { body: obj, contentType: 'model/obj' });
    await selected.getByRole('tab', { name: 'Original' }).click();
    await expect(selected.getByTitle(`${sourceStem}.glb`, { exact: true })).toBeVisible();
    expect(await download(page, 'Export selected GLB', `${sourceStem}.glb`)).toEqual(source);
  }
});
