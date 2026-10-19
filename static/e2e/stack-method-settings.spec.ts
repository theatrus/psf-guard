import { expect, test } from '@playwright/test';
import { openSettings, registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

// Settings › Stacking › Stacking method: the orientation group margin saves
// when the field loses focus, refuses a value past the limit, and is off
// while the best-graded frame is the reference.

const METHOD_URL = '/api/settings/stacking/method';

test.beforeEach(async ({ request }) => {
  // With no catalog the app opens Settings on its own.
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, { name: 'Imaging Rig e2e', slug: 'imaging-rig-e2e' });
  await waitForCacheReady(request, entry.id);
});

test.afterEach(async ({ request }) => {
  // Other specs stack with the recommended method.
  const current = await (await request.get(METHOD_URL)).json();
  await request.put(METHOD_URL, { data: current.data.recommended });
});

test('the orientation group margin saves on blur and follows the reference choice', async ({ page, request }, testInfo) => {
  await page.goto('/');
  await openSettings(page);
  await page.getByRole('tab', { name: 'Stacking' }).click();
  const margin = page.getByRole('spinbutton', { name: /Degrees, 0 to 5/ });
  await expect(margin).toHaveValue('1');
  await margin.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('stacking-method.png') });

  await margin.fill('2.5');
  await margin.blur();
  await expect(page.getByText('Now: Custom')).toBeVisible();
  const saved = await (await request.get(METHOD_URL)).json();
  expect(saved.data.method.reference_rotation_margin_degrees).toBe(2.5);

  // Past the limit: nothing is saved and the field shows the saved margin.
  await margin.fill('7');
  await margin.press('Enter');
  await expect(margin).toHaveValue('2.5');

  // The radio shows the saved method, so it turns on once the save returns.
  const bestGraded = page.getByRole('radio', { name: /^Best graded/ });
  await bestGraded.click();
  await expect(bestGraded).toBeChecked();
  await expect(margin).toBeDisabled();
  await page.screenshot({ path: testInfo.outputPath('stacking-method-best-graded.png') });
});
