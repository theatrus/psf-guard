import { test, expect } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';
import { openSettings } from '../e2e/helpers';
import type { TransferJob } from '../src/api/transferTypes';
import type { JobRow } from '../src/api/types';

// The header row the server builds from a transfer (`transfer_row` in
// src/server/jobs.rs). The server reads transfers from the Director, which
// this test replaces in the browser, so the test adds the row itself.
function transferRow(job: TransferJob): JobRow | null {
  if (job.state === 'complete' || job.state === 'cancelled') return null;
  const active = job.state === 'uploading' || job.state === 'preparing';
  const sent = job.completed_bytes + job.sent_bytes;
  const labels: Record<string, string> = { preparing: 'Checking files', review: 'Review required', queued: 'Pending', uploading: 'Uploading', paused: 'Paused', retry: 'Retry scheduled', needs_review: 'Needs attention' };
  return {
    key: `transfer:${job.id}`, kind: 'transfer', title: 'Transfers', scope: `${job.catalog} · ${job.label}`,
    detail: job.error ?? `${job.completed_files}/${job.total_files} files · ${(sent / 1024 / 1024).toFixed(1)} MiB`,
    status: active ? 'running' : job.state === 'review' || job.state === 'needs_review' ? 'attention' : job.state === 'queued' ? 'queued' : 'waiting',
    background: false, automatic: false, stoppable: false,
    percent: active && job.total_bytes > 0 ? Math.min(100, (100 * sent) / job.total_bytes) : null,
    state_label: labels[job.state], hint: job.current_file || undefined, transfer: job,
  };
}

