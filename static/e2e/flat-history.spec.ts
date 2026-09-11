import { expect, request as playwrightRequest, test, type APIRequestContext } from '@playwright/test';
import { randomUUID, createHash } from 'node:crypto';
import { REVIEW_DB, REVIEW_TOKEN } from './fixtures/sync';

let review: APIRequestContext;
const headers = { Authorization: `Bearer ${REVIEW_TOKEN}` };

test.beforeAll(async () => {
  review = await playwrightRequest.newContext({ baseURL: process.env.PSF_GUARD_E2E_REVIEW_URL });
});
test.afterAll(async () => { await review.dispose(); });

test('invalidates selected flat coverage, retains decisions across stale snapshots and shows acknowledgements', async ({ page }, testInfo) => {
  const origin = randomUUID();
  const target = randomUUID();
  const profile = randomUUID();
  const source = `C925-${origin.slice(0, 8)}`;
  const scope = { protocol_version: 1, catalog_id: REVIEW_DB, origin_id: origin };
  const records = [1, 2, 3].map((id) => ({
    source_row_id: id,
    fingerprint: createHash('sha256').update(`${origin}:${id}`).digest('hex'),
    target_guid: target, target_name: 'NGC 7331', profile_id: profile,
    light_session_date: 1788998400, light_session_id: 3,
    flats_taken_date: 1789023600, flats_type: 'sky', filter_name: id === 3 ? 'Ha' : 'L',
    gain: 100, offset: 30, bin: 1, readout_mode: 0, rotation: 90, roi: 1,
  }));
  const snapshot = { ...scope, source_name: source, records };
  const posted = await review.post('/api/sync/v1/flat-history/snapshot', { headers, data: snapshot });
  expect(posted.status(), await posted.text()).toBe(200);
  const listed = await review.get(`/api/db/${REVIEW_DB}/flat-history`, { params: { q: source } });
  expect((await listed.json()).data.total).toBe(3);

  await page.goto(`${process.env.PSF_GUARD_E2E_REVIEW_URL}/`);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('tab', { name: 'Databases', exact: true }).click();
  await page.locator('.db-entry').filter({ has: page.locator('.db-row-slug', { hasText: REVIEW_DB }) })
    .getByRole('button', { name: 'Manage', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Calibration library' });
  await dialog.getByRole('tab', { name: 'Scheduler flats', exact: true }).click();
  await dialog.getByLabel('Target, source or filter').fill(source);
  await dialog.getByRole('button', { name: 'Search', exact: true }).click();
  await expect(dialog.locator('tbody tr')).toHaveCount(3);
  await dialog.getByRole('checkbox', { name: 'Select coverage 1 for NGC 7331', exact: true }).check();
  await dialog.getByRole('checkbox', { name: 'Select coverage 2 for NGC 7331', exact: true }).check();
  await expect(dialog.getByRole('button', { name: 'Invalidate coverage', exact: true })).toBeDisabled();
  await dialog.getByLabel('Reason', { exact: true }).fill('Bright-star artifacts in the luminance sky flats');
  expect(await dialog.getByRole('button', { name: 'Refresh coverage', exact: true }).locator('svg')
    .evaluate((el) => el.getBoundingClientRect().width)).toBeGreaterThanOrEqual(16);
  await dialog.screenshot({ path: testInfo.outputPath('flat-history-desktop.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(dialog.getByRole('button', { name: 'Invalidate coverage', exact: true })).toBeVisible();
  expect(await dialog.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
  await dialog.screenshot({ path: testInfo.outputPath('flat-history-mobile.png') });
  await dialog.getByRole('button', { name: 'Invalidate coverage', exact: true }).click();
  await expect(dialog.getByRole('status')).toHaveText('2 coverage records awaiting sync.');
  await expect(dialog.locator('tbody tr')).toHaveCount(1);

  // A stale plugin snapshot must not revive the explicitly invalidated coverage.
  expect((await review.post('/api/sync/v1/flat-history/snapshot', { headers, data: snapshot })).ok()).toBe(true);
  const pending = await review.post('/api/sync/v1/flat-history/pending', { headers, data: scope });
  const decisions = (await pending.json()).data.decisions as Array<{
    record_id: string; source_row_id: number; fingerprint: string;
  }>;
  expect(decisions.map((d) => d.source_row_id).sort()).toEqual([1, 2]);
  const ack = await review.post('/api/sync/v1/flat-history/acknowledge', {
    headers, data: { ...scope, results: decisions.map((d) => ({
      ...d, status: d.source_row_id === 1 ? 'removed' : 'conflict',
      detail: d.source_row_id === 1 ? null : 'Source coverage changed; new flats preserved.',
    })) },
  });
  expect(ack.status(), await ack.text()).toBe(200);
  await page.setViewportSize({ width: 1280, height: 900 });
  await dialog.getByLabel('Coverage status', { exact: true }).selectOption('all');
  await expect(dialog.locator('tbody tr')).toHaveCount(3);
  await expect(dialog.locator('tbody').getByText('Coverage removed', { exact: true })).toBeVisible();
  await expect(dialog.locator('tbody').getByText('Changed in NINA', { exact: true })).toBeVisible();
  await expect(dialog.getByText('Source coverage changed; new flats preserved.', { exact: true })).toBeVisible();
  const remaining = await review.post('/api/sync/v1/flat-history/pending', { headers, data: scope });
  expect((await remaining.json()).data.decisions).toEqual([]);
});
