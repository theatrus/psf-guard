import { expect, test, type Page } from '@playwright/test';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

let dbId: string;
let slugCounter = 0;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  slugCounter += 1;
  const entry = await registerFixtureDb(request, {
    name: `Imaging Rig e2e ${slugCounter}`,
    slug: `imaging-rig-e2e-${slugCounter}`,
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

async function readDetailViewportAnchor(page: Page) {
  return page.locator('.zoom-container img.detail-main-image').first().evaluate((img) => {
    const container = img.closest('.zoom-container');
    if (!(container instanceof HTMLElement)) {
      throw new Error('Missing zoom container');
    }

    const matrix = new DOMMatrixReadOnly(getComputedStyle(img).transform);
    const rect = container.getBoundingClientRect();
    const viewportCenterX = rect.width / 2;
    const viewportCenterY = rect.height / 2;
    const scale = matrix.a || 1;
    const imageX = (viewportCenterX - matrix.e) / scale;
    const imageY = (viewportCenterY - matrix.f) / scale;

    return {
      src: (img as HTMLImageElement).currentSrc || (img as HTMLImageElement).src,
      naturalWidth: (img as HTMLImageElement).naturalWidth,
      naturalHeight: (img as HTMLImageElement).naturalHeight,
      scale,
      offsetX: matrix.e,
      offsetY: matrix.f,
      centerRatioX: imageX / (img as HTMLImageElement).naturalWidth,
      centerRatioY: imageY / (img as HTMLImageElement).naturalHeight,
    };
  });
}

async function readDetailFitState(page: Page) {
  return page.locator('.zoom-container img.detail-main-image').first().evaluate((img) => {
    const container = img.closest('.zoom-container');
    if (!(container instanceof HTMLElement)) {
      throw new Error('Missing zoom container');
    }

    const image = img as HTMLImageElement;
    const matrix = new DOMMatrixReadOnly(getComputedStyle(image).transform);
    const containerRect = container.getBoundingClientRect();
    const expectedFitScale = Math.min(
      (containerRect.width - 20) / image.naturalWidth,
      (containerRect.height - 20) / image.naturalHeight
    );
    const renderedCenterX = matrix.e + (image.naturalWidth * matrix.a) / 2;
    const renderedCenterY = matrix.f + (image.naturalHeight * matrix.d) / 2;

    return {
      className: image.className,
      currentSrc: image.currentSrc || image.src,
      expectedFitScale,
      naturalHeight: image.naturalHeight,
      naturalWidth: image.naturalWidth,
      opacity: getComputedStyle(image).opacity,
      renderedCenterX,
      renderedCenterY,
      scale: matrix.a || 1,
      viewportCenterX: containerRect.width / 2,
      viewportCenterY: containerRect.height / 2,
    };
  });
}

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
  let res = await request.get(
    `/api/db/${encodeURIComponent(dbId)}/images/1/preview?size=screen`
  );
  if (res.status() === 202) {
    await expect
      .poll(
        async () => {
          const status = await request.post(
            `/api/db/${encodeURIComponent(dbId)}/images/generation-status`,
            {
              data: {
                requests: [{ image_id: 1, kind: 'preview', size: 'screen' }],
              },
            }
          );
          return (await status.json()).data.statuses[0].state;
        },
        { timeout: 30_000, intervals: [500] }
      )
      .toBe('ready');

    res = await request.get(
      `/api/db/${encodeURIComponent(dbId)}/images/1/preview?size=screen`
    );
  }
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

test('detail view hides the pending image while arrow-key navigation generates it', async ({
  page,
}) => {
  let releaseNextImageResponse!: () => void;
  let markNextImageHeld!: () => void;
  const releasePromise = new Promise<void>((resolve) => {
    releaseNextImageResponse = resolve;
  });
  const nextImageHeldPromise = new Promise<void>((resolve) => {
    markNextImageHeld = resolve;
  });
  let nextImageHeld = false;

  await page.route('**/images/2/preview**', async (route) => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('size') !== 'large') {
      await route.continue();
      return;
    }

    const response = await route.fetch();
    if (response.status() === 200 && !nextImageHeld) {
      nextImageHeld = true;
      markNextImageHeld();
      await releasePromise;
    }
    await route.fulfill({ response });
  });

  try {
    await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);

    const img = page.locator('.zoom-container img.detail-main-image').first();
    await expect(img).toBeVisible({ timeout: 15_000 });
    await page.waitForFunction(
      (el) => el instanceof HTMLImageElement && el.complete && el.naturalWidth > 0,
      await img.elementHandle(),
      { timeout: 30_000 }
    );

    await page.keyboard.press('ArrowRight');

    await Promise.race([
      nextImageHeldPromise,
      page.waitForTimeout(45_000).then(() => {
        throw new Error('Timed out waiting for generated next detail image');
      }),
    ]);

    await expect(page).toHaveURL(/\/detail\/2/);
    await expect(page.locator('.image-loading-overlay')).toBeVisible({
      timeout: 10_000,
    });
    await expect(img).toHaveCSS('opacity', '0');
    await expect(img).toHaveClass(/detail-image-hidden/);

    releaseNextImageResponse();

    await page.waitForFunction(
      (el) =>
        el instanceof HTMLImageElement &&
        el.complete &&
        el.naturalWidth > 0 &&
        (el.currentSrc || el.src).includes('/images/2/preview'),
      await img.elementHandle(),
      { timeout: 30_000 }
    );
    await expect(page.locator('.image-loading-overlay')).toHaveCount(0, {
      timeout: 10_000,
    });
    await expect(img).not.toHaveClass(/detail-image-hidden/);
    await expect(img).toHaveCSS('opacity', '1', { timeout: 10_000 });

    const fit = await readDetailFitState(page);
    expect(fit.currentSrc).toContain('/images/2/preview');
    expect(fit.className).not.toContain('detail-image-hidden');
    expect(fit.opacity).toBe('1');
    expect(fit.naturalWidth).toBeGreaterThan(0);
    expect(Math.abs(fit.scale - fit.expectedFitScale)).toBeLessThan(0.05);
    expect(Math.abs(fit.renderedCenterX - fit.viewportCenterX)).toBeLessThan(2);
    expect(Math.abs(fit.renderedCenterY - fit.viewportCenterY)).toBeLessThan(2);
  } finally {
    releaseNextImageResponse();
    await page.unroute('**/images/2/preview**');
  }
});

