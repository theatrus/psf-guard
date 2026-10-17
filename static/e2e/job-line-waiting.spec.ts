import { expect, test, type Page, type Route } from '@playwright/test';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

/**
 * The header's job line while a person's build runs: automatic work that
 * stepped aside stays listed, marked waiting, and runs again once the
 * build ends, listed once. The activity endpoints are stubbed with what the
 * server reports in that moment, since holding real automatic work behind a
 * real build would take minutes of stacking.
 */

let dbId: string;
let personBuilding = true;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, {
    name: 'Imaging Rig e2e',
    slug: 'imaging-rig-e2e',
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
  personBuilding = true;
});

function fulfil(route: Route, data: unknown) {
  return route.fulfill({ json: { success: true, data, error: null, status: 'ready' } });
}

async function stubActivity(page: Page) {
  // The fixture's catalog refresh has settled; keep a late poll of it out
  // of the count.
  await page.route('**/cache-progress', (route) => fulfil(route, {
    is_refreshing: false, stage: 'completed', progress_percentage: 100, elapsed_seconds: 0,
    directories_total: 0, directories_processed: 0, current_directory_name: null, files_scanned: 0,
    projects_total: 0, projects_processed: 0, current_project_name: null, targets_total: 0,
    targets_processed: 0, files_found: 0, files_missing: 0,
  }));
  await page.route('**/api/stack-activity', (route) => fulfil(route, {
    schema_version: 1,
    active: personBuilding
      ? [{
          kind: 'mono', job_id: 'p'.repeat(64), database_id: dbId, project_id: 1, target_id: 1,
          state: 'running', label: 'Alpha M44 · B', detail: 'Registering frames',
          processed_units: 2, total_units: 3, created_unix_seconds: 1_760_000_000, automatic: false,
        }]
      : [{
          kind: 'mono', job_id: 'a'.repeat(64), database_id: dbId, project_id: 2, target_id: 2,
          state: 'running', label: 'Beta Field · L', detail: 'Stacking frames',
          processed_units: 13, total_units: 40, created_unix_seconds: 1_760_000_100, automatic: true,
        }],
    scheduled: personBuilding
      ? [{
          database_id: dbId, database_name: 'Imaging Rig e2e', project_id: 2,
          project_name: 'Project Beta', reason: 'arrival', due_in_seconds: 0,
          channels: ['Beta Field · L'], waiting: true,
          waiting_for: 'the stack build of Alpha M44 · B',
          last_progress: { label: 'Beta Field · L', processed_units: 12, total_units: 40 },
        }]
      : [],
    waiting_reapplies: personBuilding
      ? [{
          database_id: dbId, project_id: 1, target_id: 1, label: 'Alpha M44 · R',
          waiting_for: 'the stack build of Alpha M44 · B',
        }]
      : [],
  }));
  await page.route('**/analysis/quality-backfill', (route) => route.request().method() === 'GET'
    ? fulfil(route, {
        started: false,
        progress: {
          running: true, force: false, total_targets: 8, processed_targets: 2,
          current_target_id: 2, started_at: 1, finished_at: null,
        },
      })
    : route.fallback());
  await page.route(/\/analysis\/quality-scan(\?.*)?$/, (route) => route.request().method() === 'GET'
    ? fulfil(route, {
        started: false,
        cached_count: 0,
        progress: {
          running: true, stage: 'spatial', target_id: 2, filter_name: null, total: 10, processed: 4,
          skipped_cached: 0, spatial_processed: 4, astrometry_processed: 0, solved: 0, solve_failed: 0,
          operational_errors: 0, errors: 0, current_file: null, started_at: 1, finished_at: null,
          last_error: null, automatic: true,
          waiting_for: personBuilding ? 'a stack build' : null,
        },
      })
    : route.fallback());
}

