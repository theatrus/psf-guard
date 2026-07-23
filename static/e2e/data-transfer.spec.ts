import { expect, test, type Route } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import {
  fixtureDbPath,
  fixtureImageDir,
  resetDatabases,
  tmpBase,
} from './helpers';

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const peerPath = path.join(tmpBase(), 'telescope.sqlite');
  fs.copyFileSync(fixtureDbPath(), peerPath);

  for (const database of [
    { name: 'Review copy', db_path: fixtureDbPath(), slug: 'review' },
    { name: 'Telescope scheduler', db_path: peerPath, slug: 'telescope' },
  ]) {
    const response = await request.post('/api/databases', {
      data: {
        ...database,
        image_dirs: [fixtureImageDir()],
      },
    });
    expect(response.ok()).toBeTruthy();
  }
});

test('grade transfer requires a dry preview before Apply appears', async ({
  page,
}) => {
  const requests: Array<Record<string, unknown>> = [];
  await page.route('**/api/databases/review/sync/preview', async (route) => {
    requests.push(route.request().postDataJSON() as Record<string, unknown>);
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          preview_id: 'grade-preview',
          created_at: 1,
          expires_at: 4_102_444_800,
          result: {
            kind: 'push_grades',
            dry_run: true,
            source_db_id: 'review',
            destination_db_id: 'telescope',
            exposuretemplate: {
              inserted: 0,
              updated: 0,
              unchanged: 0,
              skipped: 0,
            },
            project: { inserted: 0, updated: 0, unchanged: 0, skipped: 0 },
            ruleweight: { inserted: 0, updated: 0, unchanged: 0, skipped: 0 },
            target: { inserted: 0, updated: 0, unchanged: 0, skipped: 0 },
            exposureplan: {
              inserted: 0,
              updated: 0,
              unchanged: 0,
              skipped: 0,
            },
            acquiredimage: null,
            imagedata: null,
            grades: {
              source_considered: 4,
              source_no_guid: 0,
              matched: 4,
              changed: 2,
              unchanged: 2,
              unmatched_source: 0,
              destination_only: 0,
              duplicate_guids: 0,
              transitions: { 'Pending→Accepted': 2 },
            },
            grade_filled: 0,
            grade_preserved: 0,
            imagedata_bytes: 0,
            total_inserted: 0,
            total_updated: 2,
          },
        },
        error: null,
        status: 'ready',
      }),
    });
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Settings' }).click();

  const workspace = page.locator('.scheduler-sync-workspace');
  await workspace.getByLabel('Transfer source').selectOption({ label: 'Review copy' });
  await workspace
    .getByLabel('Transfer destination')
    .selectOption({ label: 'Telescope scheduler' });
  await workspace.getByRole('button', { name: 'Send reviewed grades' }).click();
  await expect(
    workspace.getByRole('button', { name: 'Apply this preview' })
  ).toHaveCount(0);

  await workspace.getByRole('button', { name: 'Preview changes' }).click();

  await expect(
    workspace.getByText('2 reviewed grade(s) will change')
  ).toBeVisible();
  await expect(
    workspace.getByRole('button', { name: 'Apply this preview' })
  ).toBeVisible();
  expect(requests).toHaveLength(1);
  expect(requests[0]).toMatchObject({
    peer_db_id: 'telescope',
    kind: 'push_grades',
    dry_run: true,
    reviewed_only: true,
  });
});

test('data transfer controls fit a compact settings view', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings' }).click();

  const workspace = page.locator('.scheduler-sync-workspace');
  await expect(workspace.getByRole('heading', { name: 'Data Transfer' })).toBeVisible();
  await expect(workspace.getByLabel('Transfer source')).toBeVisible();
  await expect(workspace.getByLabel('Transfer destination')).toBeVisible();
  await expect(
    workspace.getByRole('button', { name: 'Swap source and destination' })
  ).toBeVisible();

  const overflows = await workspace.evaluate(
    (element) => element.scrollWidth > element.clientWidth + 1
  );
  expect(overflows).toBe(false);
});

test('a pending transfer preview returns after a page reload', async ({ page }) => {
  const counts = { inserted: 0, updated: 0, unchanged: 0, skipped: 0 };
  const preview = {
    preview_id: 'reload-preview',
    created_at: 1,
    expires_at: 4_102_444_800,
    result: {
      kind: 'push_planning',
      dry_run: true,
      source_db_id: 'review',
      destination_db_id: 'telescope',
      exposuretemplate: counts,
      project: counts,
      ruleweight: counts,
      target: counts,
      exposureplan: counts,
      acquiredimage: null,
      imagedata: null,
      grades: null,
      grade_filled: 0,
      grade_preserved: 0,
      imagedata_bytes: 0,
      total_inserted: 0,
      total_updated: 0,
    },
  };
  const fulfillPreview = (route: Route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: preview,
        error: null,
        status: 'ready',
      }),
    });
  await page.route('**/api/databases/review/sync/preview', fulfillPreview);
  await page.route(
    '**/api/databases/review/sync/previews/reload-preview',
    fulfillPreview
  );

  await page.goto('/');
  await page.getByRole('button', { name: 'Settings' }).click();

  let workspace = page.locator('.scheduler-sync-workspace');
  await workspace.getByLabel('Transfer source').selectOption({ label: 'Review copy' });
  await workspace
    .getByLabel('Transfer destination')
    .selectOption({ label: 'Telescope scheduler' });
  await workspace.getByRole('button', { name: 'Send planning' }).click();
  await workspace.getByRole('button', { name: 'Preview changes' }).click();
  await expect(
    workspace.getByRole('button', { name: 'Apply this preview' })
  ).toBeVisible();

  await page.reload();
  await page.getByRole('button', { name: 'Settings' }).click();
  workspace = page.locator('.scheduler-sync-workspace');
  await expect(
    workspace.getByText('Restored the pending transfer preview.')
  ).toBeVisible();
  await expect(
    workspace.getByRole('button', { name: 'Apply this preview' })
  ).toBeVisible();
  await workspace.getByRole('button', { name: 'Cancel' }).click();
});
