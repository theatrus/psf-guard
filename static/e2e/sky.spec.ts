import { expect, test, type Page } from '@playwright/test';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

let dbId: string;

test.beforeEach(async ({ request, page }) => {
  // Keep the banner layout deterministic instead of depending on a live release.
  await page.route('**/api/update-notice', (route) => route.fulfill({ json: {
    success: true,
    data: {
      notice: {
        schema_version: 1,
        version: '999.0.0',
        release_url: 'https://github.com/theatrus/psf-guard/releases/latest',
        summary: 'A test release with an update notice above the sky map.',
        urgency: 'normal',
        minimum_supported_version: '0.0.0',
        published_at: '2026-07-26T18:00:00Z',
      },
      checking: false,
      checked_at_unix_seconds: 1_774_806_400,
    },
    error: null,
    status: 'ready',
  } }));
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, { name: 'Sky Rig', slug: 'sky-rig' });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

async function dragMap(page: Page, dx: number, dy: number) {
  const map = page.locator('.sky-map');
  await expect(page.locator('.update-notice')).toBeVisible();
  // Raw mouse actions do not scroll their target into view like locator clicks.
  await map.scrollIntoViewIfNeeded();
  const box = await map.boundingBox();
  if (!box) throw new Error('no map');
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  const endX = x + box.width * dx;
  const endY = y + box.height * dy;
  const viewport = page.viewportSize();
  if (!viewport) throw new Error('no viewport');
  for (const [point, limit] of [[x, viewport.width], [endX, viewport.width], [y, viewport.height], [endY, viewport.height]]) {
    expect(point).toBeGreaterThan(0);
    expect(point).toBeLessThan(limit);
  }
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(endX, endY, { steps: 8 });
  await page.mouse.up();
}

test('the sky page maps every target, tells its story on hover, and opens it in Images', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Sky' }).click();
  await expect(page).toHaveURL(/#\/sky/);

  const hero = page.locator('.sky-hero');
  await expect(hero).toBeVisible({ timeout: 15_000 });
  await expect(hero.locator('[data-stat="frames"]')).toHaveText('4');
  await expect(hero.locator('[data-stat="targets"]')).toHaveText('2');

  const targets = page.locator('.sky-target');
  await expect(targets).toHaveCount(2);

  const alpha = page.locator('.sky-target[data-target="Alpha M44"]');
  await alpha.hover();
  const card = page.locator('.sky-card');
  await expect(card).toBeVisible();
  await expect(card).toContainText('Alpha M44');
  await expect(card).toContainText('Sky Rig · Project Alpha');
  await expect(card).toContainText('3 frames');

  await alpha.click();
  await expect(page).toHaveURL(new RegExp(`#/grid\\?db=${dbId}&project=1&target=1`));
});

test('the timeline scrubs the map back through the nights', async ({ page }) => {
  await page.goto('/#/sky');
  await expect(page.locator('.sky-timeline')).toBeVisible({ timeout: 15_000 });
  await expect(page.locator('.sky-lane-name').first()).toHaveText('Sky Rig');

  const scrubber = page.locator('.sky-scrubber-to');
  const start = page.locator('.sky-scrubber-from');
  const nights = Number(await scrubber.getAttribute('max')) + 1;
  expect(nights).toBeGreaterThanOrEqual(1);
  await expect(page.locator('.sky-timeline-asof')).toContainText('every night');

  if (nights > 1) {
    await scrubber.fill('0');
    await expect(page.locator('.sky-timeline-asof')).toContainText('as of');
    const shownEarly = await page.locator('.sky-target').count();
    expect(shownEarly).toBeLessThanOrEqual(2);
    await scrubber.fill(String(nights - 1));
    await expect(page.locator('.sky-timeline-asof')).toContainText('every night');
    // The start thumb cuts the early nights away instead.
    await start.fill(String(nights - 1));
    await expect(page.locator('.sky-timeline-asof')).toContainText('from ');
    await start.fill('0');
    await expect(page.locator('.sky-timeline-asof')).toContainText('every night');
  }

  await page.getByRole('radio', { name: 'Galactic' }).click();
  await expect(page.locator('.sky-map')).toHaveAttribute('aria-label', /galactic/);
  await expect(page.locator('.sky-target')).toHaveCount(2);
});

test('the map zooms, shows constellations, and stays whole-sky by default', async ({ page }) => {
  await page.goto('/#/sky');
  const map = page.locator('.sky-map');
  await expect(map).toBeVisible({ timeout: 15_000 });
  await expect(map).toHaveAttribute('data-zoom', '1.00');
  await expect(page.locator('.sky-constellations path').first()).toBeAttached();
  await expect(page.locator('.sky-constellation-names text', { hasText: 'Cancer' })).toBeAttached();

  await page.getByRole('button', { name: 'Zoom in' }).click();
  await page.getByRole('button', { name: 'Zoom in' }).click();
  await expect(map).toHaveAttribute('data-zoom', '2.56');
  await expect(page.locator('.sky-target')).toHaveCount(2);

  await page.getByRole('button', { name: 'Whole sky' }).click();
  await expect(map).toHaveAttribute('data-zoom', '1.00');

  await page.getByLabel('Constellations').uncheck();
  await expect(page.locator('.sky-constellations')).toHaveCount(0);
});

test('the sky remembers its turn and zoom while you are away in another view', async ({ page }) => {
  await page.goto('/#/sky');
  const map = page.locator('.sky-map');
  await expect(map).toBeVisible({ timeout: 15_000 });
  await page.getByRole('button', { name: 'Zoom in' }).click();
  await page.getByRole('button', { name: 'Zoom in' }).click();
  await expect(map).toHaveAttribute('data-zoom', '2.56');
  await dragMap(page, 1 / 8, 1 / 8);
  await expect(map).not.toHaveAttribute('data-center', '180.0,0.0');
  await page.getByRole('radio', { name: 'Galactic' }).click();
  await expect(map).toHaveAttribute('data-zoom', '1.00');
  await page.getByRole('button', { name: 'Zoom in' }).click();

  await page.getByRole('button', { name: 'Library', exact: true }).click();
  await expect(page).toHaveURL(/#\/(\?|$)/);
  await page.getByRole('button', { name: 'Sky' }).click();
  await expect(page.locator('.sky-map')).toBeVisible({ timeout: 15_000 });
  await expect(page.locator('.sky-map')).toHaveAttribute('data-zoom', '1.60');
  await expect(page.locator('.sky-map')).toHaveAttribute('aria-label', /galactic/);
  await expect(page.getByRole('radio', { name: 'Galactic' })).toHaveAttribute('aria-checked', 'true');
});

test('the globe turns when dragged and the flat map spins its central meridian', async ({ page }) => {
  await page.goto('/#/sky');
  const map = page.locator('.sky-map');
  await expect(map).toBeVisible({ timeout: 15_000 });
  await expect(map).toHaveAttribute('data-center', '180.0,0.0');

  const dustBefore = await page.locator('.sky-stars circle').first().getAttribute('cx');
  await dragMap(page, 1 / 4, 0);
  await expect(map).toHaveAttribute('data-center', '270.0,0.0');
  // The stars turn with the sky: nothing on the map stays put.
  expect(await page.locator('.sky-stars circle').first().getAttribute('cx')).not.toBe(dustBefore);
  // A vertical drag tilts the flat map too: the viewer is inside the sphere.
  await dragMap(page, 0, 1 / 4);
  await expect(map).toHaveAttribute('data-center', '270.0,45.0');
  // Zoomed in, a drag still turns the sky rather than sliding a picture.
  await page.getByRole('button', { name: 'Zoom in' }).click();
  await expect(map).toHaveAttribute('data-zoom', '1.60');
  await dragMap(page, -1 / 8, 0);
  await expect(map).toHaveAttribute('data-center', /^241\.9,45\.0$/);
  await page.getByRole('button', { name: 'Whole sky' }).click();
  await expect(map).toHaveAttribute('data-center', '180.0,0.0');

  await page.getByRole('radio', { name: 'Globe' }).click();
  await expect(map).toHaveAttribute('data-center', '180.0,25.0');
  await dragMap(page, 0, -1 / 4);
  await expect(map).toHaveAttribute('data-center', '180.0,-5.0');
  await expect(page.locator('.sky-target')).toHaveCount(2);

  await page.getByRole('button', { name: 'Whole sky' }).click();
  await expect(map).toHaveAttribute('data-center', '180.0,25.0');
});
