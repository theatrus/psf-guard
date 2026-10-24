import { expect, test } from '@playwright/test';
import { openSettings, registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

// Settings › Stacking › Stacking method: the orientation group margin saves
// when the field loses focus, refuses a value past the limit, and is off
// while the best-graded frame is the reference. Settings › Stacking › GPU:
// turning the GPU off saves and disables its passes, and Restore defaults
// turns every pass back on.

const METHOD_URL = '/api/settings/stacking/method';
const GPU_URL = '/api/settings/stacking/gpu';

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
  const gpu = await (await request.get(GPU_URL)).json();
  await request.put(GPU_URL, { data: gpu.data.recommended });
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

test('turning the GPU off saves, disables its passes, and the defaults come back', async ({ page, request }, testInfo) => {
  await page.goto('/');
  await openSettings(page);
  await page.getByRole('tab', { name: 'Stacking' }).click();
  const useGpu = page.getByRole('checkbox', { name: /^Use the GPU/ });
  await expect(useGpu).toBeChecked();
  const resampling = page.getByRole('checkbox', { name: /^Resampling/ });
  await expect(resampling).toBeEnabled();
  await useGpu.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('stacking-gpu.png') });

  await useGpu.click();
  await expect(useGpu).not.toBeChecked();
  await expect(resampling).toBeDisabled();
  const saved = await (await request.get(GPU_URL)).json();
  expect(saved.data.gpu.enabled).toBe(false);
  await page.screenshot({ path: testInfo.outputPath('stacking-gpu-off.png') });

  await page.getByRole('button', { name: 'Restore defaults' }).click();
  await expect(useGpu).toBeChecked();
  await expect(resampling).toBeEnabled();
  const restored = await (await request.get(GPU_URL)).json();
  expect(restored.data.gpu).toEqual(restored.data.recommended);
});
