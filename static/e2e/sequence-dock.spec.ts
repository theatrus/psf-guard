import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import { fixtureDbPath, fixtureImageDir, resetDatabases, stubPixelWork, tmpBase, waitForCacheReady } from './helpers';

// Sequence on a session long enough to scroll: the summary and the score
// chart dock under the toolbar without moving the page, a solved pointing
// dot goes to its frame, the frame under the cursor stays clear of the
// docked chart, and the pointing chart folds. WebKit runs this too: Safari
// and the macOS app have no scroll anchoring to hide a jump.

const FRAMES = 48;

/** A copy of the fixture catalog holding one session of FRAMES frames, all
 *  with one real frame's headers, five minutes apart. */
function sessionCatalog(): string {
  const databasePath = path.join(tmpBase(), `sequence-dock-${randomUUID()}.sqlite`);
  fs.copyFileSync(fixtureDbPath(), databasePath);
  const database = new Database(databasePath);
  try {
    const existing = database.prepare('SELECT acquireddate, metadata FROM acquiredimage WHERE projectId=1 LIMIT 1').get() as {
      acquireddate: number; metadata: string;
    };
    database.exec('DELETE FROM acquiredimage');
    const insert = database.prepare(
      'INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata) VALUES(?,1,1,?,?,0,?)',
    );
    for (let index = 0; index < FRAMES; index += 1) {
      insert.run(1000 + index, existing.acquireddate + index * 300, 'L', JSON.stringify({
        ...JSON.parse(existing.metadata),
        ExposureDuration: 300, ExposureTime: 300, Filter: 'L',
      }));
    }
  } finally {
    database.close();
  }
  return databasePath;
}

async function register(request: APIRequestContext, databasePath: string): Promise<string> {
  await resetDatabases(request);
  const registered = await request.post('/api/databases', {
    data: { name: 'Sequence dock', slug: `sequence-dock-${randomUUID().slice(0, 8)}`, db_path: databasePath, image_dirs: [fixtureImageDir()] },
  });
  expect(registered.ok(), await registered.text()).toBeTruthy();
  const dbId = (await registered.json()).data.id as string;
  await waitForCacheReady(request, dbId).catch(() => {});
  return dbId;
}

/** Solved centres on a grid of 8 by 6, 12″ apart, so no dot covers
 *  another's middle, as a quality scan would have left them. The first
 *  `offTarget` frames are flagged off target, for a suspect pill. */
async function withPointing(page: Page, offTarget = 0) {
  await page.route(/\/api\/db\/[^/]+\/analysis\/sequence/, async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    for (const sequence of body.data?.sequences ?? []) {
      sequence.images.forEach((image: Record<string, unknown>, index: number) => {
        const flags = index < offTarget ? ['off_target'] : [];
        image.flags = [...((image.flags as string[] | undefined) ?? []), ...flags];
        image.pointing = {
          pixel_solved: true,
          solve_failed: false,
          image_quality_evidence: true,
          expected_target: true,
          flags,
          east_offset_arcsec: ((index % 8) - 3.5) * 12,
          north_offset_arcsec: (Math.floor(index / 8) - 2.5) * 12,
        };
      });
    }
    await route.fulfill({ response, json: body });
  });
}

async function openSequence(page: Page, dbId: string) {
  await page.goto(`/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`);
  await expect(page.locator('.sequence-image-card')).toHaveCount(FRAMES, { timeout: 30_000 });
  await expect(page.locator('.pointing-dot')).toHaveCount(FRAMES);
}

/** Where things sit: the scroll, and the toolbar's, the dock's and one
 *  card's edges, in pixels from the top of the scrolling region. A phone's
 *  header slides away as the page scrolls, which moves that region. */
