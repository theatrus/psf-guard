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

  // The group header for the B filter should render even when collapsed.
  // (Cards live inside collapsible groups; expand-on-load is racy, so we
  // assert the more stable signal here. See the direct-deep-link test for
  // the card-visible assertion.)
  await expect(page.getByRole('heading', { name: 'B', exact: true })).toBeVisible({
    timeout: 15_000,
  });
});

test('direct deep link to the grid loads when ?db= matches a configured DB', async ({
  page,
}) => {
  // Hash-router URLs: /#/grid?... The fixture has a single B-filter group;
  // pre-expand it via the `expanded` URL param so .image-card actually
  // mounts. (The auto-expand effect in GroupedImageGrid is racy with the
  // initial data load.)
  await page.goto(
    `/#/grid?db=${encodeURIComponent(dbId)}&project=1&expanded=B`
  );
  await expect(page.locator('.image-card').first()).toBeVisible({
    timeout: 15_000,
  });
});

test('scoped view without ?db= renders no image cards', async ({ page }) => {
  // Loading /grid with no db param means the route can't resolve a database.
  // The query is gated on !!dbId && !!projectId, so no .image-card renders.
  await page.goto('/#/grid');
  await expect(page.locator('.image-card')).toHaveCount(0);
});
