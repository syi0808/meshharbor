import { test as base } from '@playwright/test';
import { fileURLToPath } from 'node:url';
import { withEmbeddedStudio } from '../../../scripts/embedded-studio-test-server.mjs';

const executable = process.platform === 'win32' ? 'meshharbor.exe' : 'meshharbor';
const binary = process.env.MESHHARBOR_BINARY ?? fileURLToPath(new URL(`../../../target/debug/${executable}`, import.meta.url));

export const test = base.extend({
  studioOrigin: async ({}, use) => withEmbeddedStudio(binary, use),
});
