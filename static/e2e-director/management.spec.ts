import { expect, test } from '@playwright/test';

test('global Director identities survive reload, conflicts, and narrow viewports', async ({ page, request }, testInfo) => {
  const browserErrors: string[] = [];
  const scopedRequests: string[] = [];
  page.on('pageerror', error => browserErrors.push(error.message));
  page.on('request', req => { if (req.url().includes('/api/db/parked-catalog')) scopedRequests.push(req.url()); });
  await page.goto('/#/director?db=parked-catalog&project=123');
  await expect(page.getByRole('heading', { name: 'Director', exact: true })).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByText('Acquisition is not yet available.')).toBeVisible();
  await page.getByRole('button', { name: 'New project' }).click();
  await page.getByLabel('Project name').fill('Multi-rig Andromeda');
  const createdResponse = page.waitForResponse(response => response.url().endsWith('/api/director/v1/projects') && response.request().method() === 'POST');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Created Multi-rig Andromeda.')).toBeVisible();
  const project = (await (await createdResponse).json()).data;

  await page.getByRole('button', { name: 'Rename Multi-rig Andromeda' }).click();
  const changed = await request.patch(`/api/director/v1/projects/${project.id}`, { data: { expected_revision: 1, name: 'Edited from another session' } });
  expect(changed.ok()).toBeTruthy();
  await page.getByLabel('Project name').fill('Local draft');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('conflicts');
  await expect(page.getByLabel('Project name')).toHaveValue('Local draft');
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.getByRole('button', { name: 'Refresh records' }).click();
  await page.getByRole('button', { name: 'Rename Edited from another session' }).click();
  await page.getByLabel('Project name').fill('Andromeda survey');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Renamed Andromeda survey.')).toBeVisible();

  await page.reload();
  await expect(page.getByText('Andromeda survey', { exact: true })).toBeVisible();
  await expect(page.locator('.director-plan', { hasText: 'Andromeda survey' }).getByText('Not linked to any database')).toBeVisible();
  expect(new URL(page.url().split('#')[1], 'http://test').searchParams.get('db')).toBe('parked-catalog');
  await page.screenshot({ path: testInfo.outputPath('director-desktop.png'), fullPage: true });
  await page.setViewportSize({ width: 375, height: 812 });
  await page.getByRole('button', { name: 'Rename Andromeda survey' }).click();
  await expect(page.getByLabel('Project name')).toBeVisible();
  expect(await page.locator('.director-page').evaluate(element => element.scrollWidth <= element.clientWidth)).toBeTruthy();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy();
  await page.screenshot({ path: testInfo.outputPath('director-mobile.png'), fullPage: true });
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  // Rigs are a section of the same page; with no databases it offers to add one.
  await expect(page.getByRole('heading', { name: 'Rigs' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Add database' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New rig' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New site' })).toHaveCount(0);
  expect(browserErrors).toEqual([]);
  expect(scopedRequests).toEqual([]);
  expect((await (await request.get('/api/databases')).json()).data).toEqual([]);
});

test('a disabled Director has no navigation entry or editable records', async ({ page }) => {
  await page.route('**/api/director/v1/status', route => route.fulfill({ json: {
    success: true, data: { protocol_version: 1, enabled: false, instance_id: null, acquisition_available: false }, error: null,
  } }));
  await page.goto('/#/director');
  await expect(page.getByText('Director management is unavailable on this server.')).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: 'Director' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /New project|New site|New rig/ })).toHaveCount(0);
});
