import assert from 'node:assert/strict';
import { withEmbeddedStudio } from './embedded-studio-test-server.mjs';

const binaryArgument = process.argv[2];
assert.ok(binaryArgument, 'pass the built meshharbor binary path');
await withEmbeddedStudio(binaryArgument, async (origin) => {
  const studioResponse = await fetch(origin);
  assert.equal(studioResponse.status, 200, 'embedded Studio root should load');
  assert.match(studioResponse.headers.get('content-type') ?? '', /^text\/html\b/i);
  const html = await studioResponse.text();
  assert.match(html, /<title>MeshHarbor Studio<\/title>/);
  assert.doesNotMatch(html, /assets are not built/i, 'binary must include the Studio build');

  const assetPaths = [...html.matchAll(/(?:src|href)=["']([^"']+)["']/g)]
    .map((match) => match[1])
    .filter((assetPath) => assetPath.startsWith('/assets/'));
  const scriptPaths = assetPaths.filter((assetPath) => assetPath.endsWith('.js'));
  assert.ok(scriptPaths.length > 0, 'Studio HTML must reference a bundled JavaScript asset');

  for (const assetPath of assetPaths) {
    const response = await fetch(new URL(assetPath, origin));
    assert.equal(response.status, 200, `embedded asset should load: ${assetPath}`);
    const expectedType = assetPath.endsWith('.js')
      ? /^text\/javascript\b/i
      : assetPath.endsWith('.css')
        ? /^text\/css\b/i
        : null;
    if (expectedType) {
      assert.match(response.headers.get('content-type') ?? '', expectedType);
    }
    assert.ok((await response.arrayBuffer()).byteLength > 0, `asset should be nonempty: ${assetPath}`);
  }

  console.log(`Embedded Studio startup smoke passed on ${process.platform} (${assetPaths.length} asset(s)).`);
});
