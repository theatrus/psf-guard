import { expect, test } from '@playwright/test';

test('plans without a database survive reload, conflicts, and narrow viewports in the Library', async ({ page, request }, testInfo) => {
  const browserErrors: string[] = [];
  const scopedRequests: string[] = [];
  page.on('pageerror', error => browserErrors.push(error.message));
  page.on('request', req => { if (req.url().includes('/api/db/parked-catalog')) scopedRequests.push(req.url()); });
  // An old Planning link lands in the Library, keeping its catalog scope.
  await page.goto('/#/director?db=parked-catalog&project=123');
  await expect(page).toHaveURL(/#\/\?db=parked-catalog&project=123$/);
  // With no database the Library offers to add one; Settings opens once by itself.
  const settings = page.locator('.tauri-settings');
  await expect(settings).toBeVisible();
  await settings.getByRole('button', { name: '×' }).click();
  await expect(settings).toHaveCount(0);
  await page.waitForTimeout(1500);
  await expect(settings).toHaveCount(0);
  const plans = page.getByRole('region', { name: 'Plans with nothing captured yet' });
  await plans.getByRole('button', { name: 'New plan' }).click();
  await page.getByLabel('Plan name').fill('Multi-rig Andromeda');
  const createdResponse = page.waitForResponse(response => response.url().endsWith('/api/director/v1/projects') && response.request().method() === 'POST');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Created Multi-rig Andromeda.')).toBeVisible();
  const project = (await (await createdResponse).json()).data;

  await page.getByRole('button', { name: 'Rename Multi-rig Andromeda' }).click();
  const changed = await request.patch(`/api/director/v1/projects/${project.id}`, { data: { expected_revision: 1, name: 'Edited from another session' } });
  expect(changed.ok()).toBeTruthy();
  await page.getByLabel('Plan name').fill('Local draft');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(plans.getByRole('alert')).toContainText('conflicts');
  await expect(page.getByLabel('Plan name')).toHaveValue('Local draft');
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await plans.getByRole('button', { name: 'Refresh plans' }).click();
  await page.getByRole('button', { name: 'Rename Edited from another session' }).click();
  await page.getByLabel('Plan name').fill('Andromeda survey');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Renamed Andromeda survey.')).toBeVisible();

  await page.reload();
  await page.locator('.tauri-settings').getByRole('button', { name: '×' }).click();
  // Other specs in this run leave plans behind, so read this plan's own row.
  const survey = plans.getByTestId('plan-row').filter({ hasText: 'Andromeda survey' });
  await expect(survey).toHaveCount(1);
  await expect(survey.getByText('Not linked to any database')).toBeVisible();
  await expect(plans.getByRole('link', { name: 'Open Andromeda survey' })).toBeVisible();
  expect(new URL(page.url().split('#')[1], 'http://test').searchParams.get('db')).toBe('parked-catalog');
  await page.screenshot({ path: testInfo.outputPath('library-plans-desktop.png'), fullPage: true });
  await page.setViewportSize({ width: 375, height: 812 });
  await page.getByRole('button', { name: 'Rename Andromeda survey' }).click();
  await expect(page.getByLabel('Plan name')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy();
  await page.screenshot({ path: testInfo.outputPath('library-plans-mobile.png'), fullPage: true });
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  // Rig setup lives under Settings › Rigs; with no databases it offers to add one.
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByRole('tab', { name: 'Rigs' }).click();
  await expect(page.getByRole('heading', { name: 'Rigs' })).toBeVisible();
  await expect(page.locator('.tauri-settings').getByRole('button', { name: 'Add database' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New rig' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New site' })).toHaveCount(0);
  expect(browserErrors).toEqual([]);
  expect(scopedRequests).toEqual([]);
  expect((await (await request.get('/api/databases')).json()).data).toEqual([]);
});

test('a disabled Director has no navigation entry or editable records', async ({ page }) => {
  await page.route('**/api/director/v1/status', route => route.fulfill({ json: {
    success: true, data: { protocol_version: 1, enabled: false, instance_id: null, acquisition_available: false, database_management: true },
    error: null,
  } }));
  // An old Planning link forwards to the Library; a plan address says plans are off.
  await page.goto('/#/director');
  await expect(page).toHaveURL(/#\/$/);
  await page.goto('/#/plan?plan=anything');
  await expect(page.getByText('Plans are unavailable on this server.')).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Views' }).getByRole('group', { name: 'Plan' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /^Live rigs/ })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /New plan|New project|New site|New rig/ })).toHaveCount(0);
  // The Library has no Planning section either.
  await page.goto('/#/');
  await expect(page.getByRole('heading', { name: 'No databases configured' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Plans with nothing captured yet' })).toHaveCount(0);
});
