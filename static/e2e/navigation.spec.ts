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
  // Wait for the background cache refresh to settle so has_files flips true
  // and the View Project link actually shows up on the overview.
  await waitForCacheReady(request, dbId);
});

test('overview groups projects under the configured DB section', async ({
  page,
}) => {
  await page.goto('/');
  // The DB section header carries the display name. Use a regex match because
  // the h2 also contains the slug badge and project count.
  await expect(
    page.getByRole('heading', { name: /Imaging Rig e2e/i })
  ).toBeVisible({ timeout: 15_000 });
  // Fixture defines two projects (Alpha + Beta) under this DB.
  await expect(page.getByText(/2 projects/)).toBeVisible();
  await expect(page.getByRole('heading', { name: /Project Alpha/i })).toBeVisible();
  await expect(page.getByRole('heading', { name: /Project Beta/i })).toBeVisible();
});

test('click View Project from overview lands on the grid scoped to that DB', async ({
  page,
}) => {
  await page.goto('/');
  await expect(
    page.getByRole('heading', { name: /Imaging Rig e2e/i })
  ).toBeVisible({ timeout: 15_000 });

  // Click the Alpha project's "View Project" link (first one in the list).
  await page.getByText('View Project →').first().click();

  // URL carries both the db slug and a project id atomically.
  await expect(page).toHaveURL(/[#?].*db=imaging-rig-e2e/);
  await expect(page).toHaveURL(/[#?].*project=\d+/);

  // With auto-expand on first data arrival, the cards mount directly.
  const firstCard = page.locator('.image-card').first();
  await expect(firstCard).toBeVisible({ timeout: 15_000 });
});

test('direct deep link to the grid loads when ?db= matches a configured DB', async ({
  page,
}) => {
  // Hash-router URLs: /#/grid?... GroupedImageGrid auto-expands its filter
  // groups the first time image data arrives, so a deep link with no
  // `expanded=` param should still show cards.
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card').first()).toBeVisible({
    timeout: 15_000,
  });
});

test('session grouping opens the newest time run and keeps its URL state intact', async ({
  page,
}) => {
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&grouping=session`
  );
  await page.getByRole('button', { name: 'Images', exact: true }).click();

  const sessionHeader = page.locator('.filter-header').first();
  await expect(sessionHeader).toContainText('Alpha M44 · B ·');
  await expect(page.locator('.image-card')).toHaveCount(3);

  // Session labels contain commas. Closing and reopening the group proves the
  // URL-backed expanded key survives round-tripping as one value.
  await sessionHeader.click();
  await expect(page.locator('.image-card')).toHaveCount(0);
  await sessionHeader.click();
  await expect(page.locator('.image-card')).toHaveCount(3);
});

test('multi-select stays active without changing the toolbar height', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const cards = page.locator('.image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  const controls = page.locator('.image-controls');
  const before = await controls.boundingBox();
  expect(before).not.toBeNull();

  await cards.nth(0).click();
  const additiveModifier: 'Meta' | 'Control' = process.platform === 'darwin' ? 'Meta' : 'Control';
  await cards.nth(1).click({ modifiers: [additiveModifier] });

  const actions = page.locator('.selection-action-bar');
  await expect(actions).toBeVisible();
  await expect(actions).toContainText('2 selected');
  await expect(page.locator('.stack-preview-heading p')).toContainText('2 selected images');

  const after = await controls.boundingBox();
  expect(after).not.toBeNull();
  expect(after!.height).toBeCloseTo(before!.height, 1);

  // The old filter effect cleared the selection on the next render.
  await page.waitForTimeout(250);
  await expect(actions).toBeVisible();
  await expect(page.locator('.image-card-wrapper.multi-selected')).toHaveCount(2);

  await cards.nth(2).click({ modifiers: ['Shift'] });
  await expect(actions).toContainText('3 selected');
  await expect(page.locator('.image-card-wrapper.multi-selected')).toHaveCount(3);
});

test('arrow keys move through the image grid', async ({ page }) => {
  await page.setViewportSize({ width: 760, height: 720 });
  await page.goto(
    `/#/grid?db=${encodeURIComponent(dbId)}&project=1&size=300`
  );

  const wrappers = page.locator('.image-card-wrapper');
  await expect(wrappers).toHaveCount(3, { timeout: 15_000 });
  await expect(wrappers.nth(0)).toHaveClass(/current-selection/);

  // At this width the third card is below the first two. Vertical movement
  // follows the nearest card in the next row; horizontal movement follows
  // image order.
  await page.keyboard.press('ArrowDown');
  await expect(wrappers.nth(2)).toHaveClass(/current-selection/);
  const thirdId = await wrappers.nth(2).getAttribute('data-image-id');
  expect(thirdId).not.toBeNull();
  await expect(page).toHaveURL(new RegExp(`current=${thirdId}`));
  await expect(page).not.toHaveURL(/selected=/);
  await expect(page).not.toHaveURL(/(?:groupIndex|imageIndex)=/);

  await page.keyboard.press('ArrowUp');
  await expect(wrappers.nth(0)).toHaveClass(/current-selection/);

  await page.keyboard.press('ArrowRight');
  await expect(wrappers.nth(1)).toHaveClass(/current-selection/);

  await page.keyboard.press('ArrowLeft');
  await expect(wrappers.nth(0)).toHaveClass(/current-selection/);
});

test('Space toggles the current image without losing keyboard selection', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const wrappers = page.locator('.image-card-wrapper');
  await expect(wrappers).toHaveCount(3, { timeout: 15_000 });
  await expect(wrappers.nth(0)).toHaveClass(/current-selection/);

  await page.keyboard.press('Space');
  await expect(wrappers.nth(0)).toHaveClass(/multi-selected/);

  await page.keyboard.press('ArrowRight');
  await expect(wrappers.nth(1)).toHaveClass(/current-selection/);
  await expect(wrappers.nth(0)).toHaveClass(/multi-selected/);

  await page.keyboard.press('Space');
  await expect(page.locator('.image-card-wrapper.multi-selected')).toHaveCount(2);
  await expect(page.locator('.selection-action-bar')).toContainText('2 selected');

  await page.keyboard.press('Space');
  await expect(wrappers.nth(1)).not.toHaveClass(/multi-selected/);
  await expect(wrappers.nth(0)).toHaveClass(/multi-selected/);
});

