import { expect, test, type APIRequestContext } from '@playwright/test';
import { spawn, type ChildProcess } from 'child_process';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { openSettings, registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

// Settings › Workers: a token is shown once inside the command to run, a real
// `psf-guard worker` joins with it and shares the exchange folder, and
// revoking the token disconnects the worker, which stops with the reason.

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const BINARY = process.env.PSF_GUARD_E2E_BINARY
  ?? path.resolve(__dirname, '../../target/release/psf-guard');
const BASE = `http://127.0.0.1:${Number(process.env.PSF_GUARD_E2E_PORT ?? 13099)}`;

let worker: ChildProcess | null = null;

test.beforeEach(async ({ request }) => {
  // With no catalog the app opens Settings on its own, over the menu.
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, { name: 'Imaging Rig e2e', slug: 'imaging-rig-e2e' });
  await waitForCacheReady(request, entry.id);
});

async function revokeNamed(request: APIRequestContext, name: string) {
  const listed = await (await request.get('/api/workers')).json();
  for (const token of listed.data?.tokens ?? []) {
    if (token.name === name) await request.delete(`/api/workers/tokens/${token.id}`);
  }
}

test.afterEach(async ({ request }, testInfo) => {
  worker?.kill();
  worker = null;
  await request.put('/api/workers/placement', { data: { rc_astro: 'workers_first' } });
  await revokeNamed(request, `e2e-worker-${testInfo.project.name}`);
});

test('a worker joins with the token shown once, and stops when it is revoked', async ({ page, request }, testInfo) => {
  const name = `e2e-worker-${testInfo.project.name}`;
  await revokeNamed(request, name);
  await page.goto('/');
  await openSettings(page);
  await page.getByRole('tab', { name: 'Workers' }).click();

  // This server's own worker is listed even with no rc-astro here.
  const local = page.locator('.worker-row', { has: page.getByText('This server', { exact: true }) });
  await expect(local).toContainText('No RC-Astro tools');
  await expect(local).toContainText('Reads task files in place');

  // Where RC-Astro runs saves at once; stack builds and WBPP stay here.
  // Each radio shows the saved choice, so it turns on once the save returns.
  const workersOnly = page.getByRole('radio', { name: /^Workers only/ });
  await workersOnly.click();
  await expect(workersOnly).toBeChecked();
  await expect.poll(async () => (await (await request.get('/api/workers')).json()).data.placement.rc_astro)
    .toBe('workers_only');
  await expect(page.getByText('Stack builds and WBPP runs stay on this server.')).toBeVisible();
  await page.getByRole('radio', { name: /^Workers first/ }).click();
  await expect(page.getByRole('radio', { name: /^Workers first/ })).toBeChecked();

  await page.getByRole('button', { name: '+ Add a worker' }).click();
  await page.getByLabel('Worker name').fill(name);
  await page.getByRole('button', { name: 'Create token' }).click();
  const secret = page.getByRole('status').filter({ hasText: 'Copy a command now.' });
  const command = (await secret.locator('[data-shell="posix"]').textContent()) ?? '';
  // This page is on 127.0.0.1, which another computer cannot reach, so the
  // command has a placeholder and says to replace it.
  expect(command).toContain('psf-guard worker --server http://THIS-SERVER:PORT');
  await expect(secret).toContainText('Replace THIS-SERVER:PORT');
  await expect(secret.locator('[data-shell="powershell"]')).toContainText('$env:PSF_GUARD_WORKER_TOKEN=');
  const token = /PSF_GUARD_WORKER_TOKEN=(psfw_[0-9a-f]{64})/.exec(command)?.[1];
  expect(token).toBeTruthy();
  await secret.screenshot({ path: testInfo.outputPath('worker-token.png') });
  await secret.getByRole('button', { name: 'Done' }).click();
  await expect(page.getByText(token!)).toHaveCount(0);
  const tokenRow = page.locator('.token-row', { hasText: name });
  await expect(tokenRow).toContainText('Not connected');

  // The worker shares this machine's disk with the server, so the exchange
  // probe succeeds and task files go by path.
  let output = '';
  let exited: (code: number | null) => void = () => {};
  const exit = new Promise<number | null>((resolve) => { exited = resolve; });
  worker = spawn(BINARY, ['worker', '--server', BASE, '--name', name, '--files', 'shared'], {
    env: { ...process.env, PSF_GUARD_WORKER_TOKEN: token!, PSF_GUARD_RC_ASTRO: 'off' },
  });
  worker.stdout?.on('data', (chunk) => { output += chunk; });
  worker.stderr?.on('data', (chunk) => { output += chunk; });
  worker.on('exit', (code) => exited(code));

  const row = page.locator(`.worker-row[data-worker="${name}"]`);
  await expect(row).toContainText('Connected', { timeout: 30_000 });
  await expect(row).toContainText('Shared files');
  await expect(row).toContainText('No RC-Astro tools');
  await expect(tokenRow).toContainText('In use');
  await row.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('worker-connected.png') });

  page.once('dialog', (dialog) => void dialog.accept());
  await tokenRow.getByRole('button', { name: `Revoke the token for ${name}` }).click();
  await expect(tokenRow).toHaveCount(0);
  await expect(row).toHaveCount(0);
  // The worker's next request is refused; it stops and says what to do.
  const code = await Promise.race([
    exit,
    new Promise<string>((resolve) => setTimeout(() => resolve('still running'), 45_000)),
  ]);
  expect(code, output).not.toBe('still running');
  expect(code, output).not.toBe(0);
  expect(output).toContain('Settings › Workers');
});
