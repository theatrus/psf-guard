import { expect, test } from '@playwright/test';
import {
  registerFixtureDb,
  resetDatabases,
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
  await expect(
    alphaCard.getByRole('button', { name: 'Plan & coordinates' })
  ).toBeVisible();
  await expect(
    alphaCard.getByRole('button', { name: 'Edit project' })
  ).toBeVisible();
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
  await expect(page.locator('#project-select')).toBeFocused();
});
