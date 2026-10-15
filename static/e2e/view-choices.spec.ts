import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import { fixtureDbPath, fixtureImageDir, resetDatabases, tmpBase, waitForCacheReady } from './helpers';

// The View panel's choices on a catalog long enough to scroll: the order
// flipping around the current card, exposure sub-groups as frames arrive,
// each view's own thumbnail size, and an old All projects link.

const HOUR = 60 * 60;

interface Frame {
  project: number;
  target: number;
  filter: string;
  exposure: number;
  offset: number;
}

/** A copy of the fixture catalog holding these frames and no others, all
 *  with one real frame's headers and their own exposure and filter. */
function syntheticCatalog(frames: Frame[]): { databasePath: string; insert: (id: number, frame: Frame) => void } {
  const databasePath = path.join(tmpBase(), `view-choices-${randomUUID()}.sqlite`);
  fs.copyFileSync(fixtureDbPath(), databasePath);
  const database = new Database(databasePath);
  const existing = database.prepare('SELECT acquireddate, metadata FROM acquiredimage WHERE projectId=1 LIMIT 1').get() as {
    acquireddate: number; metadata: string;
  };
  database.exec('DELETE FROM acquiredimage');
  database.close();
  const insert = (id: number, frame: Frame) => {
    const writer = new Database(databasePath);
    try {
      writer.prepare(
        'INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata) VALUES(?,?,?,?,?,0,?)',
      ).run(id, frame.project, frame.target, existing.acquireddate + frame.offset, frame.filter, JSON.stringify({
        ...JSON.parse(existing.metadata),
        ExposureDuration: frame.exposure, ExposureTime: frame.exposure, Filter: frame.filter,
      }));
    } finally {
      writer.close();
    }
  };
  frames.forEach((frame, index) => insert(1000 + index, frame));
  return { databasePath, insert };
}

async function register(request: APIRequestContext, databasePath: string): Promise<string> {
  await resetDatabases(request);
  const registered = await request.post('/api/databases', {
    data: { name: 'View choices', slug: `view-choices-${randomUUID().slice(0, 8)}`, db_path: databasePath, image_dirs: [fixtureImageDir()] },
  });
  expect(registered.ok(), await registered.text()).toBeTruthy();
  const dbId = (await registered.json()).data.id as string;
  await waitForCacheReady(request, dbId).catch(() => {});
  return dbId;
}

const gridIds = (page: Page) => page.locator('.image-card-wrapper').evaluateAll(
  (cards) => cards.map((card) => Number(card.getAttribute('data-image-id'))),
);
const headings = (page: Page) => page.locator('.filter-header h3').allTextContents();

/** Until no card waits for its preview and every preview has faded in.
 *  On a slow runner WebKit draws almost no frames while a page of previews
 *  decodes and fades in, and a click then waits for a steady frame that
 *  does not come in its five seconds. Polled on a timer, not per frame. */
async function previewsSettled(page: Page) {
  await expect(page.locator('.image-card-wrapper .preview-status-box:not(.preview-status-error)')).toHaveCount(0, { timeout: 60_000 });
  await page.waitForFunction(
    () => document.getAnimations().every((animation) => !((animation.effect as KeyframeEffect | null)?.target instanceof HTMLImageElement)),
    null,
    { polling: 250, timeout: 60_000 },
  );
}

/** By keyboard, as a person can: a key press does not wait for the
 *  steady frame a click does, which a slow WebKit can take seconds to draw
 *  under a page of previews. */
async function chooseOrder(page: Page, name: 'Oldest first' | 'Newest first') {
  await page.getByRole('button', { name: /^View/ }).focus();
  await page.keyboard.press('Enter');
  const choice = page.getByRole('radiogroup', { name: 'Sort' }).getByRole('radio', { name });
  await choice.focus();
  await page.keyboard.press('Enter');
  await expect(choice).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('Escape');
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
}

/** Where a card sits on screen, from the top of the scrolling region. */
const screenTop = (page: Page, id: number) => page.locator(`[data-image-id="${id}"]`).evaluate((card) => {
  const scroller = card.closest('.app-main')!;
  return card.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
});

