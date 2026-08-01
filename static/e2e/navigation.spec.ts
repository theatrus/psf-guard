import { expect, test, type Route } from '@playwright/test';
import type { ScoredSequence, TargetFilterRollup } from '../src/api/types';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

let dbId: string;

function compactTargetFilterRollup(sequence: ScoredSequence): TargetFilterRollup {
  return {
    target_id: sequence.target_id,
    target_name: sequence.target_name,
    filter_name: sequence.filter_name,
    session_start: sequence.session_start,
    session_end: sequence.session_end,
    image_count: sequence.image_count,
    unavailable_image_count: 0,
    images: sequence.images.map(image => ({
      image_id: image.image_id,
      quality_score: image.quality_score,
      normalized_metrics: image.normalized_metrics,
      details: image.details,
    })),
    summary: sequence.summary,
  };
}

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

test('combined project tree expands and finds targets as the user types', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card')).toHaveCount(3, { timeout: 15_000 });

  await page.locator('#scope-select').click();
  const picker = page.getByRole('dialog', { name: 'Choose a project or target' });
  await expect(picker).toBeVisible();
  const stackingLayers = await page.evaluate(() => ({
    header: Number.parseInt(getComputedStyle(document.querySelector('.app-header')!).zIndex, 10),
    controls: Number.parseInt(
      getComputedStyle(document.querySelector('.image-controls.sticky')!).zIndex,
      10
    ),
  }));
  expect(stackingLayers.header).toBeGreaterThan(stackingLayers.controls);
  await expect(picker.getByRole('button', { name: /^Alpha M44/ })).toBeVisible();
  await expect(picker.locator('[aria-current="true"]')).toHaveCount(1);
  await expect(picker.getByRole('button', { name: /^All images/ })).toHaveAttribute(
    'aria-current',
    'true'
  );

  const search = page.getByLabel('Search projects or targets');
  await search.fill('Beta Field');
  const betaProject = picker.getByRole('button', { name: /^Project Beta/ });
  await expect(betaProject).toBeVisible();
  await expect(picker.getByRole('button', { name: /^Project Alpha/ })).toHaveCount(0);
  await expect(betaProject).toHaveAttribute('aria-expanded', 'true');
  await betaProject.click();
  await expect(betaProject).toHaveAttribute('aria-expanded', 'false');
  await expect(picker.getByRole('button', { name: /^Beta Field/ })).toHaveCount(0);
  await betaProject.click();
  await picker.getByRole('button', { name: /^Beta Field/ }).click();
  await expect(page).toHaveURL(new RegExp(`db=${dbId}.*project=2.*target=2`));
});

