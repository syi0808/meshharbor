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
  await expect(page.getByRole('button', { name: 'Jobs', exact: true })).toHaveCount(0);
  await inventory.click();
  await expect(inventory).toHaveAttribute('aria-expanded', 'true');
});

test('Inventory views stay inside the resizable right dock', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  const library = page.getByRole('complementary', { name: 'Inventory' });
  const views = library.getByRole('group', { name: 'Inventory views' });
  const jobs = page.getByRole('button', { name: 'Jobs', exact: true });
  await expect(views.getByRole('button', { name: 'Assets', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await views.getByRole('button', { name: 'Jobs', exact: true }).click();
  await expect(jobs).toHaveAttribute('aria-pressed', 'true');
  await expect(library.getByText('No saved jobs yet.')).toBeVisible();
  const bounds = await library.boundingBox();
  expect(Math.round(bounds.x + bounds.width)).toBe(1280);
  const panelWidth = () => library.evaluate(element => Math.round(element.getBoundingClientRect().width));
  const initialWidth = await panelWidth();
  const handle = page.getByRole('separator', { name: 'Resize library panel' });
  await handle.press('ArrowLeft');
  await expect.poll(panelWidth).toBeGreaterThan(initialWidth);
  const resizedWidth = await panelWidth();
  await page.reload();
  await expect.poll(panelWidth).toBe(resizedWidth);
  await expect(jobs).toHaveAttribute('aria-pressed', 'true');
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

test('buttons, native selects and dialogs keep the official Mira dimensions', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');
  await expect(page.getByRole('banner').getByText('meshharbor', { exact: true })).toBeVisible();
  await expect(page.getByRole('banner')).not.toContainText(/Studio|Local engine ready/);
  await expect(page.getByRole('button', { name: 'Open GLB' })).toHaveCSS('height', '28px');
  const runtime = page.getByRole('button', { name: 'Engine and runtimes' });
  await expect(runtime).toHaveCSS('height', '24px');
  await expect(runtime).toHaveCSS('column-gap', '4px');
  await runtime.click();
  const dialog = page.getByRole('dialog', { name: 'Engine and runtimes' });
  await expect(dialog).toHaveCSS('display', 'grid');
  await expect(dialog).toHaveCSS('padding', '16px');
  await expect(dialog.getByLabel('Backend')).toHaveCSS('height', '28px');
  expect(await page.locator('html').evaluate(element => getComputedStyle(element).getPropertyValue('--primary').trim())).toBe('#82d7b8');
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
