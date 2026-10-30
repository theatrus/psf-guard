import { expect, test, type Page } from '@playwright/test';
import * as path from 'path';
import {
  fixtureImageDir,
  registerFixtureDb,
  resetDatabases,
  stubPixelWork,
  waitForCacheReady,
} from './helpers';

let dbId: string;
let slugCounter = 0;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  slugCounter += 1;
  const entry = await registerFixtureDb(request, {
    name: `Imaging Rig e2e ${slugCounter}`,
    // A replacement Playwright worker must not reuse a failed worker's cache.
    slug: `imaging-rig-e2e-${process.pid}-${slugCounter}`,
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
  // Each card's preview may still be generating on a cold cache; give every
  // card its own wait rather than one for the lot.
  test.setTimeout(240_000);
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const cards = page.locator('.image-card');
  const firstCard = cards.first();
  await expect(firstCard).toBeVisible({ timeout: 15_000 });

  // Stack previews can put the grid below the initial viewport. Image cards
  // defer their preview until they intersect the viewport (plus root margin),
  // so scroll to the card before expecting its <img> to exist.
  await firstCard.scrollIntoViewIfNeeded();

  // The src should be db-nested at `/api/db/<slug>/images/<id>/preview`.
  const firstImg = firstCard.locator('img').first();
  await expect(firstImg).toBeAttached({ timeout: 15_000 });
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
    const card = cards.nth(i);
    await card.scrollIntoViewIfNeeded();
    const img = card.locator('img').first();
    await expect(img).toBeAttached({ timeout: 15_000 });
    // Found afresh on every poll: a card can swap its <img> once the
    // preview is ready, and a handle to the old one never loads. On a busy
    // CI runner the screen previews can queue behind a large preview of a
    // 9,576 px frame that takes 30 s on its own.
    await expect
      .poll(
        () =>
          img.evaluate((el) =>
            (el as HTMLImageElement).complete ? (el as HTMLImageElement).naturalWidth : 0
          ),
        { timeout: 90_000 }
      )
      .toBeGreaterThan(0);
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

test('detail view reads the value and RA/Dec under the pointer', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);
  const img = page.locator('.zoom-container img.detail-main-image').first();
  await page.waitForFunction(
    (el) => el instanceof HTMLImageElement && el.complete && el.naturalWidth > 0,
    await img.elementHandle(),
    { timeout: 30_000 }
  );

  // This frame's embedded WCS gives RA/Dec without a solve.
  const toggles = page.getByRole('group', { name: 'Pointer readout' });
  await expect(toggles.getByRole('button', { name: 'RA/Dec' })).toBeEnabled({ timeout: 30_000 });
  await page.keyboard.press('m');
  await page.keyboard.press('r');
  await expect(toggles.getByRole('button', { name: 'Value' })).toHaveAttribute('aria-pressed', 'true');
  await expect(toggles.getByRole('button', { name: 'RA/Dec' })).toHaveAttribute('aria-pressed', 'true');

  const box = (await page.locator('.zoom-container').first().boundingBox())!;
  await page.mouse.move(box.x + box.width * 0.45, box.y + box.height * 0.45);
  const readout = page.getByTestId('pointer-readout');
  await expect(readout.getByTestId('pointer-readout-value')).toHaveText(/^-?\d/, { timeout: 15_000 });
  await expect(readout.getByTestId('pointer-readout-sky')).toHaveText(/^RA 08h\d\dm.* Dec \+\d\d°/);

  if (process.env.PSF_GUARD_CAPTURE_DOCS === '1') {
    await page.screenshot({
      path: path.resolve(process.cwd(), '..', 'docs', 'images', 'image-detail-readout-desktop.png'),
      animations: 'disabled',
    });
  }
});