test('combined project tree reports target loading failures', async ({ page }) => {
  await page.route(`**/api/db/${dbId}/targets`, (route) =>
    route.fulfill({ status: 500, json: { success: false, error: 'fixture failure' } })
  );

  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.image-card')).toHaveCount(3, { timeout: 15_000 });
  await page.locator('#scope-select').click();

  const picker = page.getByRole('dialog', { name: 'Choose a project or target' });
  await expect(picker.getByRole('alert')).toContainText('Some targets could not be loaded.', {
    timeout: 15_000,
  });
  await expect(picker.getByRole('button', { name: 'Retry' })).toBeVisible();
  await expect(picker.getByText('No matching targets.')).toHaveCount(0);
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
    recentGroup.getByRole('heading', { name: 'Last 7 days' })
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
  await page.locator('#scope-select').click();
  const selectorArchive = page.locator('.selector-archive');
  await expect(selectorArchive).not.toHaveAttribute('open', '');
  await selectorArchive.locator('summary').click();
  await expect(
    selectorArchive.getByRole('button', { name: /^Project Beta/ })
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

test('Grid Shift-click selects a range from a plain-click anchor', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const cards = page.locator('.image-card');
  const wrappers = page.locator('.image-card-wrapper');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  await cards.nth(0).click();
  await cards.nth(2).click({ modifiers: ['Shift'] });

  await expect(page.locator('.image-card-wrapper.multi-selected')).toHaveCount(3);
  await expect(page.locator('.selection-action-bar')).toContainText('3 selected');

  await cards.nth(1).click({ modifiers: ['Shift'] });
  await expect(page.locator('.image-card-wrapper.multi-selected')).toHaveCount(2);
  await expect(wrappers.nth(2)).not.toHaveClass(/multi-selected/);
});

test('Grid Shift-click resets an anchor from another project', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const cards = page.locator('.image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  await cards.first().click();

  await page.locator('#scope-select').click();
  const picker = page.getByRole('dialog', { name: 'Choose a project or target' });
  const betaProject = picker.locator('.selector-project-tree').filter({ hasText: 'Project Beta' });
  const betaToggle = betaProject.getByRole('button', { name: /^Project Beta/ });
  if (await betaToggle.getAttribute('aria-expanded') === 'false') await betaToggle.click();
  await betaProject.getByRole('button', { name: /^All images/ }).click();

  await expect(page).toHaveURL(new RegExp(`db=${dbId}.*project=2`));
  await expect(cards).toHaveCount(1);
  await cards.first().click({ modifiers: ['Shift'] });
  await expect(page.locator('.image-card-wrapper.multi-selected')).toHaveCount(1);
  await expect(page.locator('.selection-action-bar')).toContainText('1 selected');
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

  await page.keyboard.press('ArrowLeft');
  await expect(cards.nth(0)).toHaveClass(/current-selection/);
});

test('Sequence opens the only target in a project', async ({ page }) => {
  await page.goto(`/#/sequence?db=${encodeURIComponent(dbId)}&project=1`);

  await expect(page).toHaveURL(/target=1/);
  await expect(page.locator('.sequence-image-card')).toHaveCount(3, { timeout: 15_000 });
});

test('Sequence keeps the current Grid image and opens its session', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&size=300`);
  const gridCards = page.locator('.image-card-wrapper');
  await expect(gridCards).toHaveCount(3, { timeout: 15_000 });
  const chosenId = await gridCards.nth(1).getAttribute('data-image-id');
  expect(chosenId).not.toBeNull();
  await gridCards.nth(1).click();

  await page.getByRole('button', { name: 'Sequence', exact: true }).click();

  await expect(page).toHaveURL(/target=1/);
  await expect(page).toHaveURL(new RegExp(`current=${chosenId}`));
  await expect(
    page.locator(`.sequence-image-card[data-card-image-id="${chosenId}"]`)
  ).toHaveClass(/current-selection/);
});

test('Sequence keeps a Grid selection that spans session tabs', async ({ page }) => {
  const secondSessionStart = Math.floor(Date.UTC(2026, 3, 17, 0, 25, 0) / 1000);
  await page.route(`**/api/db/${dbId}/analysis/sequence*`, async (route) => {
    const response = await route.fetch();
    const body = await response.json() as {
      data: {
        sequences: ScoredSequence[];
        target_filter_rollups?: TargetFilterRollup[];
      };
    };
    const original = body.data.sequences[0];
    const [firstImage, ...laterImages] = original.images;
    body.data.target_filter_rollups = [compactTargetFilterRollup(original)];
    body.data.sequences = [
      {
        ...original,
        image_count: 1,
        images: [firstImage],
      },
      {
        ...original,
        session_start: secondSessionStart,
        session_end: secondSessionStart + 132,
        image_count: laterImages.length,
        images: laterImages,
      },
    ];
    await route.fulfill({ response, json: body });
  });

  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  const gridCards = page.locator('.image-card-wrapper');
  await expect(gridCards).toHaveCount(3, { timeout: 15_000 });
  await gridCards.nth(0).click();
  const additiveModifier: 'Meta' | 'Control' = process.platform === 'darwin' ? 'Meta' : 'Control';
  await gridCards.nth(2).click({ modifiers: [additiveModifier] });
  await expect(page.locator('.image-card-wrapper.multi-selected')).toHaveCount(2);

  await page.getByRole('button', { name: 'Sequence', exact: true }).click();

  await expect(page.locator('.sequence-tab')).toHaveCount(3, { timeout: 15_000 });
  await expect(page.locator('.sequence-tab-selection-count')).toHaveCount(3);
  await expect(page.locator('.sequence-selection-bar')).toContainText('2 selected');
  await expect(page.locator('.sequence-image-card.selected')).toHaveCount(1);
  await expect(page.locator('.sequence-tab').nth(2)).toHaveClass(/active/);

  await page.getByRole('button', { name: /All sessions/ }).click();
  await expect(page.locator('.sequence-image-card')).toHaveCount(3);
  await expect(page.locator('.sequence-score-context')).toContainText(
    'matching capture settings across all sessions'
  );
  await expect(page).toHaveURL(/scoreScope=target-filter%3A/);
});

test('Sequence vertical arrows follow the rendered thumbnail grid', async ({ page }) => {
  await page.setViewportSize({ width: 760, height: 720 });
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1&size=300`
  );

  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  await expect(cards.nth(0)).toHaveClass(/current-selection/);

  await page.keyboard.press('ArrowDown');
  await expect(cards.nth(2)).toHaveClass(/current-selection/);
  await page.keyboard.press('ArrowUp');
  await expect(cards.nth(0)).toHaveClass(/current-selection/);
});

