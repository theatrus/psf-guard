import { expect, test } from '@playwright/test';
import { mkdtempSync, realpathSync } from 'node:fs';
import Database from 'better-sqlite3';
import { applyRealSchema } from './fixtures/sync';
import * as path from 'node:path';
import { fixtureDbPath, fixtureImageDir, resetDatabases, tmpBase } from './helpers';

// New file imports require the stable GUIDs supplied by current TS. The
// shared legacy fixture still exercises direct-open compatibility elsewhere.
function tsSource(): string {
  const source = path.join(mkdtempSync(path.join(tmpBase(), 'ts-import-')), 'scheduler.sqlite');
  const db = new Database(source);
  try {
    applyRealSchema(db);
    db.prepare('ATTACH DATABASE ? AS fixture').run(fixtureDbPath());
    db.exec(`INSERT INTO project(Id,profileId,name,description,guid)
      SELECT Id,profileId,name,description,'project-'||Id FROM fixture.project;
      INSERT INTO target(Id,projectid,name,active,ra,dec,epochcode,guid)
      SELECT Id,projectId,name,active,ra,dec,0,'target-'||Id FROM fixture.target;
      INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,guid)
      SELECT Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,'image-'||Id FROM fixture.acquiredimage;`);
  } finally { db.close(); }
  return realpathSync(source);
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
});

test('create an empty sync destination and load its overview without errors', async ({
  page,
  request,
}, testInfo) => {
  const incoming = mkdtempSync(path.join(tmpBase(), 'empty-incoming-'));
  const failedRequests: string[] = [];
  page.on('response', (response) => {
    if (response.url().includes('/api/db/') && response.status() >= 500) {
      failedRequests.push(response.url());
    }
  });
  await page.goto('/');
  await page.getByRole('heading', { name: /Welcome to PSF Guard/i }).waitFor();
  await page.locator('.tauri-settings')
    .getByRole('button', { name: /New Database from Images/i }).click();
  await page.getByPlaceholder('e.g. 2026 Archive (defaults to "Imported Images")')
    .fill('Empty sync destination');
  await page.getByPlaceholder('Type an absolute path and press Add').fill(incoming);
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  const createdResponse = page.waitForResponse((response) =>
    response.url().endsWith('/api/databases/create') && response.request().method() === 'POST');
  await page.getByRole('button', { name: 'Create & Import', exact: true }).click();
  const created = await createdResponse;
  expect(created.ok()).toBe(true);
  const dbId = (await created.json()).data.database.id as string;
  const prefix = `/api/db/${encodeURIComponent(dbId)}`;
  await expect.poll(async () => {
    const response = await request.get(`${prefix}/import`);
    return (await response.json()).data.progress.stage;
  }).toBe('complete');

  const response = await request.get(`${prefix}/stats/overall`);
  expect(response.status(), await response.text()).toBe(200);
  const stats = (await response.json()).data;
  for (const field of ['total_images', 'accepted_images', 'rejected_images', 'pending_images']) {
    expect(stats[field]).toBe(0);
  }
  expect(stats.date_range).toEqual({ earliest: null, latest: null, span_days: null });
  await page.getByRole('button', { name: 'Done', exact: true }).click();
  await expect(page.locator('.overview-summary')).toContainText('0 images');
  await expect(page.getByText('No projects with images yet.', { exact: true })).toBeVisible();
  await page.reload();
  await expect(page.locator('.overview-summary')).toContainText('0 images');
  expect(failedRequests).toEqual([]);
  await page.screenshot({ path: testInfo.outputPath('empty-catalog-overview.png'), fullPage: true });
});