test('focused grid controls keep arrow and Space keystrokes', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const wrappers = page.locator('.image-card-wrapper');
  await expect(wrappers).toHaveCount(3, { timeout: 15_000 });
  await expect(wrappers.nth(0)).toHaveClass(/current-selection/);

  const search = page.getByLabel('Search:');
  await search.focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Space');

  await expect(search).toHaveValue(' ');
  await expect(wrappers.nth(0)).toHaveClass(/current-selection/);
  await expect(page.locator('.image-card-wrapper.multi-selected')).toHaveCount(0);
});

test('image-ID cursor and selection survive regrouping and reload', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const wrappers = page.locator('.image-card-wrapper');
  await expect(wrappers).toHaveCount(3, { timeout: 15_000 });
  const firstId = await wrappers.nth(0).getAttribute('data-image-id');
  const secondId = await wrappers.nth(1).getAttribute('data-image-id');
  expect(firstId).not.toBeNull();
  expect(secondId).not.toBeNull();

  await page.keyboard.press('Space');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Space');
  await expect(page.locator('.image-card-wrapper.multi-selected')).toHaveCount(2);

  await page.locator('.grouping-control select').selectOption('session');
  const current = page.locator(`[data-image-id="${secondId}"]`);
  await expect(current).toHaveClass(/current-selection/);
  await expect(page.locator(`[data-image-id="${firstId}"]`)).toHaveClass(/multi-selected/);
  await expect(current).toHaveClass(/multi-selected/);

  await page.reload();
  await expect(page.locator('.image-card-wrapper')).toHaveCount(3, { timeout: 15_000 });
  await expect(current).toHaveClass(/current-selection/);
  await expect(page.locator('.image-card-wrapper.multi-selected')).toHaveCount(2);
});