test('Sequence Shift-click selects a visible range', async ({ page }) => {
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`
  );

  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  await cards.nth(2).click({ modifiers: ['Shift'] });

  await expect(page.locator('.sequence-image-card.selected')).toHaveCount(3);
  await expect(page.locator('.sequence-selection-bar')).toContainText('3 selected');

  await cards.nth(1).click({ modifiers: ['Shift'] });
  await expect(page.locator('.sequence-image-card.selected')).toHaveCount(2);
  await expect(cards.nth(2)).not.toHaveClass(/selected/);
});

test('Sequence keeps flagged image thumbnails at full opacity', async ({ page }) => {
  await page.route(`**/api/db/${dbId}/analysis/sequence*`, async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.data.sequences[0].images[0].quality_score = 0.2;
    body.data.sequences[0].images[0].category = 'likely_clouds';
    await route.fulfill({ response, json: body });
  });
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`
  );

  const flaggedCard = page.locator('.sequence-image-card.below-threshold').first();
  await expect(flaggedCard).toBeVisible({ timeout: 15_000 });
  await expect(flaggedCard).toHaveCSS('opacity', '1');
});

test('Sequence shows long score reasons in a popover without resizing the card', async ({ page }) => {
  const reason = 'Star count and background are outside the normal range for matching capture settings across all available sessions.';
  await page.route(`**/api/db/${dbId}/analysis/sequence*`, async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.data.sequences[0].images[0].details = reason;
    await route.fulfill({ response, json: body });
  });
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`
  );

  const card = page.locator('.sequence-image-card').first();
  await expect(card).toBeVisible({ timeout: 15_000 });
  const before = await card.boundingBox();
  await card.getByRole('button', { name: 'Show quality reason' }).click();

  const popover = page.getByRole('dialog', { name: 'Quality reason' });
  await expect(popover).toContainText(reason);
  const after = await card.boundingBox();
  expect(after?.height).toBeCloseTo(before?.height ?? 0, 2);

  await popover.getByRole('button', { name: 'Close quality reason' }).click();
  await expect(popover).not.toBeVisible();
});

test('quality reasons remain available in Grid and image details', async ({ page }) => {
  const reviewReason = 'Tracking error: elongated stars';
  const evidence = 'HFR and eccentricity are poor compared with this capture sequence.';
  let perImageQualityRequests = 0;
  page.on('request', (request) => {
    if (request.url().includes('/analysis/image/')) perImageQualityRequests += 1;
  });
  await page.route(`**/api/db/${dbId}/analysis/sequence*`, async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.data.sequences[0].images[0].regrade_reason = reviewReason;
    body.data.sequences[0].images[0].details = evidence;
    await route.fulfill({ response, json: body });
  });

  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  const card = page.locator('[data-card-image-id="1"]');
  await expect(card).toBeVisible({ timeout: 15_000 });
  await card.getByRole('button', { name: 'Show quality reason' }).click();
  const popover = page.getByRole('dialog', { name: 'Quality reason' });
  await expect(popover).toContainText(reviewReason);
  await expect(popover).toContainText(evidence);
  await popover.getByRole('button', { name: 'Close quality reason' }).click();

  await card.dblclick();
  const detail = page.locator('.detail-quality-analysis');
  await expect(detail.getByRole('heading', { name: 'Quality analysis' })).toBeVisible();
  await expect(detail).toContainText(reviewReason);
  await expect(detail).toContainText(evidence);
  expect(perImageQualityRequests).toBe(0);
});

test('All Projects loads scores without starting a quality scan', async ({
  page,
}) => {
  let sequenceRequests = 0;
  let qualityScanRequests = 0;
  let databaseScope = false;
  page.on('request', (request) => {
    if (request.url().includes('/analysis/sequence')) {
      sequenceRequests += 1;
      databaseScope = new URL(request.url()).searchParams.get('all_projects') === 'true';
    }
    if (request.url().includes('/analysis/quality-scan')) qualityScanRequests += 1;
  });

  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}`);
  await expect(page.locator('.image-card')).toHaveCount(4, { timeout: 15_000 });
  const state = page.locator('.grid-quality-state');
  await expect(state).toHaveText('Quality: 4 scored');
  await expect(page.locator('.image-card .quality-badge')).toHaveCount(4);
  expect(sequenceRequests).toBe(1);
  expect(databaseScope).toBe(true);
  expect(qualityScanRequests).toBe(0);
});