test('lists automatic work as waiting while a person builds, then running once', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await stubActivity(page);
  await page.goto('/');
  const chip = page.getByRole('button', { name: /^Background jobs/ });
  // Icons with counts keep the chip small; each says its words on hover.
  await expect(chip.getByTitle('1 running')).toBeVisible();
  await expect(chip.getByTitle('3 waiting')).toBeVisible();
  await expect(chip).toHaveAccessibleName(/1 running, 3 waiting/);
  await chip.click();
  const list = page.getByRole('region', { name: 'Background jobs' });
  const rows = list.getByRole('listitem');
  await expect(rows).toHaveCount(4);
  // The person's build first, then everything that stepped aside for it,
  // under a heading of its own.
  await expect(rows.nth(0)).toHaveAttribute('data-job-state', 'running');
  await expect(rows.nth(0)).toContainText('Alpha M44 · B');
  await expect(list.getByRole('group', { name: 'Started by you' }).getByRole('listitem')).toHaveCount(1);
  await expect(list.getByRole('group', { name: 'Automatic' }).getByRole('listitem')).toHaveCount(3);
  // Waiting is normal: muted, not drawn in the warning colour.
  const { edge, warning } = await rows.nth(1).evaluate((row) => {
    const probe = document.createElement('span');
    probe.style.color = 'var(--color-warning)';
    document.body.append(probe);
    const colour = getComputedStyle(probe).color;
    probe.remove();
    return { edge: getComputedStyle(row).boxShadow, warning: colour };
  });
  expect(edge).toMatch(/inset/);
  expect(edge).not.toContain(warning);
  for (const index of [1, 2, 3]) {
    await expect(rows.nth(index)).toHaveAttribute('data-job-state', 'waiting');
    await expect(rows.nth(index).locator('.activity-row-state')).toHaveText('waiting');
  }
  await expect(rows.nth(1)).toContainText('Waiting for a stack build · 2/8 targets');
  await expect(rows.nth(2)).toContainText(
    'Waiting for the stack build of Alpha M44 · B · stopped at 12/40 frames'
  );
  await expect(rows.nth(3)).toContainText('Re-applying processing');

  // The build ends: the refresh runs as its build, listed once, and the
  // analysis goes on.
  personBuilding = false;
  await expect(chip.getByTitle('2 running')).toBeVisible({ timeout: 10_000 });
  await expect(chip.locator('.activity-count[data-job-state="waiting"]')).toHaveCount(0);
  await expect(rows).toHaveCount(2);
  await expect(list.locator('li[data-job-state="waiting"]')).toHaveCount(0);
  await expect(list).not.toContainText('Refreshing stacks');
  await expect(rows.filter({ hasText: 'Beta Field · L' })).toHaveCount(1);
});

test('keeps the waiting count on a phone without widening the page', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await stubActivity(page);
  await page.goto('/');
  const chip = page.getByRole('button', { name: /^Background jobs/ });
  await expect(chip).toHaveAccessibleName(/1 running, 3 waiting/);
  // The percent stands for running work; how many wait still shows.
  await expect(chip.getByTitle('3 waiting')).toBeVisible();
  await expect(chip.getByTitle('1 running')).toBeHidden();
  const chipBox = await chip.boundingBox();
  expect(chipBox!.width).toBeLessThan(110);
  await chip.click();
  const list = page.getByRole('region', { name: 'Background jobs' });
  await expect(list.locator('li[data-job-state="waiting"]')).toHaveCount(3);
  const widths = await page.evaluate(() => ({
    scroll: document.documentElement.scrollWidth,
    inner: window.innerWidth,
  }));
  expect(widths.scroll).toBeLessThanOrEqual(widths.inner);
  const box = await list.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(390);
});

/** A transfer held for review: its row needs attention, and its controls
 *  sit on one line beside a long error, on a desktop and on a phone. */
async function stubHeldTransfers(page: Page) {
  const transfer = (id: string, state: string, error: string | null) => ({
    id, connection_id: 'c1', revision: 1, state, catalog: 'Imaging Rig e2e',
    label: `Alpha M44 · ${id}`, total_files: 24, completed_files: 6, total_bytes: 24_000_000,
    completed_bytes: 6_000_000, current_file: '', sent_bytes: 0, error, next_attempt_ms: 0,
    warnings: null, manifest: null, automatic: true,
  });
  await page.route('**/director/v1/transfers', (route) => fulfil(route, {
    jobs: [
      transfer('review', 'review',
        'Incomplete calibration: no flats for OIII on the nights of 2026-10-12 and 2026-10-14, and no darks at this gain'),
      transfer('failed', 'needs_review', 'The collaboration server refused the upload: the panel was closed for new frames'),
    ],
    policies: [],
  }));
}

for (const width of [1440, 390]) {
  test(`keeps a held transfer's controls on one line at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stubActivity(page);
    await stubHeldTransfers(page);
    await page.goto('/');
    const chip = page.getByRole('button', { name: /^Background jobs/ });
    await expect(chip).toHaveAccessibleName(/2 need attention/);
    await chip.click();
    const list = page.getByRole('region', { name: 'Background jobs' });
    const held = list.getByRole('group', { name: 'Automatic' }).locator('li[data-job-state="attention"]');
    await expect(held).toHaveCount(2);
    for (const index of [0, 1]) {
      const row = held.nth(index);
      const buttons = row.locator('.activity-row-actions button');
      await expect(buttons).toHaveCount(2);
      const [first, second] = [await buttons.nth(0).boundingBox(), await buttons.nth(1).boundingBox()];
      // Side by side: the same top, the second to the right of the first.
      expect(Math.abs(first!.y - second!.y)).toBeLessThan(1);
      expect(second!.x).toBeGreaterThan(first!.x + first!.width - 1);
      const rowBox = (await row.boundingBox())!;
      expect(second!.x + second!.width).toBeLessThanOrEqual(rowBox.x + rowBox.width + 0.5);
    }
  });
}
