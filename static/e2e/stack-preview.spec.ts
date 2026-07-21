import { expect, test } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

let dbId: string;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, {
    name: 'Stack Preview e2e',
    slug: 'stack-preview-e2e',
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

test('builds a real three-frame Seiza stack and exposes its frame decisions', async ({
  page,
}) => {
  test.setTimeout(240_000);
  await page.setViewportSize({ width: 1440, height: 1600 });

  // This spec is about the stack queue. Keep the ordinary image-preview queue
  // out of the way so the large FITS fixture is not decoded twice in parallel.
  await page.route('**/images/*/preview?*', (route) => route.abort());
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  const panel = page.locator('.stack-preview-panel');
  await expect(panel).toBeVisible({ timeout: 15_000 });
  await expect(panel).toContainText('3 visible images');
  const gridColumns = await panel.locator('.stack-preview-grid').evaluate(
    (grid) => getComputedStyle(grid).gridTemplateColumns.split(' ').filter(Boolean).length
  );
  expect(gridColumns).toBe(2);
  await panel.getByRole('button', { name: 'Build channel', exact: true }).click();

  const progress = panel.locator('.stack-preview-progress');
  await expect(progress).toBeVisible();
  await expect(progress).toHaveAttribute('data-stack-state', /queued|running/);
  await expect(progress).toContainText(/\d+\/3 frames/);
  await expect(panel.locator('.stack-preview-metrics')).toBeVisible();

  const results = panel.locator('.stack-preview-results');
  await expect(results).toHaveAttribute('data-job-state', 'completed', {
    timeout: 210_000,
  });
  await expect(panel.locator('.stack-group-state.ready')).toHaveText('ready');
  await expect(progress).toHaveAttribute('data-stack-state', 'ready');
  await expect(progress).toContainText('3/3 frames');
  await expect(panel).toContainText('Alpha M44');
  await expect(panel.locator('.stack-preview-channel')).toHaveText('B');
  await expect(panel).toContainText('Uncalibrated stack preview');

  const preview = panel.getByRole('img', { name: /uncalibrated stack preview/i });
  await expect(preview).toBeVisible();
  await page.waitForFunction(
    (element) =>
      element instanceof HTMLImageElement && element.complete && element.naturalWidth > 0,
    await preview.elementHandle(),
    { timeout: 30_000 }
  );

  const integrated = Number(
    await panel.locator('.stack-preview-metrics > div').first().locator('strong').textContent()
  );
  expect(integrated).toBeGreaterThanOrEqual(2);

  const fitsLink = panel.getByRole('link', { name: 'Download linear FITS' });
  const fitsHref = await fitsLink.getAttribute('href');
  expect(fitsHref).toMatch(/\/stack-previews\/[a-f0-9]{64}\/0\/fits\?v=[a-f0-9-]+$/);
  const fitsHead = await page.request.head(fitsHref!);
  expect(fitsHead.status()).toBe(200);
  expect(fitsHead.headers()['content-type']).toContain('application/fits');
  expect(fitsHead.headers()['content-disposition']).toMatch(/attachment; filename=.*\.fits/);
  expect(Number(fitsHead.headers()['content-length'])).toBeGreaterThan(10_000_000);

  await panel.getByRole('button', { name: 'Inspect full size' }).click();
  const inspector = page.getByRole('dialog', { name: /Alpha M44/i });
  await expect(inspector).toBeVisible();

  const fullSizeImage = inspector.getByTestId('stack-inspector-image');
  await page.waitForFunction(
    (element) =>
      element instanceof HTMLImageElement && element.complete && element.naturalWidth > 2400,
    await fullSizeImage.elementHandle(),
    { timeout: 60_000 }
  );
  const fullSizeDimensions = await fullSizeImage.evaluate((image) => ({
    width: image.naturalWidth,
    height: image.naturalHeight,
  }));
  expect(fullSizeDimensions.width).toBeGreaterThan(2400);
  expect(fullSizeDimensions.height).toBeGreaterThan(1600);
  await expect(inspector).toContainText(
    `${fullSizeDimensions.width} × ${fullSizeDimensions.height}`
  );

  const fullSizeSrc = await fullSizeImage.getAttribute('src');
  expect(fullSizeSrc).toContain('size=original');
  const fullSizeHead = await page.request.head(fullSizeSrc!);
  expect(fullSizeHead.status()).toBe(200);
  expect(fullSizeHead.headers()['content-type']).toContain('image/png');

  await inspector.getByRole('button', { name: '100%' }).click();
  await expect(inspector.locator('.zoom-percentage-compact')).toHaveText('100%');
  const transformBeforePan = await fullSizeImage.evaluate((image) => image.style.transform);
  const canvas = inspector.locator('.stack-inspector-canvas');
  const canvasBox = await canvas.boundingBox();
  expect(canvasBox).not.toBeNull();
  await page.mouse.move(canvasBox!.x + canvasBox!.width / 2, canvasBox!.y + canvasBox!.height / 2);
  await page.mouse.down();
  await page.mouse.move(
    canvasBox!.x + canvasBox!.width / 2 + 120,
    canvasBox!.y + canvasBox!.height / 2 + 80,
    { steps: 4 }
  );
  await page.mouse.up();
  await expect
    .poll(() => fullSizeImage.evaluate((image) => image.style.transform))
    .not.toBe(transformBeforePan);

  if (process.env.PSF_GUARD_CAPTURE_DOCS === '1') {
    const docs = path.resolve(process.cwd(), '..', 'docs');
    fs.mkdirSync(docs, { recursive: true });
    await inspector.screenshot({ path: path.join(docs, 'stack-preview-inspection.png') });
  }

  await page.keyboard.press('Escape');
  await expect(inspector).toHaveCount(0);

  const jobId = fitsHref!.match(/\/stack-previews\/([a-f0-9]{64})\/0\/fits/)![1];
  const fitsPath = path.join(
    process.env.PSF_GUARD_E2E_TMP!,
    'cache',
    dbId,
    'stack-previews',
    jobId,
    'group-0.fits'
  );
  const fitsHeader = Buffer.alloc(9);
  const fitsFile = fs.openSync(fitsPath, 'r');
  fs.readSync(fitsFile, fitsHeader, 0, fitsHeader.length, 0);
  fs.closeSync(fitsFile);
  expect(fitsHeader.toString('ascii')).toBe('SIMPLE  =');

  const latestResponse = await page.request.get(
    `/api/db/${encodeURIComponent(dbId)}/projects/1/stack-previews/latest`
  );
  expect(latestResponse.status()).toBe(200);
  const latestPayload = await latestResponse.json();
  expect(latestPayload.data.groups).toHaveLength(1);
  expect(latestPayload.data.groups[0].job_id).toBe(jobId);

  if (process.env.PSF_GUARD_CAPTURE_DOCS === '1') {
    const docs = path.resolve(process.cwd(), '..', 'docs');
    fs.mkdirSync(docs, { recursive: true });
    await panel.screenshot({ path: path.join(docs, 'stack-preview.png') });
  }

  const details = panel.locator('.stack-preview-details');
  await details.locator('summary').click();
  await expect(details.locator('tbody tr')).toHaveCount(3);
  await expect(details).toContainText('reference');
  await expect(details).toContainText('accepted');

  if (process.env.PSF_GUARD_CAPTURE_DOCS === '1') {
    const docs = path.resolve(process.cwd(), '..', 'docs');
    await details.screenshot({ path: path.join(docs, 'stack-preview-decisions.png') });
  }

  // Changing policy marks the remembered result out of date without hiding it.
  const acceptedOnly = panel.getByRole('checkbox', { name: 'Accepted only' });
  await acceptedOnly.check();
  await expect(panel.locator('.stack-preview-card')).toHaveAttribute('data-outdated', 'true');
  await expect(panel.locator('.stack-preview-outdated')).toContainText('Accepted only changed');
  await expect(panel.getByRole('img', { name: /uncalibrated stack preview/i })).toBeVisible();
  await acceptedOnly.uncheck();
  await expect(panel.locator('.stack-preview-card')).toHaveAttribute('data-outdated', 'false');

  await page.goto(
    `/#/grid?db=${encodeURIComponent(dbId)}&project=1&search=no-such-stack-target`
  );
  await expect(page.locator('.stack-preview-outdated')).toContainText(
    'not in the current input'
  );
  await expect(page.getByRole('img', { name: /uncalibrated stack preview/i })).toBeVisible();
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.stack-preview-card')).toHaveAttribute('data-outdated', 'false');

  // The last successful per-channel result survives navigation and restart-like
  // page reloads without starting another stack job.
  const rememberedSrc = await panel
    .getByRole('img', { name: /uncalibrated stack preview/i })
    .getAttribute('src');
  await page.reload();
  const reloadedPanel = page.locator('.stack-preview-panel');
  await expect(reloadedPanel.locator('.stack-preview-results')).toHaveAttribute(
    'data-job-state', 'remembered'
  );
  await expect(reloadedPanel.getByRole('img', { name: /uncalibrated stack preview/i })).toBeVisible();
  expect(
    await reloadedPanel.getByRole('img', { name: /uncalibrated stack preview/i }).getAttribute('src')
  ).toBe(rememberedSrc);

  // Scheduler grade changes are independently detected even when the set of
  // image IDs is unchanged.
  const input = latestPayload.data.groups[0].group.input_images[0];
  const statusNames = ['pending', 'accepted', 'rejected'] as const;
  const changedStatus = input.grading_status === 2 ? 'accepted' : 'rejected';
  const gradeResponse = await page.request.put(
    `/api/db/${encodeURIComponent(dbId)}/images/${input.image_id}/grade`,
    { data: { status: changedStatus } }
  );
  expect(gradeResponse.ok()).toBe(true);
  await page.reload();
  await expect(page.locator('.stack-preview-outdated')).toContainText('image grades changed');

  const restoreResponse = await page.request.put(
    `/api/db/${encodeURIComponent(dbId)}/images/${input.image_id}/grade`,
    { data: { status: statusNames[input.grading_status] } }
  );
  expect(restoreResponse.ok()).toBe(true);
  await page.reload();
  await expect(page.locator('.stack-preview-card')).toHaveAttribute('data-outdated', 'false');

  // Rebuild just this channel. Its content-addressed job stays the same, but
  // the forced run receives a fresh artifact revision.
  const cachedSrc = await page.locator('.stack-preview-panel')
    .getByRole('img', { name: /uncalibrated stack preview/i })
    .getAttribute('src');
  await page.locator('.stack-preview-panel')
    .getByRole('button', { name: 'Rebuild channel', exact: true })
    .click();
  await expect(page.locator('.stack-preview-results')).toHaveAttribute(
    'data-job-state',
    'completed',
    { timeout: 210_000 }
  );
  const rebuiltSrc = await page.locator('.stack-preview-panel')
    .getByRole('img', { name: /uncalibrated stack preview/i })
    .getAttribute('src');
  expect(rebuiltSrc).not.toBe(cachedSrc);
});
