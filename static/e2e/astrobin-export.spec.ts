import { expect, test } from '@playwright/test';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

let dbId: string;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, {
    name: 'AstroBin Rig',
    slug: 'astrobin-rig',
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
  await request.put('/api/settings/astrobin', { data: { filter_ids: {} } });
  await request.put(`/api/db/${dbId}/astrobin/filters`, { data: { entries: [] } });
});

// Leave the server as found: the specs that follow expect no database
// registered (the empty state opens Settings on its own) and no filter map.
test.afterEach(async ({ request }) => {
  await request.put('/api/settings/astrobin', { data: { filter_ids: {} } });
  await request.put(`/api/db/${dbId}/astrobin/filters`, { data: { entries: [] } });
  await resetDatabases(request);
});

test('the acquisition rows count one night of B lights, essentials and full', async ({
  request,
}) => {
  // Alpha M44: three B lights on one night, one accepted and two ungraded.
  const essentials = await request.get(`/api/db/${dbId}/astrobin-export`, {
    params: { target_id: 1 },
  });
  expect(essentials.ok()).toBe(true);
  const accepted = (await essentials.json()).data;
  expect(accepted.scope).toBe('Alpha M44');
  expect(accepted.rows).toEqual([
    expect.objectContaining({ date: '2026-04-16', filter: 'B', filter_id: null, number: 1, duration: 60 }),
  ]);
  expect(accepted.csv).toBe('date,filter,number,duration\n2026-04-16,,1,60\n');
  expect(accepted.unmapped_filters).toEqual(['B']);

  const withPending = await request.get(`/api/db/${dbId}/astrobin-export`, {
    params: { target_id: 1, include_pending: 'true', detail: 'full' },
  });
  const full = (await withPending.json()).data;
  expect(full.frames).toBe(3);
  expect(full.rows[0].number).toBe(3);
  // The fixture lights are real N.I.N.A. frames: the header read finds the
  // f-ratio, and the empty calibration library matches nothing.
  expect(full.rows[0].f_number).toBe(4.8);
  expect(full.rows[0].darks).toBe(0);
  expect(full.rows[0].flats).toBe(0);
  expect(full.csv.split('\n')[0]).toBe(
    'date,filter,number,duration,binning,gain,sensorCooling,fNumber,darks,flats,flatDarks,bias,temperature'
  );

  // The server-wide defaults fill the filter column.
  const saved = await request.put('/api/settings/astrobin', {
    data: { filter_ids: { B: 4047 } },
  });
  expect((await saved.json()).data.filter_ids).toEqual({ B: 4047 });
  const csv = await request.get(`/api/db/${dbId}/astrobin-export.csv`, {
    params: { target_id: 1, include_pending: 'true' },
  });
  expect(csv.status()).toBe(200);
  expect(csv.headers()['content-type']).toContain('text/csv');
  expect(csv.headers()['content-disposition']).toBe(
    'attachment; filename="astrobin-Alpha-M44-essentials.csv"'
  );
  expect(await csv.text()).toBe('date,filter,number,duration\n2026-04-16,4047,3,60\n');

  // The catalog's own map wins over the defaults, by night: an entry that
  // ended before the fixture's night does not apply, one covering it does.
  const map = await request.put(`/api/db/${dbId}/astrobin/filters`, {
    data: {
      entries: [
        { filter_name: 'B', astrobin_id: 100, label: 'Old B', to_night: '2026-04-15' },
        { filter_name: 'b', astrobin_id: 200, label: 'Chroma B', from_night: '2026-04-16' },
      ],
    },
  });
  expect(map.ok()).toBe(true);
  const stored = (await map.json()).data.entries;
  expect(stored.map((e: { id: number; astrobin_id: number }) => e.astrobin_id)).toEqual([100, 200]);
  expect(stored[0].id).toBeGreaterThan(0);
  const mapped = await request.get(`/api/db/${dbId}/astrobin-export`, {
    params: { target_id: 1 },
  });
  const row = (await mapped.json()).data.rows[0];
  expect(row.filter_id).toBe(200);
  expect(row.filter_label).toBe('Chroma B');

  // A bad entry is refused and leaves the map as it was.
  const refused = await request.put(`/api/db/${dbId}/astrobin/filters`, {
    data: { entries: [{ filter_name: 'B', astrobin_id: 1, from_night: 'last week' }] },
  });
  expect(refused.status()).toBe(400);
  const still = await request.get(`/api/db/${dbId}/astrobin/filters`);
  expect((await still.json()).data.entries).toEqual(stored);
});

test('the Overview opens the AstroBin dialog for a target and maps a filter from it', async ({
  page,
}) => {
  await page.goto('/');
  const alphaCard = page.locator('.project-card').filter({ hasText: 'Project Alpha' });
  await expect(alphaCard).toBeVisible({ timeout: 15_000 });
  await alphaCard.getByRole('button', { name: '☆ AstroBin' }).first().click();

  const dialog = page.locator('.astrobin-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('AstroBin acquisitions — Alpha M44');
  await expect(dialog.locator('.astrobin-table tbody tr')).toHaveCount(1);
  await expect(dialog.locator('.astrobin-table tbody tr').first()).toContainText('2026-04-16');
  await expect(dialog.locator('.astrobin-summary')).toContainText('3 frames');

  const download = dialog.getByRole('link', { name: 'Download CSV' });
  await expect(download).toHaveAttribute(
    'href',
    /\/api\/db\/astrobin-rig\/astrobin-export\.csv\?target_id=1&include_pending=true$/
  );

  await dialog.getByLabel('AstroBin id for filter B').fill('4047');
  await dialog.getByRole('button', { name: 'Save filter ids' }).click();
  await expect(dialog.locator('.astrobin-table tbody tr').first()).toContainText('#4047');
  await expect(dialog.getByLabel('AstroBin id for filter B')).toHaveCount(0);
  // The id went into this catalog's own map, open-ended.
  const map = await page.request.get(`/api/db/${dbId}/astrobin/filters`);
  expect((await map.json()).data.entries).toEqual([
    expect.objectContaining({ filter_name: 'B', astrobin_id: 4047 }),
  ]);

  await dialog.getByRole('radio', { name: /Full/ }).check();
  await expect(dialog.locator('.astrobin-table thead')).toContainText('Darks');
  await expect(download).toHaveAttribute('href', /detail=full$/);
});

test('Settings shows each database its own filter map with night ranges', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const summary = page.locator('.astrobin-filter-summary');
  await expect(summary).toContainText('none named yet');
  await summary.getByRole('button', { name: 'Edit filters' }).click();

  const dialog = page.locator('.astrobin-filter-map');
  await expect(dialog).toContainText('AstroBin filters — AstroBin Rig');
  await dialog.getByRole('button', { name: '+ Add entry' }).click();
  await dialog.getByLabel('Filter name, row 1').fill('B');
  await dialog.getByLabel('AstroBin id, row 1').fill('200');
  await dialog.getByLabel('Label, row 1').fill('Chroma B');
  await dialog.getByLabel('First night, row 1').fill('2026-04-16');
  await dialog.getByRole('button', { name: 'Save map' }).click();
  await expect(summary).toContainText('1 name, 1 entry');

  const map = await page.request.get(`/api/db/${dbId}/astrobin/filters`);
  expect((await map.json()).data.entries).toEqual([
    expect.objectContaining({
      filter_name: 'B',
      astrobin_id: 200,
      label: 'Chroma B',
      from_night: '2026-04-16',
    }),
  ]);
});
