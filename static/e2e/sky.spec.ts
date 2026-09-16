import { expect, test } from '@playwright/test';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

let dbId: string;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, { name: 'Sky Rig', slug: 'sky-rig' });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

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

  const scrubber = page.locator('.sky-scrubber');
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
  }

  await page.getByRole('radio', { name: 'Galactic' }).click();
  await expect(page.locator('.sky-map')).toHaveAttribute('aria-label', /galactic/);
  await expect(page.locator('.sky-target')).toHaveCount(2);
});
