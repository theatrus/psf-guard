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

  // The settings map fills the filter column.
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

  await dialog.getByRole('radio', { name: /Full/ }).check();
  await expect(dialog.locator('.astrobin-table thead')).toContainText('Darks');
  await expect(download).toHaveAttribute('href', /detail=full$/);
});
