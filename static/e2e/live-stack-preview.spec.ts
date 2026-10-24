import { expect, test } from '@playwright/test';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

test('a running stack refreshes its image, survives a reload, and becomes the finished preview', async ({ page, request }) => {
  await resetDatabases(request);
  const db = await registerFixtureDb(request);
  await waitForCacheReady(request, db.id);
  const jobId = 'a'.repeat(64);
  let frames = 1;
  let finished = false;
  const ok = (data: unknown) => ({ success: true, data, error: null, status: 'ready' });
  await page.route('**/api/stack-activity', (route) => route.fulfill({ json: ok({
    schema_version: 1,
    active: [{ kind: 'mono', job_id: jobId, database_id: db.id, project_id: 1,
      state: 'running', label: 'Alpha M44 · B', detail: 'Registering frames',
      processed_units: frames, total_units: 3, created_unix_seconds: 100 }],
  }) }));
  await page.route(`**/projects/1/stack-previews/${jobId}`, (route) => route.fulfill({ json: ok({
    schema_version: 2, job_id: jobId, database_id: db.id, project_id: 1,
    state: finished ? 'completed' : 'running', accepted_only: false,
    created_unix_seconds: 100, artifact_revision: 'done', cache_version: 7,
    stacking_version: '0.45.0', error: null,
    groups: [{ index: 0, target_id: 1, target_name: 'Alpha M44', filter_name: 'B',
      state: finished ? 'ready' : 'running', phase: finished ? 'ready' : 'stacking',
      total_candidates: 3, eligible_frames: 3, quality_excluded: 0, missing_files: 0,
      processed_frames: frames, accepted_frames: frames, rejected_frames: 0,
      output_channels: 1, reference_image_id: 1, total_exposure_seconds: frames * 180,
      sky_orientation: null, preview_url: null, fits_url: null, error: null,
      calibration: { state: 'none', bias_frames: 0, dark_frames: 0, dark_flat_frames: 0, flat_frames: 0, warning: null },
      input_images: [], frames: [],
      live_preview: finished ? null : {
        url: `/api/db/${db.id}/stack-previews/${jobId}/0/preview?live=true&v=${frames}`,
        accepted_frames: frames,
      },
    }],
  }) }));
  // Keep the job running deterministically while the page polls it. The
  // real preview renderer and HTTP cache behavior have Rust coverage.
  await page.route(`**/stack-previews/${jobId}/0/preview?*`, (route) => route.fulfill({
    contentType: 'image/png',
    body: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=', 'base64'),
  }));
  await page.goto(`/#/stacks?db=${encodeURIComponent(db.id)}&project=1`);
  const panel = page.locator('.stack-preview-panel');
  const image = panel.getByRole('img', { name: 'Alpha M44 B live stack preview' });
  await expect(image).toBeVisible();
  await expect(image).toHaveJSProperty('naturalWidth', 1);
  await expect(image).toHaveAttribute('src', /live=true&v=1$/);
  await expect(panel.getByRole('link', { name: 'Download linear FITS' })).toHaveCount(0);
  frames = 3;
  await expect(image).toHaveAttribute('src', /live=true&v=3$/);
  await expect(panel).toContainText('Live stack · 3 accepted frames');
  await page.reload();
  await expect(image).toBeVisible();
  await expect(image).toHaveAttribute('src', /live=true&v=3$/);
  finished = true;
  const ready = panel.getByRole('img', { name: 'Alpha M44 B stack preview', exact: true });
  await expect(ready).toHaveAttribute('src', /preview\?v=done$/);
  await expect(panel.getByText(/final processing pending/)).toHaveCount(0);
  await expect(panel.getByRole('link', { name: 'Download linear FITS' })).toBeVisible();
});
