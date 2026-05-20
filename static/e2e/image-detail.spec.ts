import { expect, test } from '@playwright/test';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

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

test('preview images load with per-DB-nested URLs and render pixels', async ({
  page,
}) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const cards = page.locator('.image-card');
  await expect(cards.first()).toBeVisible({ timeout: 15_000 });

  // The src should be db-nested at `/api/db/<slug>/images/<id>/preview`.
  const firstImg = cards.first().locator('img').first();
  const src = await firstImg.getAttribute('src');
  expect(src, 'preview src should be db-nested').toContain(
    `/api/db/${dbId}/images/`
  );
  expect(src).toContain('/preview');

  // Wait for every card's preview to finish loading. LazyImageCard ramps
  // opacity 0 → 1 only after `<img>.onLoad` fires, so this also matches
  // the moment the user actually sees the thumbnails.
  await page.waitForLoadState('networkidle');
  const cardCount = await cards.count();
  for (let i = 0; i < cardCount; i++) {
    const img = cards.nth(i).locator('img').first();
    await page.waitForFunction(
      (el) =>
        el instanceof HTMLImageElement &&
        el.complete &&
        el.naturalWidth > 0,
      await img.elementHandle(),
      { timeout: 30_000 }
    );
    const dims = await img.evaluate((el) => ({
      natW: (el as HTMLImageElement).naturalWidth,
      natH: (el as HTMLImageElement).naturalHeight,
    }));
    expect(dims.natW, `card #${i} naturalWidth`).toBeGreaterThan(100);
    expect(dims.natH, `card #${i} naturalHeight`).toBeGreaterThan(100);
  }
});

test('preview endpoint returns a 200 for a registered image', async ({
  request,
}) => {
  // Direct API smoke: confirms the server can find and serve the FITS file
  // we wrote into the fixture image dir, end-to-end through the per-DB
  // routing and find_fits_file lookup. This decouples "preview works" from
  // any frontend rendering quirks.
  const res = await request.get(
    `/api/db/${encodeURIComponent(dbId)}/images/1/preview?size=screen`
  );
  expect(res.status(), `body: ${await res.text().catch(() => '<unread>')}`).toBe(200);
  expect(res.headers()['content-type']).toMatch(/^image\//);
  const buf = await res.body();
  expect(buf.byteLength).toBeGreaterThan(0);
});

test('detail view loads the large preview and renders pixels', async ({
  page,
}) => {
  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);

  // Scope to the detail overlay so we don't pick up the empty grid img
  // sitting behind it. The detail container's <img> targets the
  // per-DB preview endpoint with size=large by default.
  const img = page.locator('.detail-overlay img').first();
  await expect(img).toBeVisible({ timeout: 15_000 });

  const src = await img.getAttribute('src');
  expect(src).toContain(`/api/db/${dbId}/images/1/`);

  // Wait for the bytes to arrive and decode — a 404 or broken render path
  // would leave naturalWidth at 0.
  await page.waitForFunction(
    (el) => el instanceof HTMLImageElement && el.complete && el.naturalWidth > 0,
    await img.elementHandle(),
    { timeout: 30_000 }
  );
  const dims = await img.evaluate((el) => ({
    natW: (el as HTMLImageElement).naturalWidth,
    natH: (el as HTMLImageElement).naturalHeight,
  }));
  expect(dims.natW).toBeGreaterThan(500);
  expect(dims.natH).toBeGreaterThan(500);
});