test('detail panels fold and stay folded from image to image and across a reload', async ({ page }) => {
  // About the sidebar, not the pixels: keep the preview queue free for the
  // timed preview tests below.
  await stubPixelWork(page);
  const calibrationAsked: string[] = [];
  page.on('request', (request) => {
    if (/\/images\/\d+\/calibration/.test(request.url())) calibrationAsked.push(request.url());
  });
  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);

  const info = page.getByRole('button', { name: /^Image Information/ });
  const sky = page.getByRole('button', { name: /^Sky context/ });
  const calibration = page.getByTestId('calibration-panel').getByRole('button', { name: /^Calibration/ });
  await expect(sky).toHaveAttribute('aria-expanded', 'true', { timeout: 15_000 });
  await expect(info).toHaveAttribute('aria-expanded', 'true');

  await info.click();
  await sky.click();
  // Calibration starts open or folded by the light's marks; a click makes it
  // the viewer's choice either way.
  await calibration.click();
  if ((await calibration.getAttribute('aria-expanded')) === 'true') await calibration.click();
  await expect(info).toHaveAttribute('aria-expanded', 'false');
  await expect(sky).toHaveAttribute('aria-expanded', 'false');
  await expect(calibration).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByTestId('image-file-location')).toHaveCount(0);
  // Folded, the heading still says something.
  await expect(sky).toHaveAccessibleName(/^Sky context .+/);

  await page.keyboard.press('ArrowRight');
  await expect(page).toHaveURL(/\/detail\/2/);
  await expect(info).toHaveAttribute('aria-expanded', 'false');
  await expect(sky).toHaveAttribute('aria-expanded', 'false');
  await expect(calibration).toHaveAttribute('aria-expanded', 'false');

  await page.reload();
  await expect(sky).toHaveAttribute('aria-expanded', 'false', { timeout: 15_000 });
  await expect(calibration).toHaveAttribute('aria-expanded', 'false');
  // A folded Calibration panel asks for nothing about the lights it passes.
  expect(calibrationAsked.filter((url) => url.includes('/images/2/calibration'))).toEqual([]);

  await sky.click();
  await expect(sky).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByTestId('astrometry-panel').getByRole('button').nth(1)).toBeVisible();
});

