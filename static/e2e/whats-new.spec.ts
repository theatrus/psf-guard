import { expect, test } from '@playwright/test';
import highlights from '../src/updates/highlights.json' with { type: 'json' };

const version = highlights[0].version;
const key = 'psf-guard.whats-new.acknowledged';

test.beforeEach(async ({ page }) => {
  // No catalog operations: exercise the app shell with the installed version.
  await page.route('**/api/info', async route => {
    const response = await route.fetch();
    const body = await response.json();
    await route.fulfill({ response, json: { ...body, data: { ...body.data, version, allow_database_management: false } } });
  });
  await page.goto('/');
  await page.evaluate(key => localStorage.removeItem(key), key);
  await page.reload();
});

test('close for now, hide this version, then reopen from the menu', async ({ page }) => {
  const dialog = page.getByRole('dialog', { name: `What's new in PSF Guard v${version}` });
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await page.reload();
  await expect(dialog).toBeVisible();
  await dialog.getByRole('checkbox', { name: "Don't show again for this version" }).check();
  await dialog.getByRole('button', { name: 'Done' }).click();
  await expect(dialog).toBeHidden();
  await page.reload();
  await expect(page.getByRole('button', { name: 'Menu', exact: true })).toBeVisible();
  await expect(dialog).toBeHidden();
  await page.getByRole('button', { name: 'Menu', exact: true }).click();
  await page.getByRole('menuitem', { name: "What's new" }).click();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('checkbox')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Menu', exact: true })).toBeFocused();
});

test('an earlier acknowledgment still shows the new version', async ({ page }) => {
  await page.evaluate(key => localStorage.setItem(key, '0.1.0'), key);
  await page.reload();
  const dialog = page.getByRole('dialog', { name: `What's new in PSF Guard v${version}` });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('heading', { name: 'Upgrade notes' })).toBeVisible();
  await expect(dialog.getByRole('link', { name: `Full release notes for v${version}` })).toHaveAttribute('href', `https://github.com/theatrus/psf-guard/releases/tag/v${version}`);
});

test('notes scroll while dismiss controls stay visible on a narrow screen', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 700 });
  const dialog = page.getByRole('dialog', { name: `What's new in PSF Guard v${version}` });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Done' })).toBeInViewport();
  await expect(dialog.getByRole('checkbox')).toBeInViewport();
  expect(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await dialog.getByRole('link', { name: `Full release notes for v${version}` }).scrollIntoViewIfNeeded();
  await expect(dialog.getByRole('button', { name: 'Done' })).toBeInViewport();
  await dialog.locator('.dialog-body').evaluate(element => { element.scrollTop = 0; });
  await page.screenshot({ path: testInfo.outputPath('whats-new-mobile.png') });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.screenshot({ path: testInfo.outputPath('whats-new-desktop.png') });
});
