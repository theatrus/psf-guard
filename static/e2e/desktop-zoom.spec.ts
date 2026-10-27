import { expect, test } from '@playwright/test';
import { openSettings, registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

// The native zoom call is stubbed here; CSS zoom makes its visual effect
// visible in browser captures. Unit tests cover native failures and ordering.
test('desktop UI zoom works from the menu and shortcuts, including Settings, and survives reload', async ({ page, request }, testInfo) => {
  await resetDatabases(request);
  const catalog = await registerFixtureDb(request, { name: 'Zoom test catalog', slug: 'ui-zoom-test' });
  await waitForCacheReady(request, catalog.id);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.addInitScript(() => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      invoke: async (command: string, args?: { value?: number }) => {
        switch (command) {
          case 'get_desktop_server_settings': return { server_url: location.origin, starting: false, error: null };
          case 'get_server_url': return location.origin;
          case 'is_configuration_valid': return true;
          case 'get_current_configuration': return { schema_version: 2, databases: [] };
          case 'get_log_folder': return null;
          case 'plugin:updater|check': return null;
          case 'plugin:webview|set_webview_zoom':
            document.documentElement.style.zoom = String(args!.value);
            return;
          default: throw new Error(`Unmocked desktop command: ${command}`);
        }
      },
    } });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Menu', exact: true }).click();
  await expect(page.getByText('UI zoom: 100%')).toBeVisible();
  await page.getByRole('menuitem', { name: 'Zoom in', exact: true }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem('psf-guard.ui-zoom-percent'))).toBe('110');
  await page.getByRole('button', { name: 'Menu', exact: true }).click();
  await expect(page.getByText('UI zoom: 110%')).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('desktop-ui-zoom-menu.png') });
  await page.keyboard.press('Escape');
  await openSettings(page);
  const settings = page.locator('.tauri-settings');
  await expect(settings).toBeVisible();
  await page.keyboard.press('Control+=');
  await expect.poll(() => page.evaluate(() => localStorage.getItem('psf-guard.ui-zoom-percent'))).toBe('120');
  await expect(settings).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('desktop-ui-zoom-settings.png') });
  await page.keyboard.press('Control+0');
  await expect.poll(() => page.evaluate(() => localStorage.getItem('psf-guard.ui-zoom-percent'))).toBe('100');
  await page.getByRole('button', { name: 'Close settings', exact: true }).click();
  await page.keyboard.press('Meta+-');
  await expect.poll(() => page.evaluate(() => localStorage.getItem('psf-guard.ui-zoom-percent'))).toBe('90');
  await page.reload();
  await expect.poll(() => page.evaluate(() => document.documentElement.style.zoom)).toBe('0.9');
  await page.getByRole('button', { name: 'Menu', exact: true }).click();
  await expect(page.getByText('UI zoom: 90%')).toBeVisible();
  await page.getByRole('menuitem', { name: 'Reset to 100%', exact: true }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem('psf-guard.ui-zoom-percent'))).toBe('100');
});

test('browser mode keeps browser zoom and hides desktop zoom controls', async ({ page }) => {
  await page.route('**/api/info', async route => {
    const response = await route.fetch();
    const body = await response.json();
    await route.fulfill({ response, json: { ...body, data: { ...body.data, allow_database_management: false } } });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Menu', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Zoom in', exact: true })).toHaveCount(0);
  await expect(page.getByText(/UI zoom:/)).toHaveCount(0);
});
