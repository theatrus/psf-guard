import { expect, test } from '@playwright/test';
import highlights from '../src/updates/highlights.json' with { type: 'json' };
import { registerFixtureDb, resetDatabases, stubPixelWork, waitForCacheReady } from './helpers';

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

test('an upgrade shows the new notes in priority order', async ({ page }) => {
  await page.evaluate(({ key, version }) => localStorage.setItem(key, version), { key, version: highlights[1].version });
  await page.reload();
  const dialog = page.getByRole('dialog', { name: `What's new in PSF Guard v${version}` });
  await expect(dialog).toBeVisible();
  const expectedSections = [
    highlights[0].deprecations.length ? 'Deprecations' : null,
    highlights[0].upgradeNotes.length ? 'Upgrade notes' : null,
    highlights[0].highlights.length ? 'New features' : null,
  ].filter(Boolean);
  expect(await dialog.getByRole('heading', { level: 3 }).allTextContents()).toEqual(expectedSections);
  for (const deprecation of highlights[0].deprecations) {
    await expect(dialog.getByText(deprecation.title, { exact: true })).toBeVisible();
  }
  await expect(dialog.getByRole('link', {
    name: `Full release notes for v${highlights[1].version}`,
  })).toHaveCount(0);
  await expect(dialog.getByRole('link', { name: `Full release notes for v${version}` })).toHaveAttribute('href', `https://github.com/theatrus/psf-guard/releases/tag/v${version}`);
});

test('reading the popup does not grade, navigate or close the image behind it', async ({ page, request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, { name: 'Release notes test', slug: 'release-notes-test' });
  await waitForCacheReady(request, entry.id);
  await stubPixelWork(page);
  const grades: string[] = [];
  await page.route('**/images/grade', async route => {
    grades.push(route.request().postData() ?? '');
    await route.fulfill({ status: 500, body: 'The popup must block grading shortcuts' });
  });
  await page.goto(`/#/detail/1?db=${encodeURIComponent(entry.id)}&project=1`);
  const dialog = page.getByRole('dialog', { name: `What's new in PSF Guard v${version}` });
  await expect(dialog).toBeVisible();
  await expect(page.locator('.detail-info')).toBeVisible();
  const url = page.url();
  await page.keyboard.press('a');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(page).toHaveURL(url);
  expect(grades).toEqual([]);
  await resetDatabases(request);
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
