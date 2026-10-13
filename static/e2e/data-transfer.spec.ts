import { expect, test, type Page, type Route } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import {
  fixtureDbPath,
  fixtureImageDir,
  resetDatabases,
  tmpBase,
} from './helpers';

/**
 * Settings is tabbed. Data Transfer and Remote PSF Guard live on Sync, which
 * only appears once a catalog is configured — the tests below all configure
 * one first.
 */
async function openSyncTab(page: Page) {
  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByRole('tab', { name: 'Sync' }).click();
}

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
  await openSyncTab(page);

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

test('settings shows calibration coverage for each database rig', async ({ page }) => {
  const summary = {
    schema_version: 1,
    frame_count: 42,
    master_count: 3,
    rigs: [
      {
        rig_uuid: 'rig-1',
        name: 'Ultracat · TestCam',
        profile_id: 'profile',
        telescope: 'Ultracat',
        camera: 'TestCam',
        frame_count: 42,
        bias: 10,
        dark: 12,
        dark_flat: 8,
        flat: 12,
      },
    ],
  };
  await page.route('**/api/db/review/calibrations', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: summary,
        error: null,
        status: 'ready',
      }),
    });
  });
  const ok = (data: unknown) => ({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ success: true, data, error: null, status: 'ready' }),
  });
  const dark = {
    frame_uuid: 'frame-dark', rig_uuid: 'rig-1', kind: 'dark',
    source_path: '/archive/calibration/dark-300s.fits', source_exists: true,
    captured_at: 1_752_000_000, camera: 'TestCam', width: 6248, height: 4176,
    binning_x: 1, binning_y: 1, gain: 100, offset: 30, exposure_s: 300, camera_temp: -10,
    night: '2025-07-08', session: 'session-dark',
  };
  const flat = {
    frame_uuid: 'frame-flat', rig_uuid: 'rig-1', kind: 'flat',
    source_path: '/old-host/calibration/flat-ha.fits', source_exists: false,
    camera: 'TestCam', width: 6248, height: 4176, binning_x: 1, binning_y: 1, gain: 100,
    filter: 'Ha', night: null, session: 'session-flat',
  };
  const session = (frame: typeof dark | typeof flat) => ({
    id: frame.session, kind: frame.kind, night: frame.night, rig_uuid: 'rig-1',
    rig_name: 'Ultracat · TestCam', is_master: false, filter: frame.kind === 'flat' ? 'Ha' : null,
    exposure_min_s: 'exposure_s' in frame ? frame.exposure_s : null,
    exposure_max_s: 'exposure_s' in frame ? frame.exposure_s : null,
    temp_min_c: null, temp_max_c: null, gain: 100, offset: null, readout: null, binning: '1x1',
    width: 6248, height: 4176, rotation: null, first_at: null, last_at: null,
    frames: 1, ok: 0, suspect: 0, poor: 0, ungraded: 1, kept_out: 0,
  });
  const sessionRequests: URLSearchParams[] = [];
  await page.route((url) => url.pathname === '/api/db/review/calibrations/sessions', async (route) => {
    const params = new URL(route.request().url()).searchParams;
    sessionRequests.push(params);
    const frames = params.get('missing') === 'true' ? [flat] : [dark, flat];
    await route.fulfill(ok({
      sessions: frames.map(session), total: frames.length, page: 0, per_page: 50, frames: frames.length,
      facets: { filters: ['Ha'], exposures: [300] }, night_boundary_seconds: 43200,
    }));
  });
  await page.route((url) => url.pathname === '/api/db/review/calibrations/frames', async (route) => {
    const sessionId = new URL(route.request().url()).searchParams.get('session');
    const frames = [dark, flat].filter((frame) => frame.session === sessionId);
    await route.fulfill(ok({ frames, total: frames.length, page: 0, per_page: 500 }));
  });
  await page.route((url) => /^\/api\/db\/review\/calibrations\/frames\/frame-[a-z]+$/.test(url.pathname), async (route) => {
    const uuid = new URL(route.request().url()).pathname.split('/').pop();
    const frame = uuid === 'frame-flat' ? flat : dark;
    await route.fulfill(ok({
      ...frame, measurement: null, measure_error: null, findings: [], metrics: [],
      session_frames: 1, session_measured: 0, compare: false, masters: 0,
    }));
  });

  await page.goto('/');
  await page.getByRole('button', { name: 'Settings' }).click();

  const review = page.locator('.db-row').filter({ hasText: 'Review copy' });
  await expect(review.getByText(/Calibration library · 42 frames · 3 cached masters/)).toBeVisible();
  await expect(review.getByText('Ultracat · TestCam')).toBeVisible();
  await expect(review.getByText(/10 bias · 12 dark · 8 dark-flat · 12 flat/)).toBeVisible();
  await review.getByRole('button', { name: 'Manage' }).click();

  // Manage opens the database's calibration page.
  await expect(page).toHaveURL(/#\/calibration\?db=review/);
  const sessions = page.getByRole('navigation', { name: 'Sessions' });
  await expect(sessions.getByRole('button')).toHaveCount(2);
  await sessions.getByRole('button', { name: /Flats · Ha/ }).click();
  const panel = page.getByRole('complementary', { name: 'Frame' });
  await expect(panel.getByRole('heading', { name: 'flat-ha.fits' })).toBeVisible();
  await expect(panel.getByText('Missing file', { exact: true })).toBeVisible();
  await expect(page.getByRole('list', { name: 'Frames' }).getByText('Missing', { exact: true })).toBeVisible();
  await sessions.getByRole('button', { name: /Darks · 300s/ }).click();
  await expect(panel.getByRole('heading', { name: 'dark-300s.fits' })).toBeVisible();
  // The box follows the address, which changes a moment after the click.
  await page.getByLabel('Missing files').click();
  await expect(page).toHaveURL(/calMissing=true/);
  await expect(page.getByLabel('Missing files')).toBeChecked();
  await expect(sessions.getByRole('button')).toHaveCount(1);
  await expect(sessions.getByRole('button', { name: /Flats · Ha/ })).toBeVisible();
  expect(sessionRequests.at(-1)?.get('missing')).toBe('true');
});

test('data transfer controls fit a compact settings view', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await openSyncTab(page);

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
  await openSyncTab(page);

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
  await openSyncTab(page);
  workspace = page.locator('.scheduler-sync-workspace');
  await expect(
    workspace.getByText('Restored the pending transfer preview.')
  ).toBeVisible();
  await expect(
    workspace.getByRole('button', { name: 'Apply this preview' })
  ).toBeVisible();
  await workspace.getByRole('button', { name: 'Cancel' }).click();
});
