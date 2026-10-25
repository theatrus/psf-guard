import { expect, test, type APIRequestContext } from '@playwright/test';
import { spawn, type ChildProcess } from 'child_process';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

// A real `psf-guard worker` stacks a build's channel from the real fixture
// frames, on storage it shares with the server, when Settings › Workers
// sends stack builds to workers. The card fills in from the worker's frames
// and says which worker stacked it.

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const BINARY = process.env.PSF_GUARD_E2E_BINARY
  ?? path.resolve(__dirname, '../../target/release/psf-guard');
const BASE = `http://127.0.0.1:${Number(process.env.PSF_GUARD_E2E_PORT ?? 13099)}`;
const WORKER = 'e2e-stacker';

let worker: ChildProcess | null = null;
let output = '';
let dbId: string;

async function revokeOurs(request: APIRequestContext) {
  const listed = await (await request.get('/api/workers')).json();
  for (const token of listed.data?.tokens ?? []) {
    if (token.name === WORKER) await request.delete(`/api/workers/tokens/${token.id}`);
  }
}

test.beforeEach(async ({ request }, testInfo) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, {
    name: 'Worker stacking e2e',
    // A slug of its own, so no stack an earlier spec built is reused.
    slug: `worker-stacking-e2e-${testInfo.repeatEachIndex}-${testInfo.retry}`,
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
  // One stack per channel, as the stack spec builds.
  const mixed = await request.put(
    `/api/db/${encodeURIComponent(dbId)}/projects/1/processing-settings`,
    { data: { split_exposure_groups: false } },
  );
  expect(mixed.ok()).toBeTruthy();

  await revokeOurs(request);
  const placed = await request.put('/api/workers/placement', {
    data: { rc_astro: 'workers_first', stack: 'workers_only' },
  });
  expect(placed.ok()).toBeTruthy();
  const created = await (await request.post('/api/workers/tokens', { data: { name: WORKER } })).json();
  const token = created.data.token as string;
  output = '';
  worker = spawn(BINARY, ['worker', '--server', BASE, '--name', WORKER, '--files', 'shared', '--work', 'stack'], {
    env: { ...process.env, PSF_GUARD_WORKER_TOKEN: token, PSF_GUARD_RC_ASTRO: 'off' },
  });
  worker.stdout?.on('data', (chunk) => { output += chunk; });
  worker.stderr?.on('data', (chunk) => { output += chunk; });
  await expect.poll(async () => {
    const listed = await (await request.get('/api/workers')).json();
    return listed.data.workers.find((entry: { name: string }) => entry.name === WORKER)?.stacking_version ?? null;
  }, { timeout: 30_000 }).not.toBeNull();
});

test.afterEach(async ({ request }) => {
  worker?.kill();
  worker = null;
  await revokeOurs(request);
  await request.put('/api/workers/placement', { data: { rc_astro: 'workers_first', stack: 'this_server' } });
});

test('a worker stacks a channel, and its card says which worker did', async ({ page, request }, testInfo) => {
  test.setTimeout(300_000);
  await page.setViewportSize({ width: 1440, height: 1600 });
  await page.route('**/images/*/preview?*', (route) => route.abort());
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=1`);

  const panel = page.locator('.stack-preview-panel');
  await expect(panel).toBeVisible({ timeout: 15_000 });
  await panel.getByRole('button', { name: 'Build channel', exact: true }).click();

  const results = panel.locator('.stack-preview-results');
  await expect(results).toHaveAttribute('data-job-state', 'completed', { timeout: 210_000 });
  await expect(panel.locator('.stack-group-state.ready')).toHaveText('ready');
  await expect(panel.locator('.stack-preview-progress')).toContainText('3/3 frames');
  await expect(panel).toContainText(`Stacked on ${WORKER}`);
  await panel.screenshot({ path: testInfo.outputPath('stacked-on-worker.png') });

  // The stack, its curve and its checkpoint are this server's, as a build
  // it stacked itself would leave them.
  const fitsHref = await panel.getByRole('link', { name: 'Download linear FITS' }).getAttribute('href');
  const fitsHead = await page.request.head(fitsHref!);
  expect(fitsHead.status()).toBe(200);
  expect(Number(fitsHead.headers()['content-length'])).toBeGreaterThan(10_000_000);
  const jobId = fitsHref!.match(/\/stack-previews\/([a-f0-9]{64})\/0\/fits/)![1];
  const job = await (await request.get(
    `/api/db/${encodeURIComponent(dbId)}/projects/1/stack-previews/${jobId}`,
  )).json();
  const group = job.data.groups[0];
  expect(group.frames.length).toBe(3);
  expect(group.frames[0].disposition).toBe('reference');
  expect(group.snr.points.length).toBeGreaterThanOrEqual(3);
  expect(output).toContain('Finished Stack of 3 frames');
});
