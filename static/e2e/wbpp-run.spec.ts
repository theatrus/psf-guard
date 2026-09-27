import { expect, test } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { registerFixtureDb, resetDatabases, tmpBase, waitForCacheReady } from './helpers';

let dbId: string;
let fakeBinary: string;

/**
 * A stand-in for PixInsight: reads the script and output folder out of the
 * `-r=` argument the way PixInsight would hand them to WBPP, writes a WBPP
 * style log and a master, and exits. The real thing takes an hour and a
 * licence; the plumbing around it is what this checks. It takes a few
 * seconds so a test can queue a second project behind the running one.
 */
function installFakePixInsight(root: string): string {
  const bin = path.join(root, 'bin');
  const scripts = path.join(root, 'src', 'scripts', 'BatchPreprocessing');
  fs.mkdirSync(bin, { recursive: true });
  fs.mkdirSync(scripts, { recursive: true });
  fs.writeFileSync(path.join(scripts, 'BPP-Main.js'), '// stand-in\n');
  fs.writeFileSync(
    path.join(scripts, 'BPP-Global.js'),
    'BPP.Version = {\n   WBPP_VERSION:          "3.1.0",\n};\n'
  );
  const binary = path.join(bin, 'PixInsight.sh');
  fs.writeFileSync(
    binary,
    `#!/bin/sh
for a in "$@"; do case "$a" in -r=*) R="\${a#-r=}";; esac; done
SCRIPT=$(printf '%s' "$R" | tr ',' '\\n' | sed -n '1p')
OUT=$(printf '%s' "$R" | tr ',' '\\n' | sed -n 's/^outputDirectory=//p')
mkdir -p "$OUT/logs" "$OUT/master"
LOG="$OUT/logs/fake.log"
echo "Weighted Batch Preprocessing Script 3.1.0" > "$LOG"
echo "stand-in for PixInsight; script <raw>$SCRIPT</raw>" >> "$LOG"
printf '%s' "$R" | tr ',' '\\n' | sed 's/^/automation mode parameter: /' >> "$LOG"
echo "* Begin registration of light frames" >> "$LOG"
sleep 8
echo "* End registration of light frames" >> "$LOG"
cp "$SCRIPT" "$OUT/master/masterLight_BIN-1_FILTER-B.xisf"
echo "* WeightedBatchPreprocessing: 00:00:01.000" >> "$LOG"
exit 0
`,
    { mode: 0o755 }
  );
  return binary;
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, { name: 'WBPP Rig', slug: 'wbpp-rig' });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
  fakeBinary = installFakePixInsight(path.join(tmpBase(), 'fake-pixinsight'));
});

test.afterEach(async ({ request }) => {
  await request.put('/api/settings/pixinsight', { data: { binary: null, runs_dir: null } });
  await resetDatabases(request);
});

test('PixInsight settings report what is found and where it will draw', async ({ request }) => {
  const missing = await request.put('/api/settings/pixinsight', {
    data: { binary: path.join(tmpBase(), 'nowhere', 'PixInsight.sh') },
  });
  expect(missing.ok()).toBe(true);
  const notFound = (await missing.json()).data;
  expect(notFound.ready).toBe(false);
  expect(notFound.detection.install).toBeNull();
  expect(notFound.detection.problem).toContain('not a file');

  const configured = await request.put('/api/settings/pixinsight', { data: { binary: fakeBinary } });
  const settings = (await configured.json()).data;
  expect(settings.binary).toBe(fakeBinary);
  expect(settings.detection.source).toBe('configured');
  expect(settings.detection.install.wbpp_version).toBe('3.1.0');
  expect(['own', 'xvfb', 'missing']).toContain(settings.display.kind);

  const again = await request.get('/api/settings/pixinsight');
  expect((await again.json()).data.binary).toBe(fakeBinary);
});

