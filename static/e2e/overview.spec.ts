import { expect, test } from '@playwright/test';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

const seenStorageKey = 'psf-guard:project-seen:v1';

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

  await alphaCard
    .getByRole('button', { name: 'Open Project Alpha images' })
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

  await page.goto('/');
  await expect(page.getByText('Project Alpha')).toBeVisible({
    timeout: 15_000,
  });

  await page.evaluate(
    ({ key, projectKey, totalImages, latestImage }) => {
      localStorage.setItem(
        key,
        JSON.stringify({
          [projectKey]: {
            totalImages,
            latestImage,
          },
        })
      );
    },
    {
      key: seenStorageKey,
      projectKey: `${dbId}:1`,
      totalImages: project.total_images - 2,
      latestImage: (project.date_range.latest ?? 0) - 1,
    }
  );
  await page.reload();

  const alphaCard = page.locator('.project-card').filter({
    hasText: 'Project Alpha',
  });
  await expect(alphaCard).toHaveClass(/has-new-images/, { timeout: 15_000 });
  await expect(alphaCard.getByText('2 new')).toBeVisible();

  await alphaCard
    .getByRole('button', { name: 'Open Project Alpha images' })
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
