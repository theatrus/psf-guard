import { expect, test, type Page } from '@playwright/test';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

// Scroll positions in and around the image detail. Runs in Chromium and in
// WebKit (see playwright.config.ts): WebKit has no CSS scroll anchoring, so
// Safari and the macOS app keep their place only by what the app does.

let dbId: string;

test.use({ viewport: { width: 1280, height: 640 } });

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, {
    name: 'Imaging Rig e2e',
    slug: 'imaging-rig-e2e',
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

/** The Satellite tracks panel's distance from the top of the sidebar. */
function satellitesOffset(page: Page) {
  return page.evaluate(() => {
    const sidebar = document.querySelector('.detail-info')!;
    const panel = document.querySelector('[data-testid="satellite-panel"]')!;
    return panel.getBoundingClientRect().top - sidebar.getBoundingClientRect().top;
  });
}

/** A Sequence card's distance from the top of the page's scroller. */
function cardOffset(page: Page, imageId: number) {
  return page.evaluate((id) => {
    const scroller = document.querySelector('.app-main')!;
    const card = document.querySelector(`.sequence-image-card[data-card-image-id="${id}"]`)!;
    return card.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
  }, imageId);
}

async function settledSidebar(page: Page) {
  await expect(page.getByTestId('astrometry-panel')).not.toContainText('Reading headers…');
  await expect(page.getByTestId('calibration-panel')).not.toContainText('Matching the light');
  await expect(page.getByTestId('satellite-panel')).not.toContainText('Checking cache…');
}

test('the detail sidebar keeps its place while arrow keys open and fold Calibration above it', async ({ page }) => {
  // Only the second light has a calibration finding, so its Calibration
  // panel opens on its own and the third light's folds again.
  await page.route('**/calibration-health/lights*', (route) => route.fulfill({
    json: {
      success: true,
      error: null,
      status: 'ready',
      data: { lights: [{ image_id: 2, level: 'poor', codes: ['thin_dark'] }], stale_projects: [], running: false },
    },
  }));
  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);
  const calibration = page.getByTestId('calibration-panel').getByRole('button', { name: /^Calibration/ });
  await expect(calibration).toHaveAttribute('aria-expanded', 'false', { timeout: 15_000 });
  await settledSidebar(page);

  // Read the satellites, as far down as the sidebar goes.
  await page.evaluate(() => {
    const sidebar = document.querySelector('.detail-info')!;
    sidebar.scrollTop = sidebar.scrollHeight;
  });
  const scrolled = await page.locator('.detail-info').evaluate((sidebar) => sidebar.scrollTop);
  expect(scrolled).toBeGreaterThan(0);
  const before = await satellitesOffset(page);

  await page.keyboard.press('ArrowRight');
  await expect(page).toHaveURL(/#\/detail\/2/);
  await expect(calibration).toHaveAttribute('aria-expanded', 'true');
  await settledSidebar(page);
  await expect.poll(async () => Math.abs(await satellitesOffset(page) - before)).toBeLessThanOrEqual(1.5);

  await page.keyboard.press('ArrowRight');
  await expect(page).toHaveURL(/#\/detail\/3/);
  await expect(calibration).toHaveAttribute('aria-expanded', 'false');
  await settledSidebar(page);
  await expect.poll(async () => Math.abs(await satellitesOffset(page) - before)).toBeLessThanOrEqual(1.5);
});

test('closing the detail returns Images to the same scroll position', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&size=1200`);
  const cards = page.locator('.image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  const scroller = page.locator('.app-main');
  await cards.nth(1).evaluate((card) => card.scrollIntoView({ block: 'center' }));
  await page.waitForTimeout(300);
  const before = await scroller.evaluate((element) => element.scrollTop);
  expect(before).toBeGreaterThan(0);

  // Dispatched, so Playwright does not scroll the card to make it actionable.
  await cards.nth(1).dispatchEvent('dblclick');
  await expect(page).toHaveURL(/#\/detail\/2/);
  await page.keyboard.press('ArrowRight');
  await expect(page).toHaveURL(/#\/detail\/3/);
  await page.locator('.image-detail-overlay .close-button').click();

  await expect(page).toHaveURL(/#\/grid\?/);
  await expect.poll(async () => Math.abs(await scroller.evaluate((element) => element.scrollTop) - before))
    .toBeLessThanOrEqual(1.5);
});

test('closing the detail returns Sequence with the card it opened where it was', async ({ page }) => {
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1&current=2&size=1200`
  );
  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  const current = cards.nth(1);
  await expect(current).toHaveClass(/current-selection/);
  await current.evaluate((element) => element.scrollIntoView({ block: 'center' }));
  await page.waitForTimeout(300);
  const before = await cardOffset(page, 2);
  expect(await page.locator('.app-main').evaluate((element) => element.scrollTop)).toBeGreaterThan(0);

  await current.dispatchEvent('dblclick');
  await expect(page).toHaveURL(/#\/detail\/2/);
  await page.keyboard.press('ArrowRight');
  await expect(page).toHaveURL(/#\/detail\/3/);
  await page.locator('.image-detail-overlay .close-button').click();

  await expect(page).toHaveURL(/#\/sequence\?/);
  await expect(page).toHaveURL(/current=3/);
  await expect(cards.nth(2)).toHaveClass(/current-selection/);
  // Sequence is drawn again on the way back. The card is the place to keep:
  // WebKit leaves a preview above the view unloaded, so the cards above are
  // shorter and the raw scroll offset differs, while the card does not move.
  await expect.poll(async () => Math.abs(await cardOffset(page, 2) - before)).toBeLessThanOrEqual(1.5);
});
