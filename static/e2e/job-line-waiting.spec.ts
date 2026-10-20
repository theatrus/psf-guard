import { expect, test, type Page } from '@playwright/test';
import type { JobRow } from '../src/api/types';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';
import { jobRow, stubJobs } from './jobRows';

/**
 * The header's job line while a person's build runs: automatic work that
 * stepped aside stays listed, marked waiting, and runs again once the
 * build ends, listed once. The job list is stubbed with the rows the
 * server builds in that moment, since holding real automatic work behind a
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
  transfers = [];
});

/** Transfer rows a spec adds to the job list. */
let transfers: JobRow[] = [];

/** The job list while a person's build runs, or after it ends. */
function activityRows(): JobRow[] {
  const rows: JobRow[] = personBuilding
    ? [
        jobRow({
          key: `stack:${'p'.repeat(64)}`, kind: 'stack', title: 'Stacking', scope: 'Alpha M44 · B',
          detail: '2/3 frames', percent: (2 / 3) * 100, queue: 'stack', stoppable: true,
          control: { kind: 'stack', job_id: 'p'.repeat(64) }, hint: 'Registering frames',
          link: { view: 'stacks', db: dbId, project: 1, target: 1 },
        }),
        jobRow({
          key: `backfill:${dbId}`, kind: 'quality', title: 'Analyzing database quality', scope: 'Imaging Rig e2e',
          detail: 'Waiting for a stack build · 2/8 targets · Scanning 4/10 frames', status: 'waiting',
          waiting_for: 'a stack build', percent: 25, background: true,
          link: { view: 'grid', db: dbId },
        }),
        jobRow({
          key: `refresh-waiting:${dbId}:2`, kind: 'stack', title: 'Refreshing stacks',
          scope: 'Imaging Rig e2e · Project Beta',
          detail: 'Waiting for the stack build of Alpha M44 · B · stopped at 12/40 frames', status: 'waiting',
          waiting_for: 'the stack build of Alpha M44 · B', percent: 30, automatic: true, background: true,
          remaining: '1 channel to stack', hint: 'Restacks Beta Field · L',
          link: { view: 'stacks', db: dbId, project: 2 },
        }),
        jobRow({
          key: `reapply-waiting:${dbId}:1:Alpha M44 · R`, kind: 'processing', title: 'Re-applying processing',
          scope: 'Alpha M44 · R', detail: 'Waiting for the stack build of Alpha M44 · B', status: 'waiting',
          waiting_for: 'the stack build of Alpha M44 · B', automatic: true, background: true,
          link: { view: 'stacks', db: dbId, project: 1, target: 1 },
        }),
      ]
    : [
        jobRow({
          key: `stack:${'a'.repeat(64)}`, kind: 'stack', title: 'Stacking', scope: 'Beta Field · L',
          detail: '13/40 frames', percent: 32.5, queue: 'stack', stoppable: true, automatic: true,
          background: true, control: { kind: 'stack', job_id: 'a'.repeat(64) },
          link: { view: 'stacks', db: dbId, project: 2, target: 2 },
        }),
        jobRow({
          key: `backfill:${dbId}`, kind: 'quality', title: 'Analyzing database quality', scope: 'Imaging Rig e2e',
          detail: '2/8 targets · Scanning 4/10 frames', percent: 25, background: true,
          link: { view: 'grid', db: dbId },
        }),
      ];
  // The server lists work needing attention first in each tier.
  return [
    ...rows.filter((row) => !row.background),
    ...transfers.filter((row) => !row.background),
    ...transfers.filter((row) => row.background),
    ...rows.filter((row) => row.background),
  ];
}

async function stubActivity(page: Page) {
  await stubJobs(page, activityRows);
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
function heldTransfers(): JobRow[] {
  const transfer = (id: string, state: 'review' | 'needs_review', error: string) => jobRow({
    key: `transfer:${id}`, kind: 'transfer', title: 'Transfers', scope: `Imaging Rig e2e · Alpha M44 · ${id}`,
    detail: error, status: 'attention', state_label: state === 'review' ? 'Review required' : 'Needs attention',
    automatic: true, background: true,
    transfer: {
      id, connection_id: 'c1', revision: 1, state, catalog: 'Imaging Rig e2e',
      label: `Alpha M44 · ${id}`, total_files: 24, completed_files: 6, total_bytes: 24_000_000,
      completed_bytes: 6_000_000, current_file: '', sent_bytes: 0, error, next_attempt_ms: 0,
      warnings: null, manifest: null, automatic: true,
    },
  });
  return [
    transfer('review', 'review',
      'Incomplete calibration: no flats for OIII on the nights of 2026-10-12 and 2026-10-14, and no darks at this gain'),
    transfer('failed', 'needs_review', 'The collaboration server refused the upload: the panel was closed for new frames'),
  ];
}

for (const width of [1440, 390]) {
  test(`keeps a held transfer's controls on one line at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    transfers = heldTransfers();
    await stubActivity(page);
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
