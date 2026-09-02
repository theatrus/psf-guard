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

test('the Status filter narrows the grid and says so in the summary', async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  const cards = page.locator('.image-card');
  await expect(cards).toHaveCount(3, { timeout: 15_000 });
  const status = page.getByLabel('Status:');
  const stats = page.locator('.grid-stats');

  await status.selectOption('accepted');
  await expect(cards).toHaveCount(1);
  await expect(stats).toContainText('1 of 3 images');
  await expect(stats).toContainText('Accepted');
  await expect(page).toHaveURL(/status=accepted/);

  await status.selectOption('pending');
  await expect(cards).toHaveCount(2);
  await expect(stats).toContainText('2 of 3 images');

  await status.selectOption('rejected');
  await expect(cards).toHaveCount(0);
  await expect(page.getByText('No images found')).toBeVisible();

  await page.getByRole('button', { name: 'Reset' }).click();
  await expect(cards).toHaveCount(3);
  await expect(status).toHaveValue('all');
});

test('a link saved with the old numeric status still filters', async ({ page }) => {
  // Before the fix the select emitted the grade number, so shared links
  // carry ?status=1. They must keep meaning Accepted.
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&status=1`);
  await expect(page.locator('.image-card')).toHaveCount(1, { timeout: 15_000 });
  await expect(page.getByLabel('Status:')).toHaveValue('accepted');
});
