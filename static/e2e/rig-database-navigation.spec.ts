import { expect, test } from '@playwright/test';
import { copyFileSync } from 'node:fs';
import path from 'node:path';
import { fixtureDbPath, fixtureImageDir, openSettings, resetDatabases, tmpBase, selectSettingsSection } from './helpers';

test.beforeEach(async ({ page, request }) => {
  await page.route('**/api/director/v1/status', route => route.fulfill({ json: { success: true, error: null, data: { enabled: false, protocol_version: 1 } } }));
  await resetDatabases(request);
  const defaults = await request.put('/api/settings/processing', { data: { values: { analyze_new_frames: false, pair_calibrated_copies: true, scan_calibrated_copies: false } } });
  expect(defaults.ok(), await defaults.text()).toBe(true);
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
    await selectSettingsSection(page, 'databases');
    await expect(page.locator('.settings-rig-database-pages')).toHaveCount(0);
    await expect(page.getByRole('tab', { name: 'Rig setup' })).toHaveCount(0);
    await page.getByRole('button', { name: 'Edit C925', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Automatically import new frames' }).check();
    const automaticImport = page.getByRole('group', { name: 'Automatic import', exact: true });
    await expect(automaticImport.getByRole('checkbox', { name: 'Analyze quality of the new frames in the background' })).toBeVisible();
    await expect(automaticImport.getByLabel('Import:', { exact: true })).toHaveValue('all');
    await automaticImport.evaluate(element => {
      const scroller = element.closest('.modal-body')!;
      scroller.scrollTop += element.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    });
    await page.screenshot({ path: testInfo.outputPath(`import-settings-${width}.png`), animations: 'disabled' });
    await page.getByLabel('Display name (optional):').fill('C925 edited');
    await page.getByRole('button', { name: 'Quality analysis', exact: true }).click();
    await expect(page.getByRole('alertdialog')).toContainText('Unsaved changes in database files');
    await page.getByRole('button', { name: 'Keep editing' }).click();
    await selectSettingsSection(page, 'sync');
    const warning = page.getByRole('alertdialog');
    await expect(warning).toContainText('Unsaved changes in database files');
    await warning.getByRole('button', { name: 'Keep editing' }).click();
    await expect(page.getByLabel('Display name (optional):')).toHaveValue('C925 edited');
    await page.getByRole('button', { name: 'Databases & rigs', exact: true }).click();
    await warning.getByRole('button', { name: 'Discard changes and switch' }).click();
    await page.getByRole('button', { name: 'Open Ultra Cat', exact: true }).click();
    await selectSettingsSection(page, 'databases');
    await expect(page.getByRole('region', { name: 'Ultra Cat', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'C925', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Open Calibration', exact: true })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`database-files-${width}.png`), animations: 'disabled' });
    await expect(page.getByRole('checkbox', { name: 'Analyze new frames as they arrive' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Edit filters', exact: true })).toHaveCount(0);
    await selectSettingsSection(page, 'sync');
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
    await expect(page.getByLabel('Local database', { exact: true })).toHaveValue('ultracat');
    await expect(page.getByLabel('Local database', { exact: true })).toBeDisabled();
    await page.screenshot({ path: testInfo.outputPath(`rig-sync-${width}.png`), animations: 'disabled' });
    expect(await page.locator('.tauri-settings').evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    await selectSettingsSection(page, 'database-add');
    await expect(page.getByRole('heading', { name: 'Create or import a database' })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`new-import-${width}.png`), animations: 'disabled' });
    await selectSettingsSection(page, 'review');
    const quality = page.getByRole('region', { name: 'Quality analysis', exact: true });
    await expect(quality.getByRole('region', { name: 'Global settings' })).toBeVisible();
    await quality.getByRole('button', { name: /^C925.*Uses global settings$/ }).click();
    await quality.evaluate(element => {
      const scroller = element.closest('.modal-body')!;
      scroller.scrollTop += element.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    });
    await expect(page.getByRole('button', { name: 'Close settings' })).toBeInViewport();
    await page.screenshot({ path: testInfo.outputPath(`quality-settings-${width}.png`), animations: 'disabled' });
    await selectSettingsSection(page, 'calibration');
    await expect(page.getByRole('region', { name: 'Calibrated copies', exact: true })).toBeVisible();
    await expect(page.getByRole('group', { name: 'Matching', exact: true })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`pairing-settings-${width}.png`), animations: 'disabled' });
    await selectSettingsSection(page, 'astrobin');
    await expect(page.getByRole('region', { name: 'C925 filter map' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'AstroBin', exact: true })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`filter-settings-${width}.png`), animations: 'disabled' });
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
  await expect(page.getByRole('navigation', { name: 'Rig database location' })).toBeVisible();
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

test('shared pairing defaults respect explicit database values and reset to inheritance', async ({ page, request }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto('/');
  await openSettings(page);
  await selectSettingsSection(page, 'calibration');
  const pairing = page.getByRole('region', { name: 'Calibrated copies', exact: true });
  const shared = pairing.getByRole('region', { name: 'Global settings', exact: true });
  const field = 'Pair calibrated and registered copies with their lights';
  await shared.getByRole('combobox', { name: field }).selectOption('off');
  await shared.getByRole('button', { name: 'Save global settings' }).click();
  await expect(shared.getByRole('button', { name: 'Save global settings' })).toBeDisabled();
  await pairing.getByRole('button', { name: /^C925.*Uses global settings$/ }).click();
  const own = pairing.getByRole('region', { name: 'C925 overrides', exact: true });
  const override = own.getByRole('button', { name: `${field}: override global setting`, exact: true });
  await override.click();
  await own.getByRole('combobox', { name: field }).selectOption('on');
  await selectSettingsSection(page, 'review');
  await expect(page.getByRole('alertdialog')).toContainText('C925 processing');
  await page.getByRole('button', { name: 'Keep editing' }).click();
  await own.getByRole('button', { name: 'Save database' }).click();
  await expect(own.getByRole('button', { name: 'Save database' })).toBeDisabled();
  let policies = (await (await request.get('/api/settings/processing')).json()).data;
  expect(policies.databases.find((entry: { id: string }) => entry.id === 'c925').effective.pair_calibrated_copies).toBe(true);
  expect(policies.databases.find((entry: { id: string }) => entry.id === 'ultracat').effective.pair_calibrated_copies).toBe(false);
  await page.locator('.tauri-settings .modal-body').evaluate(element => { element.scrollTop = 0; });
  await page.screenshot({ path: testInfo.outputPath('pairing-override-desktop.png'), animations: 'disabled' });
  await page.setViewportSize({ width: 390, height: 844 });
  await own.evaluate(element => {
    const item = element.closest('.calibration-rig-item')!;
    const scroller = element.closest('.modal-body')!;
    scroller.scrollTop += item.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
  });
  expect(await own.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath('pairing-override-phone.png'), animations: 'disabled' });
  await override.click();
  await own.getByRole('button', { name: 'Save database' }).click();
  await expect(pairing.getByRole('button', { name: /^C925.*Uses global settings$/ })).toBeVisible();
  policies = (await (await request.get('/api/settings/processing')).json()).data;
  expect(policies.databases.find((entry: { id: string }) => entry.id === 'c925').overrides.pair_calibrated_copies).toBeNull();
  expect(policies.databases.find((entry: { id: string }) => entry.id === 'c925').effective.pair_calibrated_copies).toBe(false);
});