test('Grid selection marker leaves the quality score visible', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}`);

  const cards = page.locator('.image-card');
  await expect(cards).toHaveCount(4, { timeout: 15_000 });
  await expect(page.locator('.image-card .quality-badge')).toHaveCount(4);
  await cards.nth(0).click();

  const selected = page.locator('.image-card-wrapper.multi-selected');
  await expect(selected).toHaveCount(1);
  const gap = await selected.evaluate((wrapper) => {
    const marker = getComputedStyle(wrapper, '::after');
    const wrapperBounds = wrapper.getBoundingClientRect();
    const scoreBounds = wrapper.querySelector('.quality-badge')!.getBoundingClientRect();
    const markerLeft = wrapperBounds.right
      - Number.parseFloat(marker.right)
      - Number.parseFloat(marker.width);
    return markerLeft - scoreBounds.right;
  });

  expect(gap).toBeGreaterThanOrEqual(4);
});

test('Sequence chart Shift-click selects a visible range', async ({ page }) => {
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`
  );

  const bars = page.locator('.sequence-timeline rect[data-image-id]');
  await expect(bars).toHaveCount(3, { timeout: 15_000 });
  await bars.nth(0).click();
  await bars.nth(2).click({ modifiers: ['Shift'] });

  await expect(page.locator('.sequence-image-card.selected')).toHaveCount(3);
  await expect(page.locator('.sequence-selection-bar')).toContainText('3 selected');
});

test('many Sequence tabs wrap into a stable grid without horizontal scrolling', async ({ page }) => {
  await page.route(`**/api/db/${dbId}/analysis/sequence*`, async (route) => {
    const response = await route.fetch();
    const body = await response.json() as {
      data: {
        sequences: ScoredSequence[];
        target_filter_rollups?: TargetFilterRollup[];
      };
    };
    const template = body.data.sequences[0];
    body.data.sequences = Array.from({ length: 12 }, (_, sequenceIndex) => ({
      ...template,
      session_start: template.session_start - sequenceIndex * 86_400,
      session_end: template.session_end - sequenceIndex * 86_400,
      images: template.images.map((image, imageIndex) => ({
        ...image,
        image_id: 1_000 + sequenceIndex * 10 + imageIndex,
      })),
    }));
    const rollupImages = body.data.sequences.flatMap(sequence => sequence.images);
    body.data.target_filter_rollups = [compactTargetFilterRollup({
      ...template,
      session_start: body.data.sequences.at(-1)?.session_start,
      image_count: rollupImages.length,
      images: rollupImages,
    })];
    await route.fulfill({ response, json: body });
  });
  await page.setViewportSize({ width: 760, height: 720 });
  await page.goto(`/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`);

  const tabs = page.locator('.sequence-tab');
  await expect(tabs).toHaveCount(13, { timeout: 15_000 });
  const before = await tabs.evaluateAll(elements => elements.map(element => ({
    top: element.getBoundingClientRect().top,
    width: element.getBoundingClientRect().width,
  })));
  expect(new Set(before.map(tab => Math.round(tab.top))).size).toBeGreaterThan(1);

  await tabs.last().click();
  await expect(tabs.last()).toHaveClass(/active/);
  const after = await tabs.evaluateAll(elements => elements.map(element => ({
    top: element.getBoundingClientRect().top,
    width: element.getBoundingClientRect().width,
  })));
  expect(after.map(tab => Math.round(tab.top - after[0].top))).toEqual(
    before.map(tab => Math.round(tab.top - before[0].top))
  );
  expect(after.map(tab => Math.round(tab.width))).toEqual(
    before.map(tab => Math.round(tab.width))
  );
  const overflow = await page.locator('.sequence-tabs').evaluate(element => ({
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
  }));
  expect(overflow.scrollWidth).toBe(overflow.clientWidth);
});

