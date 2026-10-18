import { expect, test, type APIRequestContext, type Page, type Route } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { openSettings, registerFixtureDb, resetDatabases, tmpBase } from './helpers';

/**
 * Settings › Backups against the real server: a backup made now and
 * downloaded, a schedule saved, and a running backup in the job line. The
 * server writes only below this run's tmp directory: its download area is
 * in the run's cache, and the schedule's folder is chosen there.
 */

const OFF = {
  enabled: false, frequency: 'daily', weekday: 6, time: '13:00', destination: null,
  scope: 'full', include_secrets: false, retention: { kind: 'count', count: 7 },
};

async function turnScheduleOff(request: APIRequestContext) {
  const response = await request.put('/api/backups/settings', { data: OFF });
  expect(response.ok()).toBeTruthy();
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  await registerFixtureDb(request, { name: 'Imaging Rig e2e', slug: 'imaging-rig-e2e' });
  await turnScheduleOff(request);
});

test.afterAll(async ({ request }) => {
  await turnScheduleOff(request);
});

async function openBackups(page: Page) {
  await openSettings(page);
  await page.getByRole('tab', { name: 'Backups' }).click();
}

test('backs up settings and catalogs now and downloads the archive', async ({ page, request }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await openBackups(page);
  const card = page.getByRole('region', { name: 'Back up now' });
  await expect(card.getByText(/plus the catalog\./)).toBeVisible();
  await card.getByRole('button', { name: 'Back up now' }).click();
  const done = card.locator('.backup-note.is-good');
  await expect(done).toContainText(/Backed up .*psf-guard-backup-.*-full\.zip/, { timeout: 30_000 });

  const downloading = page.waitForEvent('download');
  await done.getByRole('link', { name: 'Download' }).click();
  const download = await downloading;
  expect(download.suggestedFilename()).toMatch(/^psf-guard-backup-.*-full\.zip$/);
  const saved = path.join(tmpBase(), 'downloaded-backup.zip');
  await download.saveAs(saved);
  // A zip: its local-file signature, and room for the catalog.
  const bytes = fs.readFileSync(saved);
  expect(bytes.subarray(0, 4).toString('hex')).toBe('504b0304');

  // The list shows it in the download area, with the catalog counted.
  const list = page.getByRole('region', { name: 'Backups', exact: true });
  await expect(list.getByText(/Settings and 1 catalog · manual · download area/)).toBeVisible();
  const overview = await (await request.get('/api/backups')).json();
  expect(overview.data.backups[0].catalogs).toBe(1);
  expect(overview.data.last_run.outcome).toBe('succeeded');
});

test('saves a weekly schedule to a folder and says when it runs next', async ({ page, request }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await openBackups(page);
  const card = page.getByRole('region', { name: 'Scheduled backups' });
  await expect(card.getByText('Not scheduled')).toBeVisible();
  await card.getByRole('checkbox', { name: /Back up on a schedule/ }).check();
  const folder = path.join(tmpBase(), 'scheduled-backups');
  await card.getByLabel(/^Folder/).fill(folder);
  await card.getByLabel('Every', { exact: true }).selectOption('weekly');
  await card.getByLabel('On', { exact: true }).selectOption('Wednesday');
  await card.getByLabel('At', { exact: true }).fill('12:30');
  await card.getByLabel('Keep', { exact: true }).selectOption('age');
  await card.getByLabel('Keep (days)').fill('30');
  await card.getByRole('button', { name: 'Save schedule' }).click();
  await expect(card.getByRole('button', { name: 'Save schedule' })).toBeDisabled();
  await expect(card.locator('.backup-schedule-status dd').first()).not.toHaveText('Not scheduled');

  const overview = await (await request.get('/api/backups')).json();
  expect(overview.data.settings).toMatchObject({
    enabled: true, frequency: 'weekly', weekday: 2, time: '12:30', destination: folder,
    retention: { kind: 'age', days: 30 },
  });
  // The next run is a Wednesday, 12:30 server time, within the week.
  const next = new Date(overview.data.next_run_at * 1000);
  expect(next.getDay()).toBe(3);
  expect(next.getTime() - Date.now()).toBeLessThanOrEqual(7 * 86_400_000);
});

test('warns about a folder beside the catalog', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/');
  await openBackups(page);
  const card = page.getByRole('region', { name: 'Back up now' });
  await card.getByLabel(/Save on the server/).fill(path.join(tmpBase(), 'beside-the-catalog'));
  await expect(card.getByText(/inside the catalog folder for Imaging Rig e2e/)).toBeVisible();
});

function fulfil(route: Route, data: unknown) {
  return route.fulfill({ json: { success: true, data, error: null, status: 'ready' } });
}

test('lists a scheduled backup in the job line and stops it from there', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  let stopped = false;
  // A real backup of the fixture takes a moment; the job line is held at
  // the moment one runs.
  await page.route('**/api/backups/status', (route) => fulfil(route, {
    last_run: null,
    job: {
      trigger: 'schedule', scope: 'full', include_secrets: false, destination: '/mnt/nas/backups',
      location: 'destination', stage: 'copying', current: 'catalog imaging-rig-e2e', files_done: 2,
      files_total: 6, bytes_done: 40, bytes_total: 100, waiting_for: null,
      started_at: 1_760_000_000, stopping: stopped,
    },
  }));
  await page.route('**/api/backups/stop', (route) => {
    stopped = true;
    return fulfil(route, { job: null, last_run: null });
  });
  await page.goto('/');
  const chip = page.getByRole('button', { name: /^Background jobs/ });
  await chip.click();
  const list = page.getByRole('region', { name: 'Background jobs' });
  const row = list.getByRole('listitem').filter({ hasText: 'Scheduled backup' });
  await expect(row).toContainText('Settings and catalogs');
  // Listed with automatic work, not the work a person started.
  await expect(list.getByRole('group', { name: 'Automatic' }).getByRole('listitem')
    .filter({ hasText: 'Scheduled backup' })).toHaveCount(1);
  await expect(row).toContainText('Copying catalog imaging-rig-e2e · 2/6 files');
  await row.getByRole('button', { name: 'Stop Scheduled backup: Settings and catalogs' }).click();
  await row.getByRole('button', { name: 'Stop it' }).click();
  await expect.poll(() => stopped).toBe(true);
  const widths = await page.evaluate(() => ({
    scroll: document.documentElement.scrollWidth,
    inner: window.innerWidth,
  }));
  expect(widths.scroll).toBeLessThanOrEqual(widths.inner);
});