test('detail view keeps the viewport anchored while swapping to original resolution', async ({
  page,
}) => {
  test.setTimeout(90_000);

  let releaseOriginalResponse!: () => void;
  let markOriginalHeld!: () => void;
  const releasePromise = new Promise<void>((resolve) => {
    releaseOriginalResponse = resolve;
  });
  const originalHeldPromise = new Promise<void>((resolve) => {
    markOriginalHeld = resolve;
  });
  let originalHeld = false;

  await page.route('**/images/1/preview**', async (route) => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('size') !== 'original') {
      await route.continue();
      return;
    }

    const response = await route.fetch();
    if (response.status() === 200 && !originalHeld) {
      originalHeld = true;
      markOriginalHeld();
      await releasePromise;
    }
    await route.fulfill({ response });
  });

  try {
    await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);

    const container = page.locator('.zoom-container').first();
    const img = container.locator('img.detail-main-image').first();
    await expect(img).toBeVisible({ timeout: 15_000 });
    await page.waitForFunction(
      (el) =>
        el instanceof HTMLImageElement && el.complete && el.naturalWidth > 0,
      await img.elementHandle(),
      { timeout: 30_000 }
    );

    const box = await container.boundingBox();
    expect(box).not.toBeNull();
    const centerX = box!.x + box!.width / 2;
    const centerY = box!.y + box!.height / 2;

    await page.mouse.move(centerX, centerY);
    await page.mouse.wheel(0, -1400);
    await page.mouse.down();
    await page.mouse.move(centerX - 120, centerY - 80, { steps: 6 });
    await page.mouse.up();

    const before = await readDetailViewportAnchor(page);
    expect(before.src).toContain('size=large');

    await Promise.race([
      originalHeldPromise,
      page.waitForTimeout(45_000).then(() => {
        throw new Error('Timed out waiting for original preview request');
      }),
    ]);

    await expect(page.locator('.image-loading-overlay')).toBeVisible({
      timeout: 10_000,
    });

    const during = await readDetailViewportAnchor(page);
    expect(during.src).toContain('size=large');
    expect(Math.abs(during.centerRatioX - before.centerRatioX)).toBeLessThan(0.001);
    expect(Math.abs(during.centerRatioY - before.centerRatioY)).toBeLessThan(0.001);

    releaseOriginalResponse();

    await page.waitForFunction(
      ([el, minWidth]) =>
        el instanceof HTMLImageElement &&
        el.complete &&
        el.naturalWidth > Number(minWidth) &&
        (el.currentSrc || el.src).includes('size=original'),
      [await img.elementHandle(), before.naturalWidth],
      { timeout: 30_000 }
    );

    await expect(page.locator('.image-loading-overlay')).toHaveCount(0, {
      timeout: 10_000,
    });

    const after = await readDetailViewportAnchor(page);
    expect(after.src).toContain('size=original');
    expect(after.naturalWidth).toBeGreaterThan(before.naturalWidth);
    expect(Math.abs(after.centerRatioX - before.centerRatioX)).toBeLessThan(0.02);
    expect(Math.abs(after.centerRatioY - before.centerRatioY)).toBeLessThan(0.02);
  } finally {
    releaseOriginalResponse();
    await page.unroute('**/images/1/preview**');
  }
});
