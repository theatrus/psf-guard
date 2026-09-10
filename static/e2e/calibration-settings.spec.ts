import { expect, test, type Page } from '@playwright/test';
import type { CalibrationSettings } from '../src/api/types';
import { registerFixtureDb, resetDatabases } from './helpers';

let original: CalibrationSettings;

async function openCalibration(page: Page) {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('tab', { name: 'Setups', exact: true }).click();
  return page.locator('.calibration-matching-settings');
}

test.beforeEach(async ({ request }) => {
  const response = await request.get('/api/settings/calibration');
  expect(response.ok()).toBeTruthy();
  original = (await response.json()).data;
  await resetDatabases(request);
  await registerFixtureDb(request);
});

test.afterEach(async ({ request }) => {
  const response = await request.put('/api/settings/calibration', {
    data: {
      rotation_tolerance_deg: original.rotation_tolerance_deg,
      external_masters: original.external_masters,
      flat_star_masking: original.flat_star_masking,
    },
  });
  expect(response.ok()).toBeTruthy();
});

test('flat star masking defaults off and persists on and off across reloads', async ({
  page, request,
}, testInfo) => {
  expect(original.flat_star_masking).toBe(false);
  await page.goto('/');
  let settings = await openCalibration(page);
  let toggle = settings.getByRole('checkbox', { name: 'Mask stars in flats' });
  await expect(toggle).not.toBeChecked();
  await expect(settings.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await toggle.check();
  const sent = page.waitForRequest((request) =>
    request.method() === 'PUT' && request.url().endsWith('/api/settings/calibration')
  );
  await settings.getByRole('button', { name: 'Save', exact: true }).click();
  expect((await sent).postDataJSON()).toEqual({
    rotation_tolerance_deg: original.rotation_tolerance_deg,
    external_masters: original.external_masters,
    flat_star_masking: true,
  });
  await expect(settings.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await expect.poll(async () =>
    (await (await request.get('/api/settings/calibration')).json()).data
  ).toEqual({
    ...original,
    flat_star_masking: true,
  });
  const legacyUpdate = await request.put('/api/settings/calibration', {
    data: {
      rotation_tolerance_deg: original.rotation_tolerance_deg,
      external_masters: original.external_masters,
    },
  });
  expect(legacyUpdate.ok()).toBeTruthy();
  expect((await legacyUpdate.json()).data.flat_star_masking).toBe(true);

  await page.reload();
  settings = await openCalibration(page);
  toggle = settings.getByRole('checkbox', { name: 'Mask stars in flats' });
  await expect(toggle).toBeChecked();
  await settings.screenshot({ path: testInfo.outputPath('flat-star-masking-desktop.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await toggle.scrollIntoViewIfNeeded();
  await expect(toggle).toBeVisible();
  expect(await settings.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  expect(await settings.getByRole('group', { name: 'Matching', exact: true })
    .locator('.review-preference > span')
    .evaluateAll((labels) => labels.every((label) =>
      label.getBoundingClientRect().width >= label.parentElement!.getBoundingClientRect().width * 0.9
    ))).toBe(true);
  await page.locator('.tauri-settings')
    .screenshot({ path: testInfo.outputPath('flat-star-masking-mobile.png') });

  await toggle.uncheck();
  await settings.getByRole('button', { name: 'Save', exact: true }).click();
  await expect.poll(async () =>
    (await (await request.get('/api/settings/calibration')).json()).data.flat_star_masking
  ).toBe(false);
  await page.reload();
  settings = await openCalibration(page);
  await expect(settings.getByRole('checkbox', { name: 'Mask stars in flats' })).not.toBeChecked();
});

test('a failed masking save keeps the draft and leaves the saved value unchanged', async ({
  page, request,
}) => {
  await page.route('**/api/settings/calibration', async (route) => {
    if (route.request().method() !== 'PUT') return route.continue();
    await route.fulfill({
      status: 503,
      json: { success: false, data: null, error: 'Could not save the registry' },
    });
  });
  await page.goto('/');
  const settings = await openCalibration(page);
  const toggle = settings.getByRole('checkbox', { name: 'Mask stars in flats' });
  await toggle.setChecked(!original.flat_star_masking);
  await settings.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(settings.getByRole('alert')).toHaveText('Could not save the registry');
  await expect(toggle).toBeChecked({ checked: !original.flat_star_masking });
  await expect(settings.getByRole('button', { name: 'Save', exact: true })).toBeEnabled();
  expect((await (await request.get('/api/settings/calibration')).json()).data.flat_star_masking)
    .toBe(original.flat_star_masking);
  await page.unroute('**/api/settings/calibration');
  await settings.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(settings.getByRole('alert')).toHaveCount(0);
  await expect.poll(async () =>
    (await (await request.get('/api/settings/calibration')).json()).data.flat_star_masking
  ).toBe(!original.flat_star_masking);
});

test('a settings load failure does not show an editable masking default', async ({ page }) => {
  await page.route('**/api/settings/calibration', async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    await route.fulfill({
      status: 503,
      json: { success: false, data: null, error: 'Registry unavailable' },
    });
  });
  await page.goto('/');
  const settings = await openCalibration(page);
  await expect(settings.getByRole('alert')).toHaveText(
    'Could not load calibration settings.', { timeout: 15_000 }
  );
  await expect(settings.getByRole('checkbox', { name: 'Mask stars in flats' })).toHaveCount(0);
  await expect(settings.getByRole('button', { name: 'Save', exact: true })).toHaveCount(0);
});