test('import a Target Scheduler database via settings → see it in the Overview', async ({ page, request }) => {
  const source = tsSource();
  await page.goto('/');

  // Wait for settings to auto-open (gated on a couple of async API calls).
  await page.getByRole('heading', { name: /Welcome to PSF Guard/i }).waitFor();

  // First run offers two starting points and picks neither for you.
  await page
    .locator('.tauri-settings')
    .getByRole('button', { name: /Import from Target Scheduler/i })
    .click();

  // The form is now visible. Fill the inputs.
  await page
    .getByPlaceholder('e.g. Imaging Rig (defaults to filename)')
    .fill('e2e Rig');
  await page
    .getByPlaceholder('Select or enter database path')
    .fill(source);

  // Add the image directory via the manual-add fallback in browser mode.
  await page
    .getByPlaceholder('Type an absolute path and press Add')
    .fill(fixtureImageDir());
  await page.getByRole('button', { name: 'Add', exact: true }).click();

  // The directory should appear in the list.
  await expect(page.getByText(fixtureImageDir())).toBeVisible();

  // Save the new entry.
  await page.locator('.tauri-settings').getByRole('button', { name: 'Import from Target Scheduler', exact: true }).click();

  // Wait for the saved status (proves the round-trip succeeded), and the row
  // should appear in the "Configured Databases" list inside the modal.
  await expect(page.getByText('Target Scheduler import complete. Automatic sync and grade writeback are enabled.')).toBeVisible();
  await expect(page.getByText('e2e Rig').first()).toBeVisible();
  const databases = (await (await request.get('/api/databases')).json()).data;
  const catalog = databases.find((database: { name: string }) => database.name === 'e2e Rig');
  expect(catalog.database_path).not.toBe(source);
  expect(catalog.ts_sync.source_path).toBe(source);
  expect(catalog.ts_sync.writeback).toBe(true);
  await expect(page.getByRole('checkbox', { name: 'Sync when new images or grades arrive' })).toBeChecked();

  // Close the modal and verify the Overview reflects both projects from the
  // new database. The merged-overview hooks fan out per-DB queries; give them
  // a moment to settle after the registry mutation.
  await page.getByRole('button', { name: 'Done' }).click();
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({
    timeout: 15_000,
  });
  await expect(
    page.getByRole('button', { name: 'Open Project Alpha image grid' })
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Open Project Beta image grid' })
  ).toBeVisible();
  await expect(page.locator('.project-database', { hasText: 'e2e Rig' })).toHaveCount(2);
});

test('first-run plugin import creates a catalog and opens pairing without a TS file', async ({ page }) => {
  const receive = mkdtempSync(path.join(tmpBase(), 'plugin-incoming-'));
  await page.goto('/');
  await page.locator('.tauri-settings').getByRole('button', { name: /Import from Target Scheduler/ }).click();
  await page.getByRole('radio', { name: 'Use the PSF Guard Sync plugin' }).check();
  await expect(page.getByPlaceholder('Select or enter database path')).toBeHidden();
  await page.getByPlaceholder('e.g. Imaging Rig (defaults to filename)').fill('Plugin Rig');
  await page.getByPlaceholder('Type an absolute path and press Add').fill(receive);
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await page.getByRole('button', { name: 'Create catalog for plugin' }).click();
  await page.getByRole('button', { name: 'Generate pairing code' }).click();
  await expect(page.locator('.remote-upload-token-notice').filter({ hasText: 'Paste this code' })).toBeVisible();
});

test('add then remove a database', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('heading', { name: /Welcome to PSF Guard/i }).waitFor();
  await page
    .locator('.tauri-settings')
    .getByRole('button', { name: /Import from Target Scheduler/i })
    .click();

  await page.getByPlaceholder('Select or enter database path').fill(tsSource());
  // The backend rejects a DB without image dirs, so populate one.
  await page
    .getByPlaceholder('Type an absolute path and press Add')
    .fill(fixtureImageDir());
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await page.locator('.tauri-settings').getByRole('button', { name: 'Import from Target Scheduler', exact: true }).click();

  // Saved status appears once the POST completes; the row follows.
  await expect(page.getByText('Target Scheduler import complete. Automatic sync and grade writeback are enabled.')).toBeVisible();
  const row = page.locator('.db-row').first();
  await expect(row).toBeVisible();

  page.on('dialog', (dialog) => dialog.accept());
  await row.getByRole('button', { name: 'Remove' }).click();

  // After removing the only DB, the registry is empty again and the welcome
  // banner returns with both starting points on offer.
  await expect(page.getByText(/Removed/i)).toBeVisible();
  await expect(
    page.getByRole('heading', { name: /Welcome to PSF Guard/i })
  ).toBeVisible();
  await expect(
    page
      .locator('.tauri-settings')
      .getByRole('button', { name: /New Database from Images/i })
  ).toBeVisible();
});
