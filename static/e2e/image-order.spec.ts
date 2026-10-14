import { expect, test, type Page } from '@playwright/test';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

// Newest first in Images and Sequence, and the "new" dot, against the real
// server. Project Alpha's three frames share one filter, so Images shows
// them as one group.

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

const gridIds = (page: Page) => page.locator('.image-card-wrapper').evaluateAll(
  (cards) => cards.map((card) => card.getAttribute('data-image-id')),
);
const stripIds = (page: Page) => page.locator('.sequence-image-card').evaluateAll(
  (cards) => cards.map((card) => card.getAttribute('data-card-image-id')),
);

async function chooseOrder(page: Page, name: 'Oldest first' | 'Newest first') {
  await page.getByRole('button', { name: /^View/ }).click();
  await page.getByRole('radiogroup', { name: 'Sort' }).getByRole('radio', { name }).click();
  // Escape closes the panel and hands focus back to View; let the page
  // have the keys again, as after a click on the grid.
  await page.keyboard.press('Escape');
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
}

test('Images turns around in place, and the keys and the detail follow', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&size=300`);
  const wrappers = page.locator('.image-card-wrapper');
  await expect(wrappers).toHaveCount(3, { timeout: 15_000 });
  const oldest = await gridIds(page);

  // Put the cursor on the middle frame and select it.
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Space');
  const middle = oldest[1]!;
  await expect(page.locator(`[data-image-id="${middle}"]`)).toHaveClass(/current-selection/);

  await chooseOrder(page, 'Newest first');
  await expect.poll(() => gridIds(page)).toEqual([...oldest].reverse());
  await expect(page).toHaveURL(/order=newest/);
  await expect(page).toHaveURL(new RegExp(`current=${middle}`));
  await expect(page.locator(`[data-image-id="${middle}"]`)).toHaveClass(/current-selection/);
  await expect(page.locator(`[data-image-id="${middle}"]`)).toHaveClass(/multi-selected/);
  await expect(page.locator(`[data-image-id="${middle}"]`)).toBeInViewport();
  await expect(page.getByRole('button', { name: 'View (newest first)' })).toBeVisible();

  // Right is the next card shown: the older frame now.
  await page.keyboard.press('ArrowRight');
  await expect(page.locator(`[data-image-id="${oldest[0]}"]`)).toHaveClass(/current-selection/);
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowLeft');
  await expect(page.locator(`[data-image-id="${oldest[2]}"]`)).toHaveClass(/current-selection/);

  // The detail's next walks the same way.
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(new RegExp(`#/detail/${oldest[2]}`));
  // The detail's own keys are in place once it has drawn.
  await expect(page.locator('.image-detail')).toBeVisible();
  await expect(page.locator('.image-detail .detail-loading')).toHaveCount(0);
  await page.keyboard.press('ArrowRight');
  await expect(page).toHaveURL(new RegExp(`#/detail/${middle}`));
  await expect(page).toHaveURL(/order=newest/);
});

test('Sequence follows the order, and the score chart keeps capture order', async ({ page }) => {
  await page.goto(`/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`);
  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  const oldest = await stripIds(page);
  const chart = () => page.locator('.sequence-timeline rect[data-image-id]').evaluateAll(
    (bars) => bars.map((bar) => bar.getAttribute('data-image-id')),
  );
  expect(await chart()).toEqual(oldest);

  await chooseOrder(page, 'Newest first');
  await expect.poll(() => stripIds(page)).toEqual([...oldest].reverse());
  expect(await chart()).toEqual(oldest);
  // The cursor stays on the frame it was on, now the last card.
  await expect(cards.nth(2)).toHaveClass(/current-selection/);

  await page.keyboard.press('ArrowLeft');
  await expect(cards.nth(1)).toHaveClass(/current-selection/);
  await expect(cards.nth(1)).toHaveAttribute('data-card-image-id', oldest[1]!);

  // Images opens in the same order from the header.
  await page.getByRole('button', { name: 'Images', exact: true }).click();
  await expect(page).toHaveURL(/#\/grid\?.*order=newest/);
  await expect.poll(() => gridIds(page)).toEqual([...oldest].reverse());
});

test('a project opened from the Library remembers newest first and marks its new frames', async ({ page, request }) => {
  const response = await request.get(`/api/db/${encodeURIComponent(dbId)}/images?project_id=1&limit=100`);
  const frames = (await response.json()).data as { id: number; acquired_date: number }[];
  const byTime = [...frames].sort((a, b) => a.acquired_date - b.acquired_date);
  expect(byTime[0].acquired_date).toBeLessThan(byTime[2].acquired_date);

  // This browser last saw Project Alpha when only its oldest frame was in,
  // and chose newest first.
  await page.goto('/');
  await page.evaluate(({ key, latest }) => {
    window.localStorage.setItem('psf-guard:project-seen:v2', JSON.stringify({
      [key]: { latestImage: latest, totalImages: 1, targets: {} },
    }));
    window.localStorage.setItem('psf-guard.display-preferences', JSON.stringify({ imageOrder: 'newest' }));
  }, { key: `${dbId}:1`, latest: byTime[0].acquired_date });
  await page.reload();

  await page.getByRole('button', { name: 'Open Project Alpha image grid' }).click();
  const wrappers = page.locator('.image-card-wrapper');
  await expect(wrappers).toHaveCount(3, { timeout: 15_000 });
  // No order in the link: the remembered one.
  await expect.poll(() => gridIds(page)).toEqual(byTime.map((frame) => String(frame.id)).reverse());
  await expect(page.getByRole('button', { name: 'View (newest first)' })).toBeVisible();

  // The two later frames carry the dot; the one already seen does not.
  for (const frame of byTime.slice(1)) {
    await expect(page.locator(`[data-image-id="${frame.id}"] .image-new-mark`)).toHaveCount(1);
  }
  await expect(page.locator(`[data-image-id="${byTime[0].id}"] .image-new-mark`)).toHaveCount(0);
  await expect(page.getByTitle('New since your last visit from the Library')).toHaveCount(2);

  // Sequence marks the same frames.
  await page.getByRole('button', { name: 'Sequence', exact: true }).click();
  await expect(page.locator('.sequence-image-card')).toHaveCount(3, { timeout: 15_000 });
  await expect(page.locator('.sequence-image-card .image-new-mark')).toHaveCount(2);
});
