import { expect, test } from '@playwright/test';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

// The Images tab's Status filter against the real server. Project Alpha's
// three frames are graded Pending, Accepted, Pending, so each choice has a
// known answer.

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

test('the Status boxes keep any mix of grades and say so in the summary', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  const cards = page.locator('.image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  const box = (name: string) => page.getByRole('checkbox', { name });
  // The boxes follow the address, which changes a moment after the click,
  // so each click waits for the box rather than check() reading it at once.
  const set = async (name: string, on: boolean) => {
    await box(name).click();
    await expect(box(name)).toBeChecked({ checked: on });
  };
  const stats = page.locator('.grid-stats');

  // Everything but rejected: one box off.
  await set('Rejected', false);
  await expect(cards).toHaveCount(3);
  await expect(page).toHaveURL(/status=accepted(%2C|,)pending/);
  await expect(stats).toContainText('Accepted and Pending');

  await set('Pending', false);
  await expect(cards).toHaveCount(1);
  await expect(stats).toContainText('1 of 3 images');
  await expect(stats).toContainText('Accepted');
  await expect(page).toHaveURL(/status=accepted(?!%2C|,)/);

  await set('Pending', true);
  await set('Accepted', false);
  await expect(cards).toHaveCount(2);
  await expect(stats).toContainText('2 of 3 images');

  await set('Rejected', true);
  await set('Pending', false);
  await expect(cards).toHaveCount(0);
  await expect(page.getByText('No images found')).toBeVisible();

  // The last box unticked is All again, not an empty grid.
  await box('Rejected').click();
  await expect(cards).toHaveCount(3);
  for (const name of ['Accepted', 'Rejected', 'Pending']) await expect(box(name)).toBeChecked();
  await expect(page).not.toHaveURL(/status=/);

  await set('Rejected', false);
  await page.getByRole('button', { name: 'Reset' }).click();
  await expect(cards).toHaveCount(3);
  await expect(box('Rejected')).toBeChecked();
});

test('a link saved with the old numeric status still filters', async ({ page }) => {
  // Before the fix the select emitted the grade number, so shared links
  // carry ?status=1. They must keep meaning Accepted.
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&status=1`);
  await expect(page.locator('.image-card')).toHaveCount(1, { timeout: 15_000 });
  await expect(page.getByRole('checkbox', { name: 'Accepted' })).toBeChecked();
  await expect(page.getByRole('checkbox', { name: 'Pending' })).not.toBeChecked();
});

test('a settled grid runs no animations', async ({ page }) => {
  // Every card once carried a hidden spinner that turned forever. With
  // thousands of cards the browser restyled each one on every frame, and
  // scrolling fell to ten frames a second.
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card')).toHaveCount(3, { timeout: 15_000 });
  await expect(page.locator('.filter-images .loading-spinner')).toHaveCount(0, { timeout: 30_000 });
  await expect
    .poll(() => page.evaluate(() => document.getAnimations()
      .filter((animation) => {
        const target = (animation.effect as KeyframeEffect | null)?.target;
        return target instanceof Element && target.closest('.filter-images') !== null;
      })
      .map((animation) => (animation as CSSAnimation).animationName ?? 'transition')))
    .toEqual([]);
});

test('the toolbar keeps its shape: one height for every control, and a long choice moves nothing', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card')).toHaveCount(3, { timeout: 15_000 });
  const bar = page.locator('.image-controls').first();
  const shape = () => bar.evaluate((element) => ({
    height: Math.round(element.getBoundingClientRect().height),
    controls: [...element.querySelectorAll('button, select, input[type="text"]')]
      .filter((control) => (control as HTMLElement).offsetParent && !control.closest('.selection-action-bar, .multi-select-menu'))
      .map((control) => Math.round(control.getBoundingClientRect().height)),
  }));
  const plain = await shape();
  expect(new Set(plain.controls)).toEqual(new Set([30]));
  // Long filter and flag choices are cut short, not given room.
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&filter=Ha,OIII,SII&flag=stray_light_gradient,satellite_trail`);
  await expect(page.getByRole('button', { name: /^Flag:/ })).toContainText('Stray Light');
  const chosen = await shape();
  expect(chosen.height).toBe(plain.height);
  expect(new Set(chosen.controls)).toEqual(new Set([30]));
});
