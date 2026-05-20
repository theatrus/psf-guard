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

test('preview images load with per-DB-nested URLs', async ({ page }) => {
  // Pre-expand the group so cards mount (see note in navigation.spec.ts).
  await page.goto(
    `/#/grid?db=${encodeURIComponent(dbId)}&project=1&expanded=B`
  );

  const firstCard = page.locator('.image-card').first();
  await expect(firstCard).toBeVisible({ timeout: 15_000 });

  // The card's <img> should source from /api/db/<slug>/images/<id>/preview.
  // Just check the src shape — actual byte-level rendering depends on the
  // FITS fixture quality, but the URL construction itself is the unit under
  // test for the multi-DB API client.
  const previewImg = firstCard.locator('img').first();
  const src = await previewImg.getAttribute('src');
  expect(src, 'preview src should be db-nested').toContain(
    `/api/db/${dbId}/images/`
  );
  expect(src).toContain('/preview');
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

test('detail-view route resolves with a per-DB preview', async ({ page }) => {
  await page.goto(
    `/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`
  );
  // The detail view renders a large <img> for the full-resolution preview.
  // Like the grid card, we assert on the URL construction rather than the
  // rendered pixels.
  const img = page.locator('img').first();
  await expect(img).toBeVisible({ timeout: 10_000 });
  const src = await img.getAttribute('src');
  expect(src).toContain(`/api/db/${dbId}/images/1/`);
});
