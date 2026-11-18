import { defineConfig } from '@playwright/test';
import base from './playwright.config.mjs';

export default defineConfig(base, {
  testIgnore: [],
  testMatch: '**/quadriflow-release.spec.mjs',
  timeout: 600_000,
  retries: 0,
});
