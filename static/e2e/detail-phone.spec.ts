import { expect, test, type Page } from '@playwright/test';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

// The image detail on a phone: the image area takes the frame's shape, so a
// landscape frame has no black band above and below it. Runs in Chromium and
// in WebKit (see playwright.config.ts), the engine of every iPhone browser.

const PHONE = { width: 390, height: 844 };
test.use({ viewport: PHONE, isMobile: true, hasTouch: true });

let dbId: string;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, {
    name: 'Imaging Rig e2e',
    slug: 'imaging-rig-e2e',
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

/**
 * Answer every preview with a blank frame of this size: the image area's
 * shape is what is measured, not the pixels, and the shared preview queue
 * stays free for the specs that time it.
 */
async function stubFrame(page: Page, width: number, height: number) {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}"></svg>`;
  await page.route(/\/images\/\d+\/(preview|annotated)/, (route) =>
    route.fulfill({ status: 200, contentType: 'image/svg+xml', body: svg }),
  );
}

/** The space between the drawn frame and the edges of the image area. */
async function frameGaps(page: Page) {
  const image = page.locator('img.detail-main-image');
  await expect(image).not.toHaveClass(/detail-image-hidden/, { timeout: 15_000 });
  await expect.poll(() => image.evaluate((element) => (element as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  return page.evaluate(() => {
    const area = document.querySelector('.detail-image')!.getBoundingClientRect();
    const drawn = document.querySelector('img.detail-main-image')!.getBoundingClientRect();
    return {
      areaHeight: area.height,
      across: (drawn.left - area.left) + (area.right - drawn.right),
      down: (drawn.top - area.top) + (area.bottom - drawn.bottom),
      drawnWidth: drawn.width,
    };
  });
}

for (const [shape, width, height] of [['3:2', 1500, 1000], ['16:9', 1600, 900]] as const) {
  test(`a ${shape} frame fills the phone's image area with no band above or below`, async ({ page }) => {
    await stubFrame(page, width, height);
    await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);
    const gaps = await frameGaps(page);
    // The frame spans the width, and the space above and below it is the
    // same as at its sides.
    expect(gaps.drawnWidth).toBeGreaterThan(PHONE.width - 40);
    expect(Math.abs(gaps.down - gaps.across)).toBeLessThanOrEqual(3);

    // The next frame has the same shape: the area keeps its size, and the
    // frame still fills it.
    await page.keyboard.press('ArrowRight');
    await expect(page).toHaveURL(/#\/detail\/2/);
    const next = await frameGaps(page);
    expect(Math.abs(next.areaHeight - gaps.areaHeight)).toBeLessThanOrEqual(1);
    expect(Math.abs(next.down - next.across)).toBeLessThanOrEqual(3);
  });
}

test('a portrait frame stops at 60% of the phone screen', async ({ page }) => {
  await stubFrame(page, 1000, 1500);
  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);
  const gaps = await frameGaps(page);
  // 60% of the height for the content, plus the 0.5rem padding.
  expect(gaps.areaHeight).toBeGreaterThan(PHONE.height * 0.6);
  expect(gaps.areaHeight).toBeLessThanOrEqual(PHONE.height * 0.6 + 16 + 1);
  // Fitted to the height, it leaves bands at the sides, not above or below.
  expect(gaps.down).toBeLessThanOrEqual(36 + 3);
  expect(gaps.across).toBeGreaterThan(gaps.down);
});

test('fit and zoom still work in the phone image area', async ({ page }) => {
  await stubFrame(page, 1600, 900);
  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);
  const fitted = await frameGaps(page);
  const zoomLabel = page.locator('.zoom-percentage-compact');
  const fitPercent = await zoomLabel.textContent();

  await page.getByRole('button', { name: '+', exact: true }).click();
  await expect(zoomLabel).not.toHaveText(fitPercent!);
  await page.getByRole('button', { name: 'Fit', exact: true }).click();
  await expect(zoomLabel).toHaveText(fitPercent!);
  const refitted = await frameGaps(page);
  expect(Math.abs(refitted.down - fitted.down)).toBeLessThanOrEqual(1);
  expect(Math.abs(refitted.across - fitted.across)).toBeLessThanOrEqual(1);
});