function layout(page: Page, imageId: number) {
  return page.evaluate((id) => {
    const scroller = document.querySelector<HTMLElement>('.app-main')!;
    const origin = scroller.getBoundingClientRect().top;
    const card = document.querySelector<HTMLElement>(`.sequence-image-card[data-card-image-id="${id}"]`)!;
    const dock = document.querySelector<HTMLElement>('.toolbar-dock')!;
    return {
      scrollTop: scroller.scrollTop,
      viewportBottom: scroller.getBoundingClientRect().bottom - origin,
      toolbarBottom: document.querySelector<HTMLElement>('.sequence-view > .view-toolbar')!.getBoundingClientRect().bottom - origin,
      dockTop: dock.getBoundingClientRect().top - origin,
      dockBottom: dock.getBoundingClientRect().bottom - origin,
      // A suspect list hangs below the docked block, over the strip.
      listBottom: dock.hasAttribute('data-docked')
        ? (dock.querySelector('.sequence-issue-list')?.getBoundingClientRect().bottom ?? origin) - origin
        : 0,
      cardTop: card.getBoundingClientRect().top - origin,
      cardBottom: card.getBoundingClientRect().bottom - origin,
    };
  }, imageId);
}

/** Until the scroll has held still for ten frames: the view brings the
 *  cursor's card into sight as it opens. */
