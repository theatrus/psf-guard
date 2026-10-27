import { expect, test, type Page } from '@playwright/test';
import { openSettings, registerFixtureDb, resetDatabases, selectSettingsSection, waitForCacheReady } from './helpers';

async function expectCompactHeader(page: Page) {
  const actions = page.getByRole('group', { name: 'Settings actions' });
  await expect(actions.getByRole('button', { name: 'Close settings' })).toBeInViewport();
  await expect(page.locator('.tauri-settings .modal-footer')).toHaveCount(0);
  await expect(page.getByRole('button', { name: /^(Done|Reload Page|Restart Server)$/i })).toHaveCount(0);
  const header = await page.locator('.tauri-settings .modal-header').boundingBox();
  expect(header!.height).toBeLessThan(80);
  const content = await page.getByRole('tabpanel').boundingBox();
  expect(content!.y + content!.height).toBeCloseTo(page.viewportSize()!.height, 0);
}

test.beforeEach(async ({ page, request }) => {
  await resetDatabases(request);
  const db = await registerFixtureDb(request, { name: 'Navigation fixture', slug: 'settings-navigation' });
  await waitForCacheReady(request, db.id);
  // Show Planning's navigation without pairing equipment or changing server settings.
  await page.route('**/api/director/v1/status', route => route.fulfill({ json: {
    success: true, error: null,
    data: { protocol_version: 1, enabled: true, instance_id: '11111111-1111-4111-8111-111111111111', database_management: true },
  } }));
  await page.route('**/api/director/v1/templates', route => route.fulfill({ json: { success: true, error: null, data: [] } }));
});

test('desktop groups related settings, moves GPU and presets, and keeps section shortcuts', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto('/');
  await openSettings(page);
  await expectCompactHeader(page);
  const nav = page.getByRole('tablist', { name: 'Settings sections' });
  await expect(nav).toHaveAttribute('aria-orientation', 'vertical');
  await expect(page.getByRole('tab', { name: 'Rigs', exact: true })).toHaveAccessibleDescription('Databases & rigs');
  await expect(page.getByRole('tab', { name: 'Exposure templates' })).toHaveAccessibleDescription('Presets');
  await expect(page.getByLabel('Settings section', { exact: true })).toBeHidden();

  await selectSettingsSection(page, 'stacking');
  await expect(page.getByRole('heading', { name: 'Stacking method' })).toBeVisible();
  await expect(page.getByRole('checkbox', { name: /^Use the GPU/ })).toHaveCount(0);
  await selectSettingsSection(page, 'gpu');
  await expect(page.getByRole('checkbox', { name: /^Use the GPU/ })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'GPU', exact: true })).toHaveAccessibleDescription('Performance');
  await page.screenshot({ path: testInfo.outputPath('settings-performance-desktop.png'), animations: 'disabled' });

  await selectSettingsSection(page, 'setups');
  await expect(page.locator('.processing-setups-manager')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'PixInsight' })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'AstroBin' })).toHaveCount(0);
  await selectSettingsSection(page, 'templates');
  await expect(page.getByRole('region', { name: 'Exposure template library' })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('settings-presets-desktop.png'), animations: 'disabled' });

  // Existing callers still use stable section IDs, including a catalog package to focus.
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('psf-guard:open-settings', {
    detail: { intent: 'catalogs', field: 'gaia_photometry' },
  })));
  const catalogs = page.getByRole('tab', { name: 'Reference catalogs' });
  await expect(catalogs).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('region', { name: 'Gaia colors' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Gaia colors' })).toBeInViewport();
  await catalogs.press('Home');
  await expect(page.getByRole('tab', { name: 'Databases', exact: true })).toBeFocused();
  await page.getByRole('tab', { name: 'Databases', exact: true }).press('ArrowDown');
  await expect(page.getByRole('tab', { name: 'Rigs', exact: true })).toBeFocused();
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('psf-guard:open-settings', {
    detail: { intent: 'astrobin' },
  })));
  const astrobin = page.getByRole('tab', { name: 'AstroBin' });
  await expect(astrobin).toHaveAttribute('aria-selected', 'true');
  await expect(astrobin).toBeInViewport();
});

test('phone uses a compact grouped chooser for every settings destination', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await openSettings(page);
  await expectCompactHeader(page);
  const chooser = page.getByLabel('Settings section', { exact: true });
  await expect(chooser).toBeVisible();
  await expect(page.getByRole('tablist', { name: 'Settings sections' })).toBeHidden();
  await expect(chooser.locator('optgroup[label="Presets"] option')).toHaveText(['Processing setups', 'Exposure templates']);

  await selectSettingsSection(page, 'gpu');
  await expect(chooser).toHaveValue('gpu');
  await expect(page.getByRole('checkbox', { name: /^Use the GPU/ })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('settings-performance-phone.png'), animations: 'disabled' });
  await selectSettingsSection(page, 'templates');
  await expect(page.getByRole('region', { name: 'Exposure template library' })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('settings-presets-phone.png'), animations: 'disabled' });
  await selectSettingsSection(page, 'pixinsight');
  await expect(page.getByRole('heading', { name: 'PixInsight' })).toBeVisible();
  await selectSettingsSection(page, 'storage');
  await expect(page.getByRole('heading', { name: 'Disk use' })).toBeVisible();
  const folder = page.getByRole('textbox', { name: 'WBPP storage folder' });
  const savedFolder = await folder.inputValue();
  await folder.fill('/tmp/settings-navigation-unsaved');
  await selectSettingsSection(page, 'gpu');
  const warning = page.getByRole('alertdialog');
  await expect(warning).toContainText('Switch to GPU without saving them?');
  await expect(chooser).toHaveValue('storage');
  await warning.getByRole('button', { name: 'Keep editing' }).click();
  await expect(folder).toHaveValue('/tmp/settings-navigation-unsaved');
  await page.getByRole('button', { name: 'Close settings' }).click();
  await expect(warning).toContainText('Close without saving them?');
  await expect(warning).toBeInViewport();
  await page.screenshot({ path: testInfo.outputPath('settings-unsaved-phone.png'), animations: 'disabled' });
  await warning.getByRole('button', { name: 'Keep editing' }).click();
  await selectSettingsSection(page, 'gpu');
  await warning.getByRole('button', { name: 'Discard changes and switch' }).click();
  await expect(chooser).toHaveValue('gpu');
  await selectSettingsSection(page, 'storage');
  await expect(folder).toHaveValue(savedFolder);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.documentElement.scrollHeight)).toBeLessThanOrEqual(844);
});
