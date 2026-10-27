import { expect, test } from '@playwright/test';
import { copyFileSync } from 'node:fs';
import path from 'node:path';
import { fixtureDbPath, fixtureImageDir, openSettings, resetDatabases, tmpBase } from './helpers';

test.beforeEach(async ({ page, request }) => {
  await page.route('**/api/director/v1/status', route => route.fulfill({ json: { success: true, error: null, data: { enabled: false, protocol_version: 1 } } }));
  await resetDatabases(request);
  for (const [slug, name] of [['c925', 'C925'], ['ultracat', 'Ultra Cat']]) {
    const file = path.join(tmpBase(), `${slug}.sqlite`);
    copyFileSync(fixtureDbPath(), file);
    const added = await request.post('/api/databases', { data: {
      slug, name, db_path: file, image_dirs: [fixtureImageDir()],
    } });
    expect(added.ok(), await added.text()).toBe(true);
  }
});

for (const width of [1280, 390]) {
  test(`database pages retain their context and guard edits at ${width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
    await page.goto('/#/?db=c925');
    await openSettings(page);
    await expect(page.getByRole('button', { name: 'Open C925', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Open Ultra Cat', exact: true })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`rig-databases-${width}.png`), animations: 'disabled' });
    await page.getByRole('button', { name: 'Open C925', exact: true }).click();
    const context = page.getByRole('navigation', { name: 'Settings for C925' });
    await expect(context.getByRole('button', { name: 'Database & files', exact: true })).toHaveAttribute('aria-current', 'page');
    await expect(context.getByRole('button', { name: 'Rig setup' })).toHaveCount(0);
    await page.getByRole('button', { name: 'Edit C925', exact: true }).click();
    await page.getByLabel('Display name (optional):').fill('C925 edited');
    await context.getByRole('button', { name: 'Sync', exact: true }).click();
    const warning = page.getByRole('alertdialog');
    await expect(warning).toContainText('Unsaved changes in database files');
    await warning.getByRole('button', { name: 'Keep editing' }).click();
    await expect(page.getByLabel('Display name (optional):')).toHaveValue('C925 edited');
    await page.getByRole('button', { name: 'Databases & rigs', exact: true }).click();
    await warning.getByRole('button', { name: 'Discard changes and switch' }).click();
    await page.getByRole('button', { name: 'Open Ultra Cat', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Ultra Cat', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'C925', exact: true })).toHaveCount(0);
    await page.screenshot({ path: testInfo.outputPath(`database-files-${width}.png`), animations: 'disabled' });
    await page.getByRole('navigation', { name: 'Settings for Ultra Cat' }).getByRole('button', { name: 'Sync', exact: true }).click();
    const transfer = page.locator('.scheduler-sync-workspace');
    await expect(transfer.getByLabel('Transfer destination')).toHaveValue('ultracat');
    await expect(transfer.getByLabel('Transfer destination')).toBeDisabled();
    await transfer.getByRole('button', { name: 'Send planning', exact: true }).click();
    await expect(transfer.getByLabel('Transfer source')).toHaveValue('ultracat');
    await expect(transfer.getByLabel('Transfer source')).toBeDisabled();
    await expect(transfer.getByLabel('Transfer destination')).toHaveValue('c925');
    const sourceBox = (await transfer.getByLabel('Transfer source').boundingBox())!;
    const destinationBox = (await transfer.getByLabel('Transfer destination').boundingBox())!;
    if (width >= 760) expect(sourceBox.width).toBeCloseTo(destinationBox.width, 0);
    else expect(destinationBox.y).toBeGreaterThan(sourceBox.y + sourceBox.height);
    await expect(page.getByLabel('Local catalog', { exact: true })).toHaveValue('ultracat');
    await expect(page.getByLabel('Local catalog', { exact: true })).toBeDisabled();
    await page.screenshot({ path: testInfo.outputPath(`rig-sync-${width}.png`), animations: 'disabled' });
    expect(await page.locator('.tauri-settings').evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    await page.getByRole('button', { name: 'Close settings' }).click();
    await expect(page).toHaveURL(/db=c925/);
  });
}

test('a shortcut opens the exact database and never falls back after removal', async ({ page, request }) => {
  await page.goto('/');
  await openSettings(page);
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('psf-guard:open-settings', {
    detail: { intent: 'sync', databaseId: 'ultracat' },
  })));
  await expect(page.getByRole('navigation', { name: 'Settings for Ultra Cat' })).toBeVisible();
  expect((await request.delete('/api/databases/ultracat')).ok()).toBe(true);
  await page.getByRole('button', { name: 'Close settings' }).click();
  await expect(page.locator('.tauri-settings')).toHaveCount(0);
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('psf-guard:open-settings', {
    detail: { intent: 'sync', databaseId: 'ultracat' },
  })));
  await expect(page.getByText(/This database is no longer registered/)).toBeVisible();
  await expect(page.locator('.scheduler-sync-workspace')).toHaveCount(0);
  await page.getByRole('button', { name: 'Databases & rigs', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Open C925', exact: true })).toBeVisible();
});