test('grid shortcuts do not leak into the detail overlay', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const wrappers = page.locator('.image-card-wrapper');
  await expect(wrappers).toHaveCount(3, { timeout: 15_000 });
  const firstId = await wrappers.nth(0).getAttribute('data-image-id');
  const secondId = await wrappers.nth(1).getAttribute('data-image-id');
  expect(firstId).not.toBeNull();
  expect(secondId).not.toBeNull();

  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(new RegExp(`#/detail/${firstId}`));

  await page.keyboard.press('Space');
  await expect(page).not.toHaveURL(/selected=/);

  await page.keyboard.press('ArrowRight');
  await expect(page).toHaveURL(new RegExp(`#/detail/${secondId}`));
  await expect(page).toHaveURL(new RegExp(`current=${firstId}`));
});

test('sequence mode uses arrows and Space for the same keyboard selection flow', async ({ page }) => {
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`
  );

  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  await expect(cards.nth(0)).toHaveClass(/current-selection/);

  await page.keyboard.press('Space');
  await expect(cards.nth(0)).toHaveClass(/selected/);

  await page.keyboard.press('ArrowRight');
  await expect(cards.nth(1)).toHaveClass(/current-selection/);
  await expect(cards.nth(0)).toHaveClass(/selected/);

  await page.keyboard.press('Space');
  await expect(page.locator('.sequence-image-card.selected')).toHaveCount(2);

  await page.keyboard.press('ArrowUp');
  await expect(cards.nth(0)).toHaveClass(/current-selection/);
});

test('sequence keyboard shortcuts pause while the rejection dialog is open', async ({ page }) => {
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`
  );

  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  await page.keyboard.press('Space');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Space');
  await expect(page.locator('.sequence-image-card.selected')).toHaveCount(2);
  await expect(cards.nth(1)).toHaveClass(/current-selection/);

  await page.getByRole('button', { name: 'Review rejection' }).click();
  const dialog = page.getByRole('dialog', { name: /Review 2 recommended rejections/ });
  await expect(dialog).toBeVisible();
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());

  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Space');
  await expect(cards.nth(1)).toHaveClass(/current-selection/);
  await expect(page.locator('.sequence-image-card.selected')).toHaveCount(2);

  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await page.keyboard.press('ArrowRight');
  await expect(cards.nth(2)).toHaveClass(/current-selection/);
});

test('small header wraps without horizontal overflow', async ({ page }) => {
  await page.setViewportSize({ width: 420, height: 720 });
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const header = page.locator('.app-header');
  const brand = page.locator('.header-brand');
  const tabs = page.locator('.header-view-tabs');
  await expect(header).toBeVisible();

  const [brandBox, tabsBox, headerWidth] = await Promise.all([
    brand.boundingBox(),
    tabs.boundingBox(),
    header.evaluate((element) => ({
      client: element.clientWidth,
      scroll: element.scrollWidth,
    })),
  ]);
  expect(brandBox).not.toBeNull();
  expect(tabsBox).not.toBeNull();
  expect(tabsBox!.y).toBeGreaterThan(brandBox!.y);
  expect(headerWidth.scroll).toBeLessThanOrEqual(headerWidth.client);
});

test('scoped view without ?db= renders no image cards', async ({ page }) => {
  // Loading /grid with no db param means the route can't resolve a database.
  // The query is gated on !!dbId && !!projectId, so no .image-card renders.
  await page.goto('/#/grid');
  await expect(page.locator('.image-card')).toHaveCount(0);
});
