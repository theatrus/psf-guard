import { expect, test } from '@playwright/test';
import { openSettings, registerFixtureDb, resetDatabases, waitForCacheReady, selectSettingsSection } from './helpers';

// Settings covers the page and is its only scroller. The page behind scrolls
// in .app-main, so the document itself must never be taller than the window:
// otherwise a second scrollbar shows beside Settings and the wheel moves the
// page behind it. A built stack's open measurements table has a visually
// hidden caption, which once made the document taller on the Stacks page.

let dbId: string;

test.beforeEach(async ({ request }, testInfo) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, {
    name: 'Settings Scroll e2e',
    // Removing a registry entry preserves its cache; each test needs its own slug.
    slug: `settings-scroll-e2e-${testInfo.project.name}-${testInfo.line}-${testInfo.repeatEachIndex}-${testInfo.retry}`,
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

test('Settings is the only scroller over the Stacks page, which keeps its place', async ({ page }, testInfo) => {
  test.setTimeout(300_000);
  await page.setViewportSize({ width: 1280, height: 800 });
  // Only the stack is needed; keep the image-preview queue out of its way.
  await page.route('**/images/*/preview?*', (route) => route.abort());
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=1`);

  const panel = page.locator('.stack-preview-panel');
  await expect(panel).toBeVisible({ timeout: 15_000 });
  await panel.getByRole('button', { name: 'Build channel', exact: true }).click();
  await expect(panel.locator('.stack-preview-results')).toHaveAttribute('data-job-state', 'completed', {
    timeout: 210_000,
  });

  const measurements = panel.locator('.stack-snr-data');
  await measurements.getByText(/^Exact measurements/).click();
  await expect(measurements).toHaveAttribute('open', '');
  await measurements.evaluate((element) => element.scrollIntoView({ block: 'center' }));

  const documentFits = () => page.evaluate(() =>
    document.documentElement.scrollHeight <= window.innerHeight);
  const mainScrollTop = () => page.locator('.app-main').evaluate((main) => main.scrollTop);
  const pagePlace = await mainScrollTop();
  expect(pagePlace).toBeGreaterThan(0);

  await openSettings(page);
  const settings = page.locator('.tauri-settings .modal-content');
  await expect(settings).toBeVisible();
  // The database page can now fit without scrolling. Use a long settings page
  // so this still checks wheel containment after removing the footer.
  await selectSettingsSection(page, 'calibration');
  await expect(page.getByRole('heading', { name: 'Calibration', exact: true })).toBeVisible();
  await expect.poll(() => page.getByRole('tabpanel').evaluate(element =>
    element.scrollHeight - element.clientHeight)).toBeGreaterThan(0);
  // Wheel past the end of Settings. The window must not scroll the page behind.
  await page.mouse.move(640, 400);
  for (let step = 0; step < 10; step += 1) await page.mouse.wheel(0, 2_000);
  await expect.poll(() => settings.evaluate((element) => {
    const body = element.querySelector('.modal-body');
    return element.scrollTop + (body?.scrollTop ?? 0);
  })).toBeGreaterThan(0);
  await page.screenshot({ path: testInfo.outputPath('settings-over-stacks.png') });
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
  expect(await documentFits()).toBe(true);

  await page.getByRole('button', { name: 'Close settings' }).click();
  await expect(settings).toBeHidden();
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
  expect(await mainScrollTop()).toBe(pagePlace);
  await page.evaluate(() => new Promise((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await page.screenshot({ path: testInfo.outputPath('stacks-after-settings.png') });
});