test('sequence arrows reveal a normal card that is only partly visible', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1&size=300`
  );

  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  await expect(cards.nth(0)).toHaveClass(/current-selection/);
  const nextCard = cards.nth(1);

  await nextCard.evaluate((card) => {
    const container = document.querySelector<HTMLElement>('.app-main')!;
    const cardRect = card.getBoundingClientRect();
    const containerRect = container.getBoundingClientRect();
    container.scrollTop += cardRect.top - containerRect.bottom + 20;
  });
  const visibleBefore = await nextCard.evaluate((card) => {
    const container = document.querySelector<HTMLElement>('.app-main')!;
    const cardRect = card.getBoundingClientRect();
    const containerRect = container.getBoundingClientRect();
    return {
      partlyVisible: cardRect.top < containerRect.bottom && cardRect.bottom > containerRect.bottom,
      fitsViewport: cardRect.height <= containerRect.height,
    };
  });
  expect(visibleBefore).toEqual({ partlyVisible: true, fitsViewport: true });

  await page.keyboard.press('ArrowRight');
  await expect(nextCard).toHaveClass(/current-selection/);
  await expect.poll(async () => {
    return nextCard.evaluate((card) => {
      const container = document.querySelector<HTMLElement>('.app-main')!;
      const cardRect = card.getBoundingClientRect();
      const containerRect = container.getBoundingClientRect();
      return cardRect.top >= containerRect.top - 1
        && cardRect.bottom <= containerRect.bottom + 1;
    });
  }).toBe(true);
});

test('sequence thumbnail size can be changed and survives reload', async ({ page }) => {
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`
  );

  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  const size = page.getByLabel('Size:');
  await expect(size).toHaveValue('150');

  await size.fill('500');

  await expect(page).toHaveURL(/size=500/);
  const strip = page.locator('.sequence-strip');
  await expect(strip).toHaveAttribute(
    'style',
    /grid-template-columns: repeat\(auto-fill, minmax\(min\(500px, 100%\), 1fr\)\)/
  );
  await expect(page.getByText('500px')).toBeVisible();

  await page.reload();
  await expect(page.getByLabel('Size:')).toHaveValue('500');
  await expect(page.locator('.sequence-strip')).toHaveAttribute(
    'style',
    /grid-template-columns: repeat\(auto-fill, minmax\(min\(500px, 100%\), 1fr\)\)/
  );

  await page.getByRole('button', { name: 'Images', exact: true }).click();
  await expect(page.locator('.image-card')).toHaveCount(3);
  const gridSize = page.getByLabel('Size:');
  await expect(gridSize).toHaveValue('500');
  await gridSize.fill('300');
  await expect(page).toHaveURL(/size=300/);

  await page.getByRole('button', { name: 'Sequence', exact: true }).click();
  await expect(page.getByLabel('Size:')).toHaveValue('300');

  await page.setViewportSize({ width: 760, height: 720 });
  await page.getByLabel('Size:').fill('1200');
  await expect(page).toHaveURL(/size=1200/);
  await expect(page.locator('.sequence-strip')).toHaveAttribute(
    'style',
    /grid-template-columns: repeat\(auto-fill, minmax\(min\(1200px, 100%\), 1fr\)\)/
  );
  const widths = await page.evaluate(() => {
    const controls = document.querySelector<HTMLElement>('.sequence-controls')!;
    const strip = document.querySelector<HTMLElement>('.sequence-strip')!;
    return {
      controls: { client: controls.clientWidth, scroll: controls.scrollWidth },
      strip: { client: strip.clientWidth, scroll: strip.scrollWidth },
    };
  });
  expect(widths.controls.scroll).toBeLessThanOrEqual(widths.controls.client);
  expect(widths.strip.scroll).toBeLessThanOrEqual(widths.strip.client);
});