test('detail view reveals the resolved file path on demand', async ({ page }) => {
  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);

  const disclosure = page.getByTestId('image-file-location');
  await expect(disclosure).toBeVisible({ timeout: 15_000 });

  const filePath = page.getByTestId('image-file-path');
  await expect(filePath).toBeHidden();

  await disclosure.getByText('File path').click();
  await expect(filePath).toBeVisible();
  await expect(filePath).toContainText(fixtureImageDir());
  await expect(filePath).toContainText('.fits');
  await expect(page.getByRole('button', { name: 'Copy path' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Show in folder' })).toHaveCount(0);
});

test('detail view supports pinch zoom around a moving midpoint', async ({
  page,
}) => {
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

  const before = await readDetailViewportAnchor(page);
  const box = await container.boundingBox();
  expect(box).not.toBeNull();
  expect(await container.evaluate((el) => getComputedStyle(el).touchAction))
    .toBe('none');

  const startCenter = {
    x: box!.x + box!.width / 2,
    y: box!.y + box!.height / 2,
  };
  const movedCenter = {
    x: startCenter.x + 30,
    y: startCenter.y + 20,
  };

  await container.evaluate((el, center) => {
    const event = new Event('touchstart', {
      bubbles: true,
      cancelable: true,
    });
    Object.defineProperty(event, 'touches', {
      value: [
        { clientX: center.x - 80, clientY: center.y },
        { clientX: center.x + 80, clientY: center.y },
      ],
    });
    el.dispatchEvent(event);
  }, startCenter);
  await container.evaluate((el, center) => {
    const event = new Event('touchmove', {
      bubbles: true,
      cancelable: true,
    });
    Object.defineProperty(event, 'touches', {
      value: [
        { clientX: center.x - 120, clientY: center.y },
        { clientX: center.x + 120, clientY: center.y },
      ],
    });
    el.dispatchEvent(event);
  }, movedCenter);
  await container.evaluate((el) => {
    const event = new Event('touchend', {
      bubbles: true,
      cancelable: true,
    });
    Object.defineProperty(event, 'touches', { value: [] });
    el.dispatchEvent(event);
  });

  await expect.poll(async () => (await readDetailViewportAnchor(page)).scale)
    .toBeGreaterThan(before.scale * 1.45);

  const after = await readDetailViewportAnchor(page);
  const imageX =
    (box!.width / 2 - before.offsetX) / before.scale;
  const imageY =
    (box!.height / 2 - before.offsetY) / before.scale;
  expect(after.offsetX + imageX * after.scale)
    .toBeCloseTo(box!.width / 2 + 30, 1);
  expect(after.offsetY + imageY * after.scale)
    .toBeCloseTo(box!.height / 2 + 20, 1);
});

test('detail view keeps trackpad pinch out of the page viewport', async ({
  page,
}) => {
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

  const before = await readDetailViewportAnchor(page);
  const viewportScaleBefore = await page.evaluate(
    () => window.visualViewport?.scale ?? 1
  );
  const box = await container.boundingBox();
  expect(box).not.toBeNull();
  const center = {
    x: box!.x + box!.width / 2,
    y: box!.y + box!.height / 2,
  };

  const wheelCanceled = await container.evaluate((el, point) => {
    const event = new WheelEvent('wheel', {
      bubbles: true,
      cancelable: true,
      clientX: point.x,
      clientY: point.y,
      ctrlKey: true,
      deltaY: -200,
    });
    return !el.dispatchEvent(event) && event.defaultPrevented;
  }, center);

  expect(wheelCanceled).toBe(true);
  await expect.poll(async () => (await readDetailViewportAnchor(page)).scale)
    .toBeGreaterThan(before.scale);

  const afterWheel = await readDetailViewportAnchor(page);
  const gestureCanceled = await container.evaluate((el, point) => {
    const dispatchGesture = (type: string, scale: number) => {
      const event = new Event(type, { bubbles: true, cancelable: true });
      Object.defineProperties(event, {
        clientX: { value: point.x },
        clientY: { value: point.y },
        scale: { value: scale },
      });
      return !el.dispatchEvent(event) && event.defaultPrevented;
    };

    const start = dispatchGesture('gesturestart', 1);
    const change = dispatchGesture('gesturechange', 1.5);
    dispatchGesture('gestureend', 1.5);
    return start && change;
  }, center);

  expect(gestureCanceled).toBe(true);
  await expect.poll(async () => (await readDetailViewportAnchor(page)).scale)
    .toBeGreaterThan(afterWheel.scale * 1.4);
  expect(await page.evaluate(() => window.visualViewport?.scale ?? 1))
    .toBe(viewportScaleBefore);
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

  // Keep the original warm-up behind the initial bitmap load. On a cold
  // cache the original can otherwise finish before the large preview.
  let startOriginalRequest!: () => void;
  const startOriginalPromise = new Promise<void>((resolve) => {
    startOriginalRequest = resolve;
  });
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

    await startOriginalPromise;
    const response = await route.fetch();
    if (route.request().resourceType() === 'image' && response.status() === 200 && !originalHeld) {
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
    startOriginalRequest();

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
    startOriginalRequest();
    releaseOriginalResponse();
    await page.unrouteAll({ behavior: 'wait' });
  }
});

test('detail view preserves zoom and pan exactly across arrow-key navigation', async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);

  // Exercise equal-size bitmap navigation with both large previews ready.
  // A cold initial load can choose the original before the first fit settles.
  await Promise.all([1, 2].map((id) => expect.poll(async () => {
    const response = await request.get(`/api/db/${encodeURIComponent(dbId)}/images/${id}/preview?size=large&color=true`);
    return response.status();
  }, { timeout: 45_000 }).toBe(200)));

  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);

  const container = page.locator('.zoom-container').first();
  const img = container.locator('img.detail-main-image').first();
  await expect(img).toBeVisible({ timeout: 15_000 });
  await page.waitForFunction(
    (el) => el instanceof HTMLImageElement && el.complete && el.naturalWidth > 0,
    await img.elementHandle(),
    { timeout: 30_000 }
  );

  const box = await container.boundingBox();
  expect(box).not.toBeNull();
  const centerX = box!.x + box!.width / 2;
  const centerY = box!.y + box!.height / 2;

  // Zoom to a target below raw 1.0 so the original-resolution swap stays out
  // of the picture — this test is about EXACT persistence of the transform.
  const fitScale = (await readDetailViewportAnchor(page)).scale;
  const targetScale = Math.min(0.9, fitScale + 0.4);
  expect(targetScale).toBeGreaterThan(fitScale);
  await page.mouse.move(centerX, centerY);
  // handleWheel: newScale = scale - deltaY * 0.001
  await page.mouse.wheel(0, -(targetScale - fitScale) / 0.001);
  await page.mouse.down();
  await page.mouse.move(centerX - 90, centerY - 60, { steps: 5 });
  await page.mouse.up();

  const before = await readDetailViewportAnchor(page);
  expect(before.scale).toBeLessThan(1.0);
  expect(before.scale).toBeGreaterThan(fitScale + 0.01);
  const beforePercent = await page
    .locator('.zoom-percentage-compact')
    .textContent();

  await page.keyboard.press('ArrowRight');
  await expect(page).toHaveURL(/\/detail\/2/);

  // The main <img> is keyed by image id and REMOUNTS on navigation — an
  // element handle grabbed now may bind the detached old node, so re-query
  // the DOM on every poll instead.
  await page.waitForFunction(
    () => {
      const el = document.querySelector('.zoom-container img.detail-main-image');
      return (
        el instanceof HTMLImageElement &&
        el.complete &&
        el.naturalWidth > 0 &&
        (el.currentSrc || el.src).includes('/images/2/')
      );
    },
    undefined,
    { timeout: 60_000 }
  );
  await expect(img).not.toHaveClass(/detail-image-hidden/, { timeout: 10_000 });
  await expect(page.locator('.image-loading-overlay')).toHaveCount(0, {
    timeout: 10_000,
  });

  const after = await readDetailViewportAnchor(page);
  expect(after.src).toContain('/images/2/');
  // Identical bitmap dimensions in the sequence ⇒ the transform must carry
  // over EXACTLY: same scale, same offsets, same displayed percentage.
  expect(after.naturalWidth).toBe(before.naturalWidth);
  expect(after.scale).toBeCloseTo(before.scale, 5);
  expect(Math.abs(after.offsetX - before.offsetX)).toBeLessThan(0.5);
  expect(Math.abs(after.offsetY - before.offsetY)).toBeLessThan(0.5);
  const afterPercent = await page
    .locator('.zoom-percentage-compact')
    .textContent();
  expect(afterPercent).toBe(beforePercent);
});

