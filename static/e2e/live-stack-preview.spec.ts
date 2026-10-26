import { expect, test } from '@playwright/test';
import { registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

for (const width of [16, 40]) {
  test(`a running ${width === 40 ? 'mosaic' : 'channel'} stack refreshes one image area and becomes the finished preview`, async ({ page, request }, testInfo) => {
    await page.setViewportSize({ width: 1280, height: 1900 });
    await resetDatabases(request);
    const db = await registerFixtureDb(request);
    await waitForCacheReady(request, db.id);
    const jobId = 'a'.repeat(64);
    const targetName = width === 40 ? 'Heart and Soul mosaic' : 'Alpha M44';
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
      groups: [{ index: 0, target_id: width === 40 ? -1 : 1, target_name: targetName, filter_name: 'B',
        state: finished ? 'ready' : 'running', phase: finished ? 'ready' : 'stacking',
        total_candidates: 3, eligible_frames: 3, quality_excluded: 0, missing_files: 0,
        processed_frames: frames, accepted_frames: frames, rejected_frames: 0,
        output_channels: 1, reference_image_id: 1, total_exposure_seconds: frames * 180,
        sky_orientation: null, preview_url: null, fits_url: null, error: null,
        calibration: { state: 'none', bias_frames: 0, dark_frames: 0, dark_flat_frames: 0, flat_frames: 0, warning: null },
        input_images: [], frames: [],
        snr: width === 40 ? {
          mosaic: true, order: 'capture', analysis: null,
          analysis_reason: 'Mosaic frames cover different sky areas. Total frame count cannot predict the depth at each pixel.',
          points: [1, ...(frames > 1 ? [frames] : [])].map((depth) => ({
            frames: depth, exposure_seconds: depth * 180,
            noise: depth === 1 ? 20 : 18, background: 1000, signal: depth === 1 ? 100 : 150,
            snr: depth === 1 ? 5 : 150 / 18,
            coverage: { covered_fraction: depth === 1 ? 0.5 : 1, mean_depth: depth === 1 ? 1 : 1.5 },
          })),
        } : null,
        live_preview: finished ? null : {
          url: `/api/db/${db.id}/stack-previews/${jobId}/0/preview?live=true&v=${frames}`,
          accepted_frames: frames,
        },
      }],
    }) }));
    // Keep the job running deterministically while the page polls it. The
    // real preview renderer and HTTP cache behavior have Rust coverage.
    await page.route(`**/stack-previews/${jobId}/0/preview?*`, (route) => route.fulfill({
      contentType: 'image/svg+xml',
      body: `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="12"><rect width="${width}" height="12" fill="white"/></svg>`,
    }));
    await page.goto(`/#/stacks?db=${encodeURIComponent(db.id)}&project=1`);
    const panel = page.locator('.stack-preview-panel');
    const image = panel.getByRole('img', { name: `${targetName} B live stack preview` });
    await expect(image).toBeVisible();
    await expect(image).toHaveJSProperty('naturalWidth', width);
    const card = panel.locator('.stack-preview-card').filter({
      has: page.getByRole('img', { name: `${targetName} B live stack preview` }),
    });
    await expect(card).toBeVisible();
    await expect(card.locator('.stack-preview-placeholder')).toHaveCount(0);
    await expect(card.getByRole('progressbar')).toBeVisible();
    const imageBox = (await card.locator('.stack-preview-image').boundingBox())!;
    const progressBox = (await card.locator('.stack-preview-progress').boundingBox())!;
    expect(progressBox.y - imageBox.y - imageBox.height).toBeLessThan(120);
    await expect(image).toHaveAttribute('src', /live=true&v=1$/);
    await expect(panel.getByRole('link', { name: 'Download linear FITS' })).toHaveCount(0);
    frames = 3;
    await expect(image).toHaveAttribute('src', /live=true&v=3$/);
    await expect(panel).toContainText('Live stack · 3 accepted frames');
    if (width === 40) {
      await expect(card.getByText('Mosaic SNR and coverage')).toBeVisible();
      await expect(card.getByText('of canvas covered', { exact: true })).toBeVisible();
      await expect(card.getByText('mean frames per covered pixel', { exact: true })).toBeVisible();
      await expect(card.locator('.stack-snr-ideal')).toHaveCount(0);
      await card.locator('.stack-snr-live').screenshot({ path: testInfo.outputPath('mosaic-snr-coverage.png') });
    }
    await expect(card.locator('.stack-preview-placeholder')).toHaveCount(0);
    await card.screenshot({ path: testInfo.outputPath('live-stack-one-image-area.png') });
    await page.reload();
    await expect(image).toBeVisible();
    await expect(image).toHaveAttribute('src', /live=true&v=3$/);
    finished = true;
    const ready = panel.getByRole('img', { name: `${targetName} B stack preview`, exact: true });
    await expect(ready).toHaveAttribute('src', /preview\?v=done$/);
    await expect(panel.getByText(/final processing pending/)).toHaveCount(0);
    await expect(panel.getByRole('link', { name: 'Download linear FITS' })).toBeVisible();
  });
}
