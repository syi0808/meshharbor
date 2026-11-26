import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  testIgnore: '**/quadriflow-release.spec.mjs',
  timeout: 90_000,
  forbidOnly: !!process.env.CI,
  workers: process.env.CI ? 1 : undefined,
  expect: { timeout: process.env.CI ? 15_000 : 5_000 },
  use: {
    browserName: 'chromium',
    viewport: { width: 1280, height: 800 },
    launchOptions: { args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader'] },
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
});
