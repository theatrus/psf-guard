import { expect, test } from '@playwright/test';
import { mkdtempSync } from 'node:fs';
import * as path from 'node:path';
import {
  registerFixtureDb,
  resetDatabases,
  tmpBase,
  waitForCacheReady,
} from './helpers';

const seenStorageKey = 'psf-guard:project-seen:v2';

let dbId: string;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, {
    name: 'Overview Rig',
    slug: 'overview-rig',
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

test('overview puts projects ahead of a compact catalog summary', async ({
  page,
}) => {
  await page.route(`**/api/db/${dbId}/projects/overview`, async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    const alpha = body.data.find(
      (project: { id: number }) => project.id === 1
    );
    expect(alpha).toBeTruthy();
    alpha.total_desired = 4;
    await route.fulfill({ response, json: body });
  });
  await page.goto('/');

  const summary = page.locator('.overview-summary');
  await expect(summary).toBeVisible({ timeout: 15_000 });
  await expect(summary).toContainText('4 images');
  await expect(page.locator('.stat-card')).toHaveCount(0);

  const alphaCard = page.locator('.project-card').filter({
    hasText: 'Project Alpha',
  });
  await expect(alphaCard).toHaveCount(1);
  await expect(alphaCard.getByText('Open image grid')).toBeVisible();
  await expect(alphaCard.locator('.project-frame')).toHaveCount(3);
  await expect(page.locator('.project-frame-age')).toHaveCount(4);
  await expect(page.locator('.project-frame-age').first()).toContainText('Captured');
  await expect(page.locator('.project-frame-age').first()).toContainText(/ago$/);
  const newestFrame = page.locator('.project-frame.is-newest');
  await expect(newestFrame).toHaveCount(1);
  await expect(newestFrame.getByText('Newest', { exact: true })).toBeVisible();
  await expect(newestFrame).toHaveAttribute('aria-label', /Beta Field/);
  await expect(
    alphaCard.getByRole('button', { name: 'Open the Project Alpha plan' })
  ).toBeVisible();
  await expect(
    alphaCard.getByRole('button', { name: 'Edit project' })
  ).toBeVisible();
  await expect(alphaCard.getByText('1 / 4 desired')).toBeVisible();
  const desiredProgress = alphaCard.locator('.project-desired-progress');
  const desiredLabel = desiredProgress.getByText('Desired progress', {
    exact: true,
  });
  const desiredBar = desiredProgress.locator('.desired-progress-bar');
  const gradingStatus = alphaCard.locator('.project-grading-progress');
  const gradingLabel = gradingStatus.getByText('Grading status', {
    exact: true,
  });
  const gradingBar = gradingStatus.getByRole('img', {
    name: /Grading status: \d+ accepted, \d+ rejected, \d+ pending/,
  });
  await expect(desiredLabel).toBeVisible();
  await expect(desiredBar).toBeVisible();
  await expect(gradingLabel).toBeVisible();
  await expect(gradingBar).toBeVisible();
  const [
    statsBounds,
    desiredLabelBounds,
    desiredBarBounds,
    gradingLabelBounds,
    gradingBarBounds,
  ] = await Promise.all([
    alphaCard.locator('.project-stats').boundingBox(),
    desiredLabel.boundingBox(),
    desiredBar.boundingBox(),
    gradingLabel.boundingBox(),
    gradingBar.boundingBox(),
  ]);
  expect(statsBounds).not.toBeNull();
  expect(desiredLabelBounds).not.toBeNull();
  expect(desiredBarBounds).not.toBeNull();
  expect(gradingLabelBounds).not.toBeNull();
  expect(gradingBarBounds).not.toBeNull();
  expect(desiredLabelBounds!.y + desiredLabelBounds!.height)
    .toBeLessThanOrEqual(desiredBarBounds!.y);
  expect(desiredBarBounds!.y + desiredBarBounds!.height)
    .toBeLessThanOrEqual(gradingLabelBounds!.y);
  expect(gradingLabelBounds!.y + gradingLabelBounds!.height)
    .toBeLessThanOrEqual(gradingBarBounds!.y);
  expect(gradingBarBounds!.height).toBeLessThanOrEqual(10);
  expect(Math.abs(desiredBarBounds!.x - statsBounds!.x)).toBeLessThanOrEqual(1);
  expect(Math.abs(desiredBarBounds!.width - statsBounds!.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(gradingBarBounds!.x - statsBounds!.x)).toBeLessThanOrEqual(1);
  expect(Math.abs(gradingBarBounds!.width - statsBounds!.width)).toBeLessThanOrEqual(1);
  const alphaTargets = alphaCard.locator('.target-compact-card');
  await expect(alphaTargets).toHaveCount(1);
  await expect(
    alphaTargets.getByRole('button', { name: /Open .+ image grid/ })
  ).toBeVisible();
  await expect(alphaCard.locator('.project-target-toggle')).toHaveCount(0);

  await alphaCard
    .getByRole('button', { name: 'Open Project Alpha image grid' })
    .click();
  await expect(page).toHaveURL(
    new RegExp(`#\\/grid\\?db=${encodeURIComponent(dbId)}&project=1(?:&|$)`)
  );
});

test('the Projects toolbar keeps every control inside its edge', async ({
  page,
  request,
}) => {
  // A second, empty catalog brings the Database filter, the widest pill.
  const incoming = mkdtempSync(path.join(tmpBase(), 'toolbar-second-'));
  const created = await request.post('/api/databases/create', {
    data: { name: 'Second Rig', image_dirs: [incoming], slug: 'second-rig' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();

  for (const width of [1024, 860]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    const toolbar = page.locator('.projects-toolbar');
    await expect(
      toolbar.getByRole('combobox', { name: 'Filter projects by database' })
    ).toBeVisible({ timeout: 15_000 });
    const outside = await toolbar.evaluate((bar) => {
      const edge = bar.getBoundingClientRect();
      return [...bar.querySelectorAll('*')]
        .filter((element) => {
          const box = element.getBoundingClientRect();
          return box.width > 0 && (box.right > edge.right + 0.5 || box.left < edge.left - 0.5);
        })
        .map((element) => `${element.tagName.toLowerCase()}.${String(element.className).split(' ')[0]}`);
    });
    expect(outside, `controls past the toolbar at ${width}px`).toEqual([]);
    await expect(page.getByRole('radio', { name: 'Detailed' })).toBeInViewport({ ratio: 1 });
  }
});

test('compact rows keep each pill kind in one column', async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem(
      'psf-guard.display-preferences',
      JSON.stringify({ libraryDensity: 'compact' })
    );
  });
  // One line at desktop width, two lines on a small laptop: the columns
  // line up either way.
  for (const width of [1440, 1024]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    const rows = page.getByTestId('library-row');
    await expect(rows).toHaveCount(2, { timeout: 15_000 });
    const lefts = await rows.evaluateAll((all) =>
      all.map((row) =>
        Object.fromEntries(
          [...row.children].map((cell) => [
            cell.getAttribute('data-column'),
            Math.round(cell.getBoundingClientRect().left),
          ])
        )
      )
    );
    expect(Object.keys(lefts[0])).toEqual([
      'fold', 'name', 'db', 'state', 'progress', 'grades', 'dates', 'calibration', 'views', 'plan',
    ]);
    // The fold arrow shifts with a group's or family's inset; every column
    // after it starts at one x on every row.
    for (const column of Object.keys(lefts[0]).filter((key) => key !== 'fold')) {
      expect(lefts[1][column], `${column} at ${width}px`).toBe(lefts[0][column]);
    }
  }
});

test('overview marks projects with images added since they were opened', async ({
  page,
  request,
}) => {
  const response = await request.get(
    `/api/db/${encodeURIComponent(dbId)}/projects/overview`
  );
  expect(response.ok()).toBe(true);
  const body = await response.json();
  const project = body.data.find(
    (candidate: { id: number }) => candidate.id === 1
  );
  expect(project).toBeTruthy();
  const targetsResponse = await request.get(
    `/api/db/${encodeURIComponent(dbId)}/targets/overview`
  );
  expect(targetsResponse.ok()).toBe(true);
  const targetsBody = await targetsResponse.json();
  const projectTargets = targetsBody.data.filter(
    (candidate: { project_id: number }) => candidate.project_id === 1
  );
  expect(projectTargets).toHaveLength(1);
  const changedTarget = projectTargets[0];

  await page.goto('/');
  await expect(page.getByText('Project Alpha')).toBeVisible({
    timeout: 15_000,
  });

  await page.evaluate(
    ({ key, projectKey, totalImages, latestImage, targetId, targetImages, targetLatest }) => {
      localStorage.setItem(
        key,
        JSON.stringify({
          [projectKey]: {
            totalImages,
            latestImage,
            targets: {
              [targetId]: {
                totalImages: targetImages,
                latestImage: targetLatest,
              },
            },
          },
        })
      );
    },
    {
      key: seenStorageKey,
      projectKey: `${dbId}:1`,
      totalImages: project.total_images - 2,
      latestImage: (project.date_range.latest ?? 0) - 1,
      targetId: String(changedTarget.id),
      targetImages: changedTarget.image_count - 2,
      targetLatest: (changedTarget.date_range.latest ?? 0) - 1,
    }
  );
  await page.reload();

  const alphaCard = page.locator('.project-card').filter({
    hasText: 'Project Alpha',
  });
  await expect(alphaCard).toHaveClass(/has-new-images/, { timeout: 15_000 });
  await expect(alphaCard.locator('.new-images-badge')).toHaveText('2 new');
  await expect(alphaCard.getByText('2 new frames')).toBeVisible();
  await expect(alphaCard.locator('.project-frame.is-new')).toHaveCount(2);
  const changedTargetCard = alphaCard.locator('.target-compact-card').filter({
    hasText: changedTarget.name,
  });
  await expect(changedTargetCard).toHaveClass(/has-new-images/);
  await expect(
    changedTargetCard.getByText('2 new', { exact: true })
  ).toBeVisible();

  await alphaCard
    .getByRole('button', { name: 'Open Project Alpha image grid' })
    .click();
  await expect(page).toHaveURL(
    new RegExp(`#\\/grid\\?db=${encodeURIComponent(dbId)}&project=1(?:&|$)`)
  );

  await page.goto('/');
  await expect(page.locator('.overview-summary')).toBeVisible({
    timeout: 15_000,
  });
  const seenAlphaCard = page.locator('.project-card').filter({
    hasText: 'Project Alpha',
  });
  await expect(seenAlphaCard).not.toHaveClass(/has-new-images/);
  await expect(seenAlphaCard.locator('.new-images-badge')).toHaveCount(0);
});

test('recent project frame opens the image detail view', async ({ page }) => {
  await page.goto('/');

  const alphaCard = page.locator('.project-card').filter({
    hasText: 'Project Alpha',
  });
  await expect(alphaCard.locator('.project-frame')).toHaveCount(3, {
    timeout: 15_000,
  });

  const firstFrame = alphaCard.locator('.project-frame').first();
  const frameName = await firstFrame.getAttribute('aria-label');
  expect(frameName).toMatch(/^Open .+ frame$/);
  await firstFrame.click();

  await expect(page).toHaveURL(
    new RegExp(
      `#\\/detail\\/\\d+\\?db=${encodeURIComponent(dbId)}&project=1&target=\\d+(?:&|$)`
    )
  );
});

test('image grid prompts for a project before showing an empty result', async ({
  page,
}) => {
  await page.goto('/#/grid');

  await expect(
    page.getByRole('heading', { name: 'Choose a project' })
  ).toBeVisible();
  await expect(page.getByText('No images found')).toHaveCount(0);

  await page.getByRole('button', { name: 'Choose a project' }).click();
  await expect(page.locator('#scope-select')).toBeFocused();
});