test('a flip after the person moved keeps the current card where it is on a long page', async ({ page, request }) => {
  test.setTimeout(90_000);
  // Sixty frames of one filter, an hour apart: fifteen rows at this size.
  const frames = Array.from({ length: 60 }, (_, index): Frame => ({
    project: 1, target: 1, filter: 'L', exposure: 60, offset: index * HOUR,
  }));
  const dbId = await register(request, syntheticCatalog(frames).databasePath);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&size=300`);
  await expect(page.locator('.image-card-wrapper')).toHaveCount(60, { timeout: 30_000 });
  await previewsSettled(page);
  const oldest = await gridIds(page);

  // Scroll a card from the middle into view and click it: the person has
  // moved, so the flip keeps that card's place.
  const chosen = oldest[33];
  await page.locator(`[data-image-id="${chosen}"]`).evaluate((card) => {
    const scroller = card.closest('.app-main')!;
    scroller.scrollTop += card.getBoundingClientRect().top - scroller.getBoundingClientRect().top - 320;
  });
  await previewsSettled(page);
  await page.locator(`[data-image-id="${chosen}"] .image-card`).click();
  await expect(page.locator(`[data-image-id="${chosen}"]`)).toHaveClass(/current-selection/);
  const before = await screenTop(page, chosen);
  const scrolled = await page.locator('.app-main').evaluate((main) => main.scrollTop);
  expect(scrolled).toBeGreaterThan(1000);

  await chooseOrder(page, 'Newest first');
  await expect.poll(() => gridIds(page)).toEqual([...oldest].reverse());
  await expect(page.locator(`[data-image-id="${chosen}"]`)).toHaveClass(/current-selection/);
  // Let previews and the lazy cards settle, then measure again.
  await previewsSettled(page);
  await page.waitForTimeout(500);
  expect(Math.abs(await screenTop(page, chosen) - before)).toBeLessThanOrEqual(2);

  await chooseOrder(page, 'Oldest first');
  await expect.poll(() => gridIds(page)).toEqual(oldest);
  await previewsSettled(page);
  await page.waitForTimeout(500);
  expect(Math.abs(await screenTop(page, chosen) - before)).toBeLessThanOrEqual(2);
});

test('exposure sub-groups keep their order under Newest first as frames arrive', async ({ page, request }, testInfo) => {
  test.setTimeout(90_000);
  // One filter, 60 s and 300 s frames in turn; the newest is a 60 s one.
  const frames: Frame[] = [60, 300, 60].map((exposure, index) => ({
    project: 1, target: 1, filter: 'L', exposure, offset: index * HOUR,
  }));
  const catalog = syntheticCatalog(frames);
  const dbId = await register(request, catalog.databasePath);
  // The copied catalog carries whatever earlier specs chose for the
  // project; this one splits by exposure.
  const split = await request.put(`/api/db/${encodeURIComponent(dbId)}/projects/1/processing-settings`, {
    data: { split_exposure_groups: true },
  });
  expect(split.ok(), await split.text()).toBeTruthy();
  await page.clock.install();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&order=newest&size=300`);
  await expect(page.locator('.image-card-wrapper')).toHaveCount(3, { timeout: 30_000 });
  await expect.poll(() => headings(page)).toEqual(['L · 60 s', 'L · 300 s']);
  expect(await gridIds(page)).toEqual([1002, 1000, 1001]);

  // A 300 s frame comes in, the newest now; the grid's refresh picks it up.
  catalog.insert(1003, { project: 1, target: 1, filter: 'L', exposure: 300, offset: 3 * HOUR });
  await page.clock.fastForward('00:31');
  await expect(page.locator('.image-card-wrapper')).toHaveCount(4, { timeout: 15_000 });
  // The sub-groups stay where they were; the new frame leads its own.
  expect(await headings(page)).toEqual(['L · 60 s', 'L · 300 s']);
  expect(await gridIds(page)).toEqual([1002, 1000, 1003, 1001]);
  // Tall enough to show both groups' dates.
  await page.setViewportSize({ width: 1440, height: 1240 });
  await page.mouse.move(700, 1200);
  await page.screenshot({ path: testInfo.outputPath('exposure-subgroups-newest-1440.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.locator('.filter-header h3').first()).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('exposure-subgroups-newest-390.png') });
});

test('Images and Sequence each keep their own thumbnail size across the header', async ({ page, request }, testInfo) => {
  test.setTimeout(90_000);
  const frames = Array.from({ length: 12 }, (_, index): Frame => ({
    project: 1, target: 1, filter: 'L', exposure: 60, offset: index * 600,
  }));
  const dbId = await register(request, syntheticCatalog(frames).databasePath);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&target=1`);
  await expect(page.locator('.image-card-wrapper')).toHaveCount(12, { timeout: 30_000 });
  const gridSize = page.locator('#grid-thumbnail-size');
  await expect(gridSize).toHaveValue('300');
  await previewsSettled(page);
  // Images to 450, a step at a time: the size lives in the URL, and a key
  // pressed before the last step reaches it starts from the old value.
  await gridSize.focus();
  for (const value of ['350', '400', '450']) {
    await gridSize.press('ArrowRight');
    await expect(gridSize).toHaveValue(value);
    await expect(page).toHaveURL(new RegExp(`size=${value}`));
  }
  await expect(page).toHaveURL(/size=450/);

  // Sequence keeps its own.
  await page.getByRole('button', { name: 'Sequence', exact: true }).click();
  await expect(page).toHaveURL(/#\/sequence\?/);
  await expect(page.locator('.sequence-image-card')).toHaveCount(12, { timeout: 30_000 });
  expect(page.url()).not.toMatch(/size=/);
  await expect(page.locator('#sequence-thumbnail-size')).toHaveValue('150');
  await page.mouse.move(700, 880);
  await page.screenshot({ path: testInfo.outputPath('sequence-own-size-1440.png') });

  // And Images comes back at its own.
  await page.getByRole('button', { name: 'Images', exact: true }).click();
  await expect(page).toHaveURL(/#\/grid\?/);
  await expect(page.locator('.image-card-wrapper')).toHaveCount(12, { timeout: 30_000 });
  await expect(page.locator('#grid-thumbnail-size')).toHaveValue('450');

  // On a phone, the same.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('button', { name: 'Sequence', exact: true }).click();
  await expect(page.locator('.sequence-image-card')).toHaveCount(12, { timeout: 30_000 });
  expect(page.url()).not.toMatch(/size=/);
  await page.mouse.move(200, 800);
  await page.screenshot({ path: testInfo.outputPath('sequence-own-size-390.png') });
});

test('an All projects link from before project grouping opens its groups', async ({ page, request }, testInfo) => {
  test.setTimeout(90_000);
  const frames: Frame[] = [
    ...Array.from({ length: 4 }, (_, index): Frame => ({ project: 1, target: 1, filter: 'Ha', exposure: 300, offset: index * HOUR })),
    ...Array.from({ length: 3 }, (_, index): Frame => ({ project: 2, target: 2, filter: 'OIII', exposure: 300, offset: index * HOUR })),
  ];
  const dbId = await register(request, syntheticCatalog(frames).databasePath);
  await page.setViewportSize({ width: 1440, height: 900 });
  // Such a link grouped All projects by filter and named the Ha group open.
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&expanded=Ha`);
  await expect(page.locator('.filter-header')).toHaveCount(2, { timeout: 30_000 });
  // It names no group shown now, so the default ones open: every project.
  await expect(page.locator('.filter-toggle')).toHaveText(['▼', '▼']);
  await expect(page.locator('.image-card-wrapper')).toHaveCount(7);
  await page.screenshot({ path: testInfo.outputPath('old-all-projects-link-1440.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.locator('.filter-header').first()).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('old-all-projects-link-390.png') });

  // A link that names one of today's groups still opens only that one:
  // folding the other writes such a link.
  await page.locator('.filter-header').nth(1).click();
  await expect(page.locator('.filter-toggle')).toHaveText(['▼', '▶']);
  const link = page.url();
  expect(link).toMatch(/expanded=/);
  await page.goto('about:blank');
  await page.goto(link);
  await expect(page.locator('.filter-toggle')).toHaveText(['▼', '▶'], { timeout: 30_000 });
});