const scrollAtRest = (page: Page) => page.evaluate(() => new Promise<void>((resolve) => {
  const scroller = document.querySelector<HTMLElement>('.app-main')!;
  let last = scroller.scrollTop;
  let still = 0;
  const tick = () => {
    still = scroller.scrollTop === last ? still + 1 : 0;
    last = scroller.scrollTop;
    if (still >= 10) resolve();
    else requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}));

/** Whether a card shows whole between the docked chart and the bottom. */
async function clearOfDock(page: Page, imageId: number) {
  const place = await layout(page, imageId);
  const covered = Math.max(place.toolbarBottom, place.dockBottom, place.listBottom);
  return place.cardTop >= covered - 1 && place.cardBottom <= place.viewportBottom + 1;
}

/** The first card wholly below the docked block. */
function cardUnderDock(page: Page) {
  return page.evaluate(() => {
    const dock = document.querySelector('.toolbar-dock')!.getBoundingClientRect().bottom;
    const card = Array.from(document.querySelectorAll<HTMLElement>('.sequence-image-card'))
      .find((element) => element.getBoundingClientRect().top >= dock + 10)!;
    return Number(card.dataset.cardImageId);
  });
}

/** The lowest card wholly in the window. */
function lowestCardInView(page: Page) {
  return page.evaluate(() => {
    const bottom = document.querySelector('.app-main')!.getBoundingClientRect().bottom;
    const cards = Array.from(document.querySelectorAll<HTMLElement>('.sequence-image-card'))
      .filter((element) => element.getBoundingClientRect().bottom <= bottom);
    return Number(cards[cards.length - 1].dataset.cardImageId);
  });
}

/** Scroll into the strip until the block docks. */
async function scrollToDock(page: Page) {
  await expect.poll(async () => {
    await page.evaluate(() => { document.querySelector<HTMLElement>('.app-main')!.scrollTop += 150; });
    return page.locator('.toolbar-dock').getAttribute('data-docked');
  }).toBe('true');
}

/** Two frames, so a scroll and anything it sets off have drawn. */
const settle = (page: Page) => page.evaluate(() => new Promise<void>((resolve) => {
  requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
}));

for (const viewport of [{ width: 1440, height: 900 }, { width: 390, height: 844 }]) {
  test.describe(`at ${viewport.width} wide`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize(viewport);
      await stubPixelWork(page);
      await withPointing(page);
    });

    test('the summary and chart dock under the toolbar without moving the page', async ({ page, request }) => {
      test.setTimeout(90_000);
      const dbId = await register(request, sessionCatalog());
      await openSequence(page, dbId);
      const dock = page.locator('.toolbar-dock');
      // The view may scroll its first card into sight as it opens; start
      // from the top.
      await scrollAtRest(page);
      await page.evaluate(() => { document.querySelector<HTMLElement>('.app-main')!.scrollTop = 0; });
      await settle(page);
      await expect(dock).not.toHaveAttribute('data-docked');
      // The first card: nothing above it lays itself out late, as cards
      // coming into sight below can in Safari.
      const reference = 1000;

      // Scroll through the point where the block docks, a little at a time:
      // each step moves the card by exactly the scroll, so docking shifts
      // nothing on the page.
      let before = await layout(page, reference);
      for (const step of [4, 8, 16, 40, 120, 200]) {
        await page.evaluate((by) => { document.querySelector<HTMLElement>('.app-main')!.scrollTop += by; }, step);
        await settle(page);
        const after = await layout(page, reference);
        expect(after.scrollTop - before.scrollTop).toBeCloseTo(step, 0);
        expect(before.cardTop - after.cardTop).toBeCloseTo(after.scrollTop - before.scrollTop, 0);
        before = after;
      }
      await expect(dock).toHaveAttribute('data-docked', 'true');
      await expect(dock.locator('.sequence-summary-bar')).toBeVisible();
      await expect(dock.locator('.sequence-timeline svg')).toBeVisible();
      expect(before.dockTop).toBeCloseTo(before.toolbarBottom, 0);
      // The page goes on scrolling under it.
      await page.evaluate(() => { document.querySelector<HTMLElement>('.app-main')!.scrollTop += 300; });
      await settle(page);
      const further = await layout(page, reference);
      expect(further.dockTop).toBeCloseTo(further.toolbarBottom, 0);

      // Back at the top it scrolls with the page again.
      await page.evaluate(() => { document.querySelector<HTMLElement>('.app-main')!.scrollTop = 0; });
      await expect(dock).not.toHaveAttribute('data-docked');
    });

    test('a pointing dot goes to its frame, which shows below the docked chart', async ({ page, request }) => {
      test.setTimeout(90_000);
      const dbId = await register(request, sessionCatalog());
      await openSequence(page, dbId);

      const target = 1000 + FRAMES - 4;
      await page.locator(`.pointing-dot[data-pointing-image-id="${target}"]`).click();
      await expect(page).toHaveURL(new RegExp(`current=${target}(&|$)`));
      await expect(page.locator(`.sequence-image-card[data-card-image-id="${target}"]`)).toHaveClass(/current-selection/);
      await expect(page.locator('.toolbar-dock')).toHaveAttribute('data-docked', 'true');
      await expect.poll(() => clearOfDock(page, target)).toBe(true);

      // Up through the strip by keys: every frame the cursor reaches shows
      // below the docked chart, never under it.
      let current = target;
      for (let press = 0; press < 4; press += 1) {
        await page.keyboard.press('ArrowUp');
        await expect(page).not.toHaveURL(new RegExp(`current=${current}(&|$)`));
        current = Number(new URL(page.url().replace('/#/', '/')).searchParams.get('current'));
        await expect.poll(() => clearOfDock(page, current)).toBe(true);
      }
    });

    test('the pointing chart folds and stays folded after a reload', async ({ page, request }) => {
      test.setTimeout(90_000);
      const dbId = await register(request, sessionCatalog());
      await openSequence(page, dbId);

      const toggle = page.getByRole('button', { name: 'Solved pointing' });
      await expect(toggle).toHaveAttribute('aria-expanded', 'true');
      await toggle.click();
      await expect(toggle).toHaveAttribute('aria-expanded', 'false');
      await expect(page.locator('.pointing-dot')).toHaveCount(0);
      await expect(page.locator('.pointing-scatter small')).toHaveText(`${FRAMES} solved frames`);

      await page.reload();
      await expect(page.locator('.sequence-image-card')).toHaveCount(FRAMES, { timeout: 30_000 });
      await expect(page.getByRole('button', { name: 'Solved pointing' })).toHaveAttribute('aria-expanded', 'false');
      await page.getByRole('button', { name: 'Solved pointing' }).click();
      await expect(page.locator('.pointing-dot')).toHaveCount(FRAMES);
    });
  });
}