test('detail view keeps the anchored region across navigation while zoomed past 100%', async ({
  page,
}) => {
  test.setTimeout(120_000);

  await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);

  const container = page.locator('.zoom-container').first();
  const img = container.locator('img.detail-main-image').first();
  await expect(img).toBeVisible({ timeout: 15_000 });
  await page.waitForFunction(
    (el) => el instanceof HTMLImageElement && el.complete && el.naturalWidth > 0,
    await img.elementHandle(),
    { timeout: 30_000 }
  );

  const box = await container.boundingBox();
  expect(box).not.toBeNull();
  const centerX = box!.x + box!.width / 2;
  const centerY = box!.y + box!.height / 2;

  // Zoom well past raw 1.0 and pan off-center; the view should upgrade to
  // the original-resolution artifact on its own.
  await page.mouse.move(centerX, centerY);
  await page.mouse.wheel(0, -1400);
  await page.mouse.down();
  await page.mouse.move(centerX - 120, centerY - 80, { steps: 6 });
  await page.mouse.up();

  await page.waitForFunction(
    (el) =>
      el instanceof HTMLImageElement &&
      el.complete &&
      el.naturalWidth > 0 &&
      (el.currentSrc || el.src).includes('size=original'),
    await img.elementHandle(),
    { timeout: 60_000 }
  );
  await expect(page.locator('.image-loading-overlay')).toHaveCount(0, {
    timeout: 10_000,
  });

  const before = await readDetailViewportAnchor(page);
  const beforeDisplayedWidth = before.scale * before.naturalWidth;
  const beforePercent = await page
    .locator('.zoom-percentage-compact')
    .textContent();

  await page.keyboard.press('ArrowRight');
  await expect(page).toHaveURL(/\/detail\/2/);

  // The next image must come back at the SAME anchored region and displayed
  // size, and re-upgrade to its own original-resolution artifact. Re-query
  // the DOM each poll — the keyed <img> remounts on navigation.
  await page.waitForFunction(
    () => {
      const el = document.querySelector('.zoom-container img.detail-main-image');
      const src = el instanceof HTMLImageElement ? el.currentSrc || el.src : '';
      return (
        el instanceof HTMLImageElement &&
        el.complete &&
        el.naturalWidth > 0 &&
        src.includes('/images/2/') &&
        src.includes('size=original')
      );
    },
    undefined,
    { timeout: 60_000 }
  );
  await expect(page.locator('.image-loading-overlay')).toHaveCount(0, {
    timeout: 10_000,
  });

  const after = await readDetailViewportAnchor(page);
  expect(after.naturalWidth).toBe(before.naturalWidth);
  const afterDisplayedWidth = after.scale * after.naturalWidth;
  expect(
    Math.abs(afterDisplayedWidth - beforeDisplayedWidth) / beforeDisplayedWidth
  ).toBeLessThan(0.01);
  expect(Math.abs(after.centerRatioX - before.centerRatioX)).toBeLessThan(0.02);
  expect(Math.abs(after.centerRatioY - before.centerRatioY)).toBeLessThan(0.02);
  // No top-left collapse: the anchored region stays away from the corner.
  expect(after.centerRatioX).toBeGreaterThan(0.1);
  expect(after.centerRatioY).toBeGreaterThan(0.1);
  const afterPercent = await page
    .locator('.zoom-percentage-compact')
    .textContent();
  expect(afterPercent).toBe(beforePercent);
});