test('a run writes the script for the install, launches it, and lists what it wrote', async ({
  request,
}) => {
  // Runs go where the settings say, below the database's slug, with the
  // space free there reported; the cache is only the last resort.
  const runsDir = path.join(tmpBase(), 'wbpp-runs');
  const configured = (
    await (
      await request.put('/api/settings/pixinsight', { data: { binary: fakeBinary, runs_dir: runsDir } })
    ).json()
  ).data;
  test.skip(!configured.ready, 'no display and no xvfb-run on this machine');
  expect(configured.runs_dir).toBe(runsDir);
  expect(configured.runs_dir_free_bytes).toBeGreaterThan(0);

  // The database's process directory is where finished work lives; a run
  // can save its masters there as it ends.
  const processDir = path.join(tmpBase(), '_Process');
  const updated = await request.put(`/api/databases/${dbId}`, { data: { process_dir: processDir } });
  expect(updated.ok()).toBe(true);
  expect((await updated.json()).data.process_directory).toBe(processDir);

  const started = await request.post(`/api/db/${dbId}/wbpp/runs`, {
    data: {
      project_id: 1,
      include_pending: true,
      options: { quality: 'good', fast_integration: 'off', drizzle: '2x', autocrop: false },
      extra_params: ['maxStars=500'],
      scope_label: 'Project Alpha',
      publish_folder: '2026-alpha-v1',
    },
  });
  expect(started.ok()).toBe(true);
  expect((await started.json()).data.started).toBe(true);

  // A second start while the first runs joins the line rather than starting
  // a second PixInsight; taken out again here so the first run stays the
  // current one for the checks below.
  const second = await request.post(`/api/db/${dbId}/wbpp/runs`, {
    data: { project_id: 2, include_pending: true, options: {}, scope_label: 'Project Beta' },
  });
  const secondBody = (await second.json()).data;
  if (secondBody.progress.running) {
    expect(secondBody.started).toBe(false);
    expect(secondBody.queue_id).toBeTruthy();
    expect(secondBody.queued).toMatchObject([{ scope: 'Project Beta', position: 1, project_id: 2 }]);
    const left = await request.delete(`/api/db/${dbId}/wbpp/runs/queue/${secondBody.queue_id}`);
    expect(left.ok()).toBe(true);
    expect((await left.json()).data.queued).toEqual([]);
    const gone = await request.delete(`/api/db/${dbId}/wbpp/runs/queue/${secondBody.queue_id}`);
    expect(gone.status()).toBe(404);
  }

  let progress;
  await expect
    .poll(
      async () => {
        progress = (await (await request.get(`/api/db/${dbId}/wbpp/runs/current`)).json()).data.progress;
        return progress.running;
      },
      { timeout: 30_000, intervals: [500] }
    )
    .toBe(false);
  expect(progress.stage, JSON.stringify(progress)).toBe('complete');
  expect(progress.work_dir.startsWith(path.join(runsDir, dbId) + path.sep)).toBe(true);
  expect(progress.free_bytes_at_start).toBeGreaterThan(0);
  expect(progress.lights).toBe(3);
  expect(progress.exit_code).toBe(0);
  expect(progress.wbpp_elapsed).toBe('00:00:01.000');
  expect(progress.log_tail.join('\n')).toContain('End registration of light frames');
  expect(progress.outputs.map((f: { kind: string }) => f.kind)).toContain('master');

  // The masters were saved as the run ended, the folder remembered for the
  // project, and a second save finds them already there.
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/db/${dbId}/wbpp/runs/current`)).json()).data.progress.publish?.state,
      { timeout: 15_000 }
    )
    .toBe('complete');
  const saved = path.join(processDir, '2026-alpha-v1', 'master', 'masterLight_BIN-1_FILTER-B.xisf');
  expect(fs.existsSync(saved)).toBe(true);
  const remembered = await request.get(`/api/db/${dbId}/projects/1/processing-settings`);
  expect((await remembered.json()).data.process_folder).toBe('2026-alpha-v1');
  const again = await request.post(`/api/db/${dbId}/wbpp/runs/current/publish`, {
    data: { folder: '2026-alpha-v1' },
  });
  expect(again.ok()).toBe(true);
  await expect
    .poll(
      async () =>
        (await (await request.get(`/api/db/${dbId}/wbpp/runs/current`)).json()).data.progress.publish,
      { timeout: 15_000 }
    )
    .toMatchObject({ state: 'complete', copied: 0, skipped_existing: 1, conflicts: [] });
  const bad = await request.post(`/api/db/${dbId}/wbpp/runs/current/publish`, { data: { folder: '..' } });
  expect(bad.status()).toBe(400);

  // The master downloads, as does the script and the log; nothing above the run does.
  const master = await request.get(
    `/api/db/${dbId}/wbpp/runs/current/files/wbpp-out/master/masterLight_BIN-1_FILTER-B.xisf`
  );
  expect(master.status()).toBe(200);
  expect(master.headers()['content-disposition']).toContain('masterLight_BIN-1_FILTER-B.xisf');
  const script = await request.get(`/api/db/${dbId}/wbpp/runs/current/files/run-wbpp.js`);
  expect(script.status()).toBe(200);
  const js = await script.text();
  expect(js).toContain(`#include "${path.join(tmpBase(), 'fake-pixinsight', 'src/scripts/BatchPreprocessing/BPP-Main.js')}"`);
  expect(js).toContain('"localNormalizationPsfType=2"');
  expect(js).toContain('"autoIntegrationMode=false"');
  expect(js).toContain('"autocrop=false"');
  expect(js).toContain('var psfDrizzle = { enabled: true, fast: true, scale: 2 };');
  expect(js).toContain('var psfLoadOnly = false;');
  expect(js).toMatch(/\{ path: "[^"]+\.fits" \}/);
  const log = await request.get(`/api/db/${dbId}/wbpp/runs/current/files/wbpp-out/logs/fake.log`);
  expect(await log.text()).toContain('automation mode parameter: maxStars=500');
  const escape = await request.get(`/api/db/${dbId}/wbpp/runs/current/files/../registry.json`);
  expect([400, 404]).toContain(escape.status());
});

test('the Overview stacks a project from its card and shows the masters', async ({ page, request }) => {
  const configured = (await (await request.put('/api/settings/pixinsight', { data: { binary: fakeBinary } })).json()).data;
  test.skip(!configured.ready, 'no display and no xvfb-run on this machine');

  await page.goto('/');
  const alphaCard = page.locator('.project-card').filter({ hasText: 'Project Alpha' });
  await expect(alphaCard).toBeVisible({ timeout: 15_000 });
  await alphaCard.getByText('Stack in WBPP').click();

  const dialog = page.locator('.wbpp-run-dialog');
  await expect(dialog).toContainText('Stack with WBPP — Project Alpha');
  await expect(dialog.getByRole('status')).toContainText('WBPP 3.1.0 as configured');
  await dialog.getByLabel(/^Drizzle/).selectOption('2x');
  await dialog.getByRole('button', { name: 'Start stacking' }).click();

  // While Alpha runs, Beta's action opens under Beta's own name, names the
  // busy run, and queues behind it; the queued run starts on its own.
  await expect(dialog).toContainText('PixInsight is running WBPP', { timeout: 15_000 });
  await dialog.locator('.dialog-footer').getByRole('button', { name: 'Close' }).click();
  const betaCard = page.locator('.project-card').filter({ hasText: 'Project Beta' });
  await betaCard.getByText('Stack in WBPP').click();
  await expect(dialog).toContainText('Stack with WBPP — Project Beta');
  await expect(dialog).toContainText('PixInsight is busy');
  await expect(dialog).toContainText('Project Alpha');
  await dialog.getByRole('button', { name: 'Queue stacking' }).click();
  await expect(dialog).toContainText('WBPP Project Beta is next in line');
  await dialog.locator('.dialog-footer').getByRole('button', { name: 'Close' }).click();
  await expect(page.locator('.overview-wbpp-queued')).toContainText('Project Beta is next in line');
  await expect(betaCard.getByText('Queued for WBPP')).toBeVisible();

  // Alpha finishes, Beta runs and finishes; the line shows Beta's result.
  await expect(page.locator('.overview-wbpp-run')).toContainText('WBPP Project Beta finished: 1 master', {
    timeout: 60_000,
  });
  await expect(page.locator('.overview-wbpp-queued')).toHaveCount(0);
  await page.locator('.overview-wbpp-run .link-button').click();
  await expect(dialog).toContainText('Stack with WBPP — Project Beta');
  await expect(dialog).toContainText('Finished');
  await expect(dialog.getByRole('link', { name: 'masterLight_BIN-1_FILTER-B.xisf' })).toBeVisible();
  await dialog.locator('.dialog-footer').getByRole('button', { name: 'Close' }).click();
  await expect(page.locator('.overview-wbpp-run')).toContainText('finished: 1 master');

  // A fresh tab still sees the run: the line comes back after a reload, the
  // project's action says the masters are ready, and either reopens the run.
  await page.reload();
  await expect(page.locator('.overview-wbpp-run')).toContainText('finished: 1 master', {
    timeout: 15_000,
  });
  const ready = page.locator('.project-card').filter({ hasText: 'Project Beta' }).getByText('WBPP masters ready');
  await expect(ready).toBeVisible();
  await ready.click();
  await expect(page.locator('.wbpp-run-dialog')).toContainText('Finished');
  await expect(
    page.locator('.wbpp-run-dialog').getByRole('link', { name: 'masterLight_BIN-1_FILTER-B.xisf' })
  ).toBeVisible();
  await page.locator('.wbpp-run-dialog .dialog-footer').getByRole('button', { name: 'Close' }).click();

  // × clears the finished run from the Overview, in this tab and the next.
  await page.locator('.overview-wbpp-run').getByRole('button', { name: /^Dismiss/ }).click();
  await expect(page.locator('.overview-wbpp-run')).toHaveCount(0);
  await page.reload();
  await expect(page.locator('.project-card').filter({ hasText: 'Project Beta' })).toBeVisible({ timeout: 15_000 });
  await expect(page.locator('.overview-wbpp-run')).toHaveCount(0);
  await expect(page.locator('.project-card').filter({ hasText: 'Project Beta' }).getByText('Stack in WBPP')).toBeVisible();
});