// A suspect list opened from the docked summary hangs below the block, over
// the strip, rather than growing it: the block stays docked and the page
// stays put. A cursor's card it would cover moves out from under it.
test.describe('with a suspect list open', () => {
  test('a list keeps the block docked and moves the cursor\'s card out from under it', async ({ page, request }) => {
    test.setTimeout(90_000);
    await page.setViewportSize({ width: 1440, height: 900 });
    await stubPixelWork(page);
    await withPointing(page, 2);
    const dbId = await register(request, sessionCatalog());
    await openSequence(page, dbId);
    await scrollToDock(page);
    const target = await cardUnderDock(page);
    await page.locator(`.sequence-image-card[data-card-image-id="${target}"]`).click();
    await expect(page).toHaveURL(new RegExp(`current=${target}(&|$)`));

    await page.locator('.toolbar-dock .summary-issues [data-issue]').click();
    await expect(page.locator('.toolbar-dock .sequence-issue-list')).toBeVisible();
    await expect(page.locator('.toolbar-dock')).toHaveAttribute('data-docked', 'true');
    await expect.poll(() => clearOfDock(page, target)).toBe(true);
  });

  for (const viewport of [{ width: 1440, height: 900 }, { width: 1366, height: 768 }, { width: 390, height: 844 }]) {
    test(`deep in the strip at ${viewport.width}×${viewport.height}, a long list opens and closes in place`, async ({ page, request }) => {
      test.setTimeout(90_000);
      await page.setViewportSize(viewport);
      await stubPixelWork(page);
      await withPointing(page, 30);
      const dbId = await register(request, sessionCatalog());
      await openSequence(page, dbId);
      // Far down, once the view has brought its first card into sight.
      await scrollAtRest(page);
      await page.evaluate(() => {
        const scroller = document.querySelector<HTMLElement>('.app-main')!;
        scroller.scrollTop = scroller.scrollHeight;
      });
      await scrollAtRest(page);
      const dock = page.locator('.toolbar-dock');
      await expect(dock).toHaveAttribute('data-docked', 'true');
      const pill = dock.locator('.summary-issues [data-issue]');
      await pill.click();
      await expect(dock.locator('.sequence-issue-list')).toBeVisible();
      await expect(dock).toHaveAttribute('data-docked', 'true');

      // The cursor on a card low in the window, below the list.
      const target = await lowestCardInView(page);
      await page.locator(`.sequence-image-card[data-card-image-id="${target}"]`).click();
      await expect(page).toHaveURL(new RegExp(`current=${target}(&|$)`));
      await scrollAtRest(page);
      await expect.poll(() => clearOfDock(page, target)).toBe(true);
      const start = (await layout(page, target)).scrollTop;

      // Closing and opening the list moves neither the page nor the card
      // out of sight.
      for (const open of [false, true]) {
        await pill.click();
        await expect(dock.locator('.sequence-issue-list')).toHaveCount(open ? 1 : 0);
        await expect(dock).toHaveAttribute('data-docked', 'true');
        await scrollAtRest(page);
        await expect.poll(() => clearOfDock(page, target)).toBe(true);
        expect(Math.abs((await layout(page, target)).scrollTop - start)).toBeLessThanOrEqual(2);
      }
    });
  }
});

test('on a phone, the header sliding away and back does not flip the block between docked and too tall', async ({ page, request }) => {
  test.setTimeout(90_000);
  // A short phone, where the toolbar and the block (with a pill making
  // the summary a line taller) cover more than 55% of the room with the
  // header in sight, but less with it slid away: measured from the
  // scrolling region as it is, the block flipped as the header moved.
  await page.setViewportSize({ width: 390, height: 800 });
  await stubPixelWork(page);
  await withPointing(page, 2);
  const dbId = await register(request, sessionCatalog());
  await openSequence(page, dbId);
  await scrollAtRest(page);
  await page.evaluate(() => { document.querySelector<HTMLElement>('.app-main')!.scrollTop = 1500; });
  await scrollAtRest(page);
  await expect(page.locator('.toolbar-dock')).toHaveAttribute('data-too-tall', 'true');
  await page.evaluate(() => {
    const w = window as unknown as { flips: number };
    w.flips = 0;
    new MutationObserver(() => { w.flips += 1; })
      .observe(document.querySelector('.toolbar-dock')!, { attributes: true, attributeFilter: ['data-too-tall'] });
  });
  // Down far enough for the header to slide away, then back up for it
  // to return, a step at a time.
  for (const step of [...Array(8).fill(60), ...Array(8).fill(-60)]) {
    await page.mouse.wheel(0, step);
    await page.waitForTimeout(60);
  }
  await page.waitForTimeout(400);
  expect(await page.evaluate(() => (window as unknown as { flips: number }).flips)).toBe(0);
  await expect(page.locator('.toolbar-dock')).toHaveAttribute('data-too-tall', 'true');
});