test('sequence thumbnail resizing keeps the active image visible', async ({ page }) => {
  await page.route(`**/api/db/${dbId}/analysis/sequence*`, async (route) => {
    const response = await route.fetch();
    const body = await response.json() as {
      data: {
        sequences: Array<{
          image_count: number;
          images: Array<Record<string, unknown> & { image_id: number }>;
        }>;
      };
    };
    const sequence = body.data.sequences[0];
    const template = sequence.images[0];
    sequence.images = Array.from({ length: 30 }, (_, index) => ({
      ...template,
      image_id: 101 + index,
    }));
    sequence.image_count = sequence.images.length;
    await route.fulfill({ response, json: body });
  });

  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1&current=125`
  );

  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(30, { timeout: 15_000 });
  const current = page.locator('.sequence-image-card.current-selection');
  await expect(current).toBeInViewport();

  await page.getByLabel('Size:').fill('500');

  await expect(page).toHaveURL(/size=500/);
  await expect(current).toBeInViewport();
});

test('detail closes back to the Sequence session that opened it', async ({ page }) => {
  const secondSessionStart = Math.floor(Date.UTC(2026, 3, 17, 0, 25, 0) / 1000);
  await page.route(`**/api/db/${dbId}/analysis/sequence*`, async (route) => {
    const response = await route.fetch();
    const body = await response.json() as {
      data: {
        sequences: Array<{
          session_start?: number;
          session_end?: number;
          image_count: number;
          images: Array<{ image_id: number }>;
        }>;
      };
    };
    const original = body.data.sequences[0];
    const [firstImage, ...laterImages] = original.images;
    body.data.sequences = [
      {
        ...original,
        image_count: 1,
        images: [firstImage],
      },
      {
        ...original,
        session_start: secondSessionStart,
        session_end: secondSessionStart + 132,
        image_count: laterImages.length,
        images: laterImages,
      },
    ];
    await route.fulfill({ response, json: body });
  });

  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`
  );

  const tabs = page.locator('.sequence-tab');
  await expect(tabs).toHaveCount(2, { timeout: 15_000 });
  await tabs.nth(1).click();
  await expect(tabs.nth(1)).toHaveClass(/active/);
  await expect(page).toHaveURL(/current=2/);

  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(2);
  await cards.nth(0).dblclick();
  await expect(page).toHaveURL(/#\/detail\/2/);
  await expect(page).toHaveURL(/returnTo=sequence/);
  await expect(page.locator('.image-card-wrapper')).toHaveCount(0);
  await page.keyboard.press('ArrowRight');
  await expect(page).toHaveURL(/#\/detail\/3/);
  await expect(page).toHaveURL(/current=3/);
  await page.reload();

  await page.locator('.image-detail-overlay .close-button').click();

  await expect(page).toHaveURL(/#\/sequence\?/);
  await expect(page).not.toHaveURL(/returnTo=/);
  await expect(page).toHaveURL(/current=3/);
  await expect(tabs.nth(1)).toHaveClass(/active/);
  await expect(cards.nth(1)).toHaveClass(/current-selection/);
});

test('detail restores the exact Sequence scroll position', async ({ page }) => {
  await page.goto(
    `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1&current=2&size=1200`
  );

  const cards = page.locator('.sequence-image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  const current = cards.nth(1);
  await expect(current).toHaveClass(/current-selection/);
  await current.evaluate((element) => element.scrollIntoView({ block: 'center' }));
  await expect(current).toBeInViewport();
  const scroller = page.locator('.app-main');
  const before = await scroller.evaluate((element) => element.scrollTop);
  expect(before).toBeGreaterThan(0);

  // Dispatch directly so Playwright does not reposition this oversized card
  // while making it actionable; a real user can double-click its visible area.
  await current.dispatchEvent('dblclick');
  await expect(page).toHaveURL(/#\/detail\/2/);
  expect(await page.evaluate(() => window.history.state.usr?.sequenceReturn.scrollTop)).toBe(before);
  await page.locator('.image-detail-overlay .close-button').click();

  await expect(page).toHaveURL(/#\/sequence\?/);
  await expect(cards.nth(1)).toHaveClass(/current-selection/);
  await expect.poll(
    () => scroller.evaluate((element) => element.scrollTop)
  ).toBeCloseTo(before, 0);
});

test('detail opened from Images still closes back to Images', async ({ page }) => {
  await page.goto(
    `/#/grid?db=${encodeURIComponent(dbId)}&project=1&returnTo=sequence`
  );
  const cards = page.locator('.image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  await cards.nth(0).dblclick();
  await expect(page).toHaveURL(/#\/detail\/1/);
  await expect(page).not.toHaveURL(/returnTo=/);

  await page.locator('.image-detail-overlay .close-button').click();

  await expect(page).toHaveURL(/#\/grid\?/);
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
  const dialog = page.getByRole('dialog', { name: /Review 2 selected frames/ });
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
