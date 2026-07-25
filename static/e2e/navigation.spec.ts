import { expect, test, type Route } from '@playwright/test';
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
  // and the image-grid action becomes available on the overview.
  await waitForCacheReady(request, dbId);
});

test('overview groups projects by activity and labels their database', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByText('Imaging Rig e2e')).toHaveCount(2);
  await expect(
    page.getByRole('button', { name: 'Open Project Alpha image grid' })
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Open Project Beta image grid' })
  ).toBeVisible();
});

test('open a project image grid from overview with the correct DB scope', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Earlier work' })).toBeVisible({
    timeout: 15_000,
  });

  await page
    .getByRole('button', { name: 'Open Project Alpha image grid' })
    .click();

  // URL carries both the db slug and a project id atomically.
  await expect(page).toHaveURL(/[#?].*db=imaging-rig-e2e/);
  await expect(page).toHaveURL(/[#?].*project=\d+/);

  // With auto-expand on first data arrival, the cards mount directly.
  const firstCard = page.locator('.image-card').first();
  await expect(firstCard).toBeVisible({ timeout: 15_000 });
});

test('project and target menus filter as the user types', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card')).toHaveCount(3, { timeout: 15_000 });

  await page.locator('#project-select').click();
  const stackingLayers = await page.evaluate(() => ({
    header: Number.parseInt(getComputedStyle(document.querySelector('.app-header')!).zIndex, 10),
    controls: Number.parseInt(
      getComputedStyle(document.querySelector('.image-controls.sticky')!).zIndex,
      10
    ),
  }));
  expect(stackingLayers.header).toBeGreaterThan(stackingLayers.controls);
  const projectSearch = page.getByLabel('Search projects');
  await projectSearch.fill('Beta');
  const beta = page
    .locator('.project-selector-popover')
    .getByRole('option', { name: /Project Beta/ });
  await expect(beta).toBeVisible();
  await expect(
    page.locator('.project-selector-popover').getByRole('option', {
      name: /Project Alpha/,
    })
  ).toHaveCount(0);
  await beta.click();
  await expect(page).toHaveURL(new RegExp(`db=${dbId}.*project=2`));

  await page.locator('#target-select').click();
  await page.getByLabel('Search targets').fill('Beta Field');
  await page
    .locator('.target-selector-popover')
    .getByRole('option', { name: /Beta Field/ })
    .click();
  await expect(page).toHaveURL(new RegExp('target=2'));
});

test('recent projects rise to a highlighted group', async ({ page }) => {
  const now = Math.floor(Date.now() / 1000);
  await page.route(`**/api/db/${dbId}/projects/overview`, async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    const beta = body.data.find((project: { id: number }) => project.id === 2);
    beta.date_range.latest = now;
    beta.recent_images[0].acquired_date = now;
    await route.fulfill({ response, json: body });
  });

  await page.goto('/');
  const recentGroup = page.locator('.project-activity-group.is-recent');
  await expect(
    recentGroup.getByRole('heading', { name: 'Worked on this week' })
  ).toBeVisible({ timeout: 15_000 });
  await expect(recentGroup).toContainText('Project Beta');
  await expect(recentGroup).not.toContainText('Project Alpha');
  await expect(recentGroup.getByText('Recent', { exact: true })).toHaveCount(2);
});

test('closed projects stay in collapsed archives', async ({ page }) => {
  const markBetaClosed = async (route: Route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.data.find((project: { id: number }) => project.id === 2).state = 3;
    await route.fulfill({ response, json: body });
  };
  await page.route(`**/api/db/${dbId}/projects/overview`, markBetaClosed);
  await page.route(`**/api/db/${dbId}/projects`, markBetaClosed);

  await page.goto('/');
  await expect(
    page.getByRole('button', { name: 'Open Project Alpha image grid' })
  ).toBeVisible({ timeout: 15_000 });
  await expect(
    page.getByRole('button', { name: 'Open Project Beta image grid' })
  ).toHaveCount(0);

  const overviewArchive = page.getByRole('button', { name: /Archived projects.*1/ });
  await expect(overviewArchive).toHaveAttribute('aria-expanded', 'false');
  await overviewArchive.click();
  await expect(page.locator('.project-archive-item')).toContainText('Project Beta');

  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await page.locator('#project-select').click();
  const selectorArchive = page.locator('.selector-archive');
  await expect(selectorArchive).not.toHaveAttribute('open', '');
  await selectorArchive.locator('summary').click();
  await expect(
    selectorArchive.getByRole('option', { name: /Project Beta/ })
  ).toBeVisible();
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
