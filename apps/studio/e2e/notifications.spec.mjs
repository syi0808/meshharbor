import { expect } from '@playwright/test';
import { test } from './studio-fixture.mjs';

test('the header shares dismissed toast history across Studio pages', async ({ page, studioOrigin }) => {
  await page.goto(studioOrigin);
  await expect(page.locator('main')).toHaveAttribute('data-connection', 'connected');

  const header = page.getByRole('banner');
  const bell = header.getByRole('button', { name: 'Notifications', exact: true });
  const history = page.getByRole('dialog', { name: 'Notification history' });
  const rows = history.getByRole('listitem');
  const toasts = page.getByRole('region', { name: /Notifications/ });
  async function dismissToast(message) {
    await expect(toasts.getByText(message, { exact: true })).toBeVisible();
    await toasts.getByRole('button', { name: 'Dismiss notification' }).click();
    await expect(toasts.getByText(message, { exact: true })).toHaveCount(0);
  }

  await bell.press('Enter');
  await expect(history.getByText('No notifications yet.')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(history).toHaveCount(0);
  await expect(bell).toBeFocused();

  await page.getByRole('button', { name: 'Refresh assets' }).click();
  await dismissToast('Asset library refreshed.');
  await header.getByRole('button', { name: 'Settings', exact: true }).click();
  await bell.click();
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText('Asset library refreshed.');
  await history.getByRole('button', { name: 'Close notification history' }).click();

  await page.getByRole('button', { name: 'Refresh status', exact: true }).click();
  await dismissToast('Local status refreshed.');
  await header.getByRole('button', { name: 'Dashboard', exact: true }).click();
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await dismissToast('Job history refreshed.');
  await page.setViewportSize({ width: 390, height: 844 });
  await bell.click();

  await expect(rows).toHaveCount(3);
  await expect(rows.nth(0)).toContainText('Job history refreshed.');
  await expect(rows.nth(1)).toContainText('Local status refreshed.');
  await expect(rows.nth(2)).toContainText('Asset library refreshed.');
  await expect(rows.first().locator('time')).toHaveAttribute('datetime', /^\d{4}-\d{2}-\d{2}T/);
  const bounds = await history.boundingBox();
  expect(bounds.x).toBeGreaterThanOrEqual(0);
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(390);
});
