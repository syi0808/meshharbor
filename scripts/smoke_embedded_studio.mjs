import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';

const binaryArgument = process.argv[2];
assert.ok(binaryArgument, 'pass the built meshharbor binary path');
const binaryPath = path.resolve(binaryArgument);
const dataDirectory = await mkdtemp(path.join(tmpdir(), 'meshharbor-embedded-studio-'));

function availablePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      assert.ok(address && typeof address === 'object');
      server.close((error) => error ? reject(error) : resolve(address.port));
    });
  });
}

const pause = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function stopChild(child) {
  if (child.pid === undefined || child.exitCode !== null || child.signalCode !== null) {
    return;
  }

  const exited = new Promise((resolve) => child.once('exit', resolve));
  child.kill();
  const stopped = await Promise.race([exited.then(() => true), pause(5_000).then(() => false)]);
  if (!stopped) {
    const forceExit = new Promise((resolve) => child.once('exit', resolve));
    child.kill('SIGKILL');
    await Promise.race([forceExit, pause(5_000)]);
  }
}

let child;
try {
  const port = await availablePort();
  const origin = `http://127.0.0.1:${port}`;
  let spawnError;
  child = spawn(
    binaryPath,
    ['--data-dir', dataDirectory, 'serve', '--listen', `127.0.0.1:${port}`],
    { stdio: 'ignore' },
  );
  child.once('error', (error) => { spawnError = error; });

  let healthResponse;
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if (spawnError) {
      throw new Error(`MeshHarbor could not start: ${spawnError.message}`);
    }
    if (child.exitCode !== null) {
      throw new Error(`MeshHarbor exited before becoming ready (code ${child.exitCode}).`);
    }
    try {
      healthResponse = await fetch(`${origin}/api/v1/health`, {
        signal: AbortSignal.timeout(1_000),
      });
      if (healthResponse.ok) {
        break;
      }
    } catch {
      // The process may still be starting; retry until the bounded deadline.
    }
    await pause(250);
  }
  assert.ok(healthResponse?.ok, 'MeshHarbor health endpoint did not become ready');

  const health = await healthResponse.json();
  assert.equal(health.status, 'ok');
  assert.equal(health.service, 'meshharbor');

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
} finally {
  if (child) {
    await stopChild(child);
  }
  await rm(dataDirectory, { recursive: true, force: true });
}
