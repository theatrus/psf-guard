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
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Created Multi-rig Andromeda.')).toBeVisible();
  const projects = await (await request.get('/api/director/v1/projects')).json();
  expect(projects.data.items).toHaveLength(1);
  const project = projects.data.items[0];

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

  for (const [tab, kind, name] of [['Sites', 'site', 'Starfront observatory'], ['Rigs', 'rig', 'C925 remote telescope']] as const) {
    await page.getByRole('navigation', { name: 'Director views' }).getByRole('button', { name: tab }).click();
    await page.getByRole('button', { name: `New ${kind}` }).click();
    await page.getByLabel(`${tab.slice(0, -1)} name`).fill(name);
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText(`Created ${name}.`)).toBeVisible();
    await page.reload();
    await expect(page.getByText(name, { exact: true })).toBeVisible();
    expect(new URL(page.url().split('#')[1], 'http://test').searchParams.get('db')).toBe('parked-catalog');
  }
  await page.screenshot({ path: testInfo.outputPath('director-desktop.png'), fullPage: true });
  await page.setViewportSize({ width: 375, height: 812 });
  await page.getByRole('button', { name: 'Rename C925 remote telescope' }).click();
  await expect(page.getByLabel('Rig name')).toBeVisible();
  expect(await page.locator('.director-page').evaluate(element => element.scrollWidth <= element.clientWidth)).toBeTruthy();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy();
  await page.screenshot({ path: testInfo.outputPath('director-mobile.png'), fullPage: true });
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
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