test('detail view shows the image after the 202-generating path reloads with a cache-buster', async ({
  page,
}) => {
  // This case intentionally permits a 60-second image wait below. Give the
  // containing test enough headroom when the preview queue is still draining
  // original-resolution work from the preceding detail tests.
  test.setTimeout(90_000);

  // Observe the first large-preview GET returning 202 from the real server.
  // Every test gets a unique DB slug and therefore an uncached artifact. By
  // forwarding the request rather than fabricating the response, the server
  // also enqueues the generation job that the frontend will poll. It must ride
  // generating → status poll → ready → reload with a `v=` cache-buster — and
  // then actually reveal the image. Regression test: the `v=` reload used to
  // fail the "is this src current?" identity check (busted URL vs pristine
  // URL) and the loaded image stayed at opacity 0 with no overlay.
  // Hold the original warm-up so it cannot replace the large preview before
  // this test observes the cache-busted bitmap.
  let releaseOriginal!: () => void;
  const originalReleased = new Promise<void>((resolve) => {
    releaseOriginal = resolve;
  });
  let served202 = false;
  await page.route('**/images/1/preview**', async (route) => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('size') === 'original') {
      await originalReleased;
      await route.continue();
      return;
    }
    if (
      url.searchParams.get('size') !== 'large' ||
      url.searchParams.has('v') ||
      served202
    ) {
      await route.continue();
      return;
    }
    const response = await route.fetch();
    served202 = response.status() === 202;
    await route.fulfill({ response });
  });

  try {
    await page.goto(`/#/detail/1?db=${encodeURIComponent(dbId)}&project=1`);
    await expect
      .poll(() => served202, {
        message: 'fresh detail preview should enqueue with HTTP 202',
        timeout: 10_000,
      })
      .toBe(true);

    const img = page.locator('.zoom-container img.detail-main-image').first();
    // The post-ready reload carries the cache-buster and must fully load.
    await page.waitForFunction(
      (el) =>
        el instanceof HTMLImageElement &&
        el.complete &&
        el.naturalWidth > 0 &&
        new URL(el.currentSrc || el.src).searchParams.has('v'),
      await img.elementHandle(),
      { timeout: 60_000 }
    );

    await expect(img).not.toHaveClass(/detail-image-hidden/, {
      timeout: 10_000,
    });
    await expect(img).toHaveCSS('opacity', '1');
    await expect(page.locator('.image-loading-overlay')).toHaveCount(0, {
      timeout: 10_000,
    });
  } finally {
    releaseOriginal();
    if (!page.isClosed()) {
      await page.unrouteAll({ behavior: 'wait' });
    }
  }
});