test('pending uploads offer stacks and remain visible and controllable in background activity', async ({ page, request }, testInfo) => {
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'transfers-'));
  const dbPath = path.join(root, 'catalog.sqlite'); const db = new Database(dbPath); applyRealSchema(db);
  db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(1,'profile','M31','',1,1,0,0,?)").run(randomUUID()); db.close();
  const slug = `transfers-${randomUUID().slice(0, 8)}`;
  const added = await request.post('/api/databases', { data: { name: 'Transfer rig', slug, db_path: dbPath, image_dirs: [root] } }); expect(added.ok()).toBeTruthy();
  const connection = randomUUID(), image = randomUUID(), visit = randomUUID();
  const operations: Record<string, unknown>[] = []; const jobs: TransferJob[] = [];
  const errors: string[] = []; page.on('pageerror', e => errors.push(e.message));
  await page.route('**/api/director/v1/transfers', r => r.fulfill({ json: { success: true, data: { jobs, policies: [] } } }));
  await page.route('**/api/jobs', async route => {
    const reply = await (await route.fetch()).json();
    reply.data.jobs.push(...jobs.map(transferRow).filter(row => row !== null));
    await route.fulfill({ json: reply });
  });
  await page.route('**/api/director/v1/rigs/*/collaboration', r => r.fulfill({ json: { success: true, data: [{ status: 'registered', binding: { id: connection, rig_id: randomUUID(), name: 'Transfer rig', base_url: 'https://collab.example/', agent_id: '000000000001', state: 'registered', allow_loopback_http: false } }] } }));
  await page.route(`**/api/director/v1/collaboration/${connection}/work`, async route => {
    const input = route.request().postDataJSON(); operations.push(input); let data: unknown = {};
    switch (input.operation) {
      case 'report_inputs': data = { imports: [{ id: visit, name: 'M31', night: '2026-10-05', panels: [0] }], catalogs: [{ id: slug, name: 'Transfer rig' }] }; break;
      case 'report_candidates': data = { images: [{ guid: image, file: 'M31-OIII.fits', filter: 'OIII', target: 'M31', captured_at: 1791171000, panel: 0 }] }; break;
      case 'transfer_outputs': data = { outputs: [
        { job_id: 'a'.repeat(64), group_index: 0, revision: 'revision', label: 'PSF Guard · M31 · OIII', wbpp: false, matches_selection: true },
        { job_id: 'b'.repeat(64), group_index: 0, revision: 'revision', label: 'WBPP · M31 · OIII', wbpp: true },
      ] }; break;
      case 'upload_pending': jobs.push({ id: randomUUID(), connection_id: connection, revision: 1, state: 'uploading', catalog: slug, label: 'M31', total_files: 1, completed_files: 0, total_bytes: 10485760, completed_bytes: 0, sent_bytes: 3145728, current_file: 'matched-flat-stack.fits', error: null, next_attempt_ms: 0, warnings: ['Calibration associations are retained locally.'], manifest: null }); data = { transfer: jobs[0] }; break;
      case 'transfer_manifest': data = { transfer_manifest: { version: 1, task: 'test-task', night: '2026-10-05', panel: 0, lights: [image], calibration: ['matched-flat'], output: 'PSF Guard stack', files: [{ kind: 'light', source: { path: 'C:/catalog/stacks/matched-flat-stack.fits', bytes: 10485760 } }] } }; break;
      case 'transfer_control':
        jobs[0].revision++; jobs[0].state = input.action === 'approve' || input.action === 'resume' ? 'uploading' : input.action === 'pause' ? 'paused' : 'cancelled';
        if (jobs[0].state === 'uploading') { jobs[0].sent_bytes = 3145728; jobs[0].current_file = 'matched-flat-stack.fits'; }
        else { jobs[0].sent_bytes = 0; jobs[0].current_file = ''; }
        data = { transfer: jobs[0] }; break;
    }
    await route.fulfill({ json: { success: true, data } });
  });
  await page.goto('/#/'); await openSettings(page); const settings = page.locator('.tauri-settings');
  await settings.getByRole('tab', { name: 'Overview', exact: true }).click(); await settings.getByRole('button', { name: 'Open Transfer rig' }).click();
    await settings.getByRole('tab', { name: 'Rig setup', exact: true }).click();
  await settings.getByRole('tab', { name: 'Collaboration', exact: true }).click();
  const collaboration = settings.getByRole('region', { name: 'Collaboration', exact: true });
  await collaboration.getByRole('tab', { name: 'Reports' }).click();
  await collaboration.getByLabel('Imported visit').selectOption(visit); await collaboration.getByLabel('Remote panel').selectOption('0');
  await collaboration.getByLabel('Select M31-OIII.fits').check();
  await page.setViewportSize({ width: 1440, height: 1100 });
  await settings.locator('.modal-body').evaluate(el => { el.scrollTop = 0; });
  await page.screenshot({ path: testInfo.outputPath('settings-context-1440.png'), animations: 'disabled' });
  const files = collaboration.getByRole('region', { name: 'Contribution files' });
  await expect(files.getByLabel('Upload accepted collaboration files automatically')).not.toBeChecked();
  await files.getByLabel('Files to send').selectOption({ label: 'WBPP · M31 · OIII' });
  await expect(files.getByRole('button', { name: 'Upload pending' })).toBeDisabled();
  await files.evaluate(el => el.scrollIntoView({ block: 'start' }));
  await settings.locator('.modal-body').evaluate(el => { el.scrollTop -= 200; });
  await page.screenshot({ path: testInfo.outputPath('wbpp-confirmation-1440.png'), animations: 'disabled' });
  await files.getByLabel('I confirm this WBPP stack contains the selected images').check();
  await expect(files.getByRole('button', { name: 'Upload pending' })).toBeEnabled();
  await files.getByLabel('Files to send').selectOption({ label: 'PSF Guard · M31 · OIII' });
  await files.getByRole('button', { name: 'Upload pending' }).click();
  await expect(files).toContainText('Uploading');
  await expect(files.getByRole('button', { name: 'Upload reviewed files' })).toHaveCount(0);
  expect(operations.find(o => o.operation === 'upload_pending')).toMatchObject({ request: { output: { job_id: 'a'.repeat(64) }, selection: { image_guids: [image], catalog: slug, import_id: visit } } });
  await files.getByRole('button', { name: 'Inspect files' }).click(); await expect(files).toContainText('matched-flat');
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 1100 });
    await files.evaluate(el => el.scrollIntoView({ block: 'start' }));
    if (width >= 800) await settings.locator('.modal-body').evaluate(el => { el.scrollTop -= 200; });
    expect(await files.evaluate(el => el.scrollWidth <= el.clientWidth)).toBeTruthy();
    await page.screenshot({ path: testInfo.outputPath(`transfers-${width}.png`), animations: 'disabled' });
  }
  // Inject an exceptional server reply to verify that only this path needs approval.
  jobs[0].state = 'review'; jobs[0].sent_bytes = 0; jobs[0].current_file = '';
  jobs[0].warnings = ['Review these files before uploading. This transfer was prepared for review or follows an earlier upload exception.'];
  await expect(files).toContainText('Review required');
  await page.setViewportSize({ width: 1440, height: 1100 });
  await files.evaluate(el => el.scrollIntoView({ block: 'start' }));
  await settings.locator('.modal-body').evaluate(el => { el.scrollTop -= 200; });
  await page.screenshot({ path: testInfo.outputPath('exception-review-1440.png'), animations: 'disabled' });
  await files.getByRole('button', { name: 'Upload reviewed files' }).click(); await expect(files).toContainText('Uploading');
  await settings.getByRole('button', { name: 'Close settings', exact: true }).click();
  await expect(settings).toBeHidden();
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.getByRole('button', { name: /^Background jobs:/ }).click();
  const activity = page.getByRole('region', { name: 'Background jobs' }); await expect(activity).toContainText('Transfers');
  await expect(activity).toContainText('3.0 MiB');
  await page.screenshot({ path: testInfo.outputPath('background-uploading-1440.png'), animations: 'disabled' });
  await activity.getByRole('button', { name: 'Pause transfer' }).click(); await expect(activity).toContainText('Paused');
  await page.screenshot({ path: testInfo.outputPath('background-paused-1440.png'), animations: 'disabled' });
  await expect(activity.locator('.activity-row').filter({ hasText: 'Transfers' }).locator('.is-indeterminate')).toHaveCount(0);
  await page.reload(); await page.getByRole('button', { name: /^Background jobs:/ }).click();
  const restored = page.getByRole('region', { name: 'Background jobs' }); await expect(restored).toContainText('Paused');
  await restored.getByRole('button', { name: 'Resume transfer' }).click(); await expect(restored).toContainText('Uploading');
  await restored.getByRole('button', { name: 'Cancel pending transfer' }).click();
  await expect(page.getByRole('region', { name: 'Background jobs' }).getByText('Transfers', { exact: true })).toHaveCount(0);
  expect(errors).toEqual([]);
});
