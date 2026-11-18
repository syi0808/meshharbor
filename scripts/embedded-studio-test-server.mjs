import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { setTimeout as pause } from 'node:timers/promises';

function availablePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close((error) => error ? reject(error) : resolve(port));
    });
  });
}

async function stopChild(child) {
  if (child?.pid === undefined || child.exitCode !== null || child.signalCode !== null) return;

  const exited = new Promise((resolve) => child.once('exit', () => resolve(true)));
  const waitForExit = () => Promise.race([exited, pause(5_000, false, { ref: false })]);
  child.kill();
  if (!(await waitForExit())) {
    child.kill('SIGKILL');
    assert.ok(await waitForExit(), 'MeshHarbor test server did not exit');
  }
}

export async function withEmbeddedStudio(binaryPath, runTest) {
  const dataDirectory = await mkdtemp(path.join(tmpdir(), 'meshharbor-embedded-studio-'));
  let child;
  try {
    const port = await availablePort();
    const origin = `http://127.0.0.1:${port}`;
    let spawnError;
    child = spawn(
      path.resolve(binaryPath),
      ['--data-dir', dataDirectory, 'serve', '--listen', `127.0.0.1:${port}`],
      { stdio: 'ignore', env: { ...process.env, MESHHARBOR_API_TOKEN: randomUUID() } },
    );
    child.once('error', (error) => { spawnError = error; });

    const deadline = Date.now() + 60_000;
    while (Date.now() < deadline) {
      if (spawnError) throw new Error(`MeshHarbor could not start: ${spawnError.message}`);
      if (child.exitCode !== null || child.signalCode !== null) {
        throw new Error(`MeshHarbor exited before becoming ready (${child.exitCode ?? child.signalCode}).`);
      }
      const healthResponse = await fetch(`${origin}/api/v1/health`, {
        signal: AbortSignal.timeout(1_000),
      }).catch(() => null);
      if (healthResponse?.ok) {
        const health = await healthResponse.json();
        assert.equal(health.status, 'ok');
        assert.equal(health.service, 'meshharbor');
        return await runTest(origin);
      }
      await pause(250);
    }
    throw new Error('MeshHarbor health endpoint did not become ready');
  } finally {
    await stopChild(child);
    await rm(dataDirectory, { recursive: true, force: true });
  }
}
