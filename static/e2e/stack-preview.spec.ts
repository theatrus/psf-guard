import { expect, test } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import {
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

let dbId: string;

function fitsIntegerCard(keyword: string, value: number): string {
  return `${keyword.padEnd(8)}= ${value.toString().padStart(20)}`.padEnd(80);
}

function fitsFloatCard(keyword: string, value: number): string {
  return `${keyword.padEnd(8)}= ${value.toExponential(12).padStart(20)}`.padEnd(80);
}

function fitsStringCard(keyword: string, value: string): string {
  return `${keyword.padEnd(8)}= '${value}'`.padEnd(80);
}

function fitsNumericValue(header: string, keyword: string): number {
  const card = Array.from({ length: header.length / 80 }, (_, index) =>
    header.slice(index * 80, (index + 1) * 80)
  ).find((candidate) => candidate.slice(0, 8).trim() === keyword);
  expect(card, `missing FITS card ${keyword}`).toBeDefined();
  return Number(card!.slice(10).split('/')[0].trim().replace('D', 'E'));
}

function writeSyntheticMonoStack(destination: string, variant: number): void {
  const width = 512;
  const height = 384;
  const values = new Float32Array(width * height);
  const angle = variant * 0.78;
  const centerX = width * (0.5 + 0.17 * Math.cos(angle));
  const centerY = height * (0.5 + 0.2 * Math.sin(angle));
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const dx = (x - centerX) / (75 + variant * 3);
      const dy = (y - centerY) / (55 + (variant % 3) * 7);
      const broadDx = (x - width * 0.52) / 165;
      const broadDy = (y - height * 0.5) / 120;
      values[y * width + x] = 80 + variant * 4
        + 520 * Math.exp(-(dx * dx + dy * dy) / 2)
        + (120 + variant * 18) * Math.exp(-(broadDx * broadDx + broadDy * broadDy) / 2);
    }
  }
  for (let star = 0; star < 120; star += 1) {
    const cx = 14 + ((star * 83 + 29) % (width - 28));
    const cy = 14 + ((star * 47 + 17) % (height - 28));
    const amplitude = 900 + ((star * (variant + 3) * 137) % 4_000);
    const sigma = 1.15 + (star % 4) * 0.16;
    for (let dy = -5; dy <= 5; dy += 1) {
      for (let dx = -5; dx <= 5; dx += 1) {
        const radius = dx * dx + dy * dy;
        values[(cy + dy) * width + cx + dx] += amplitude * Math.exp(-radius / (2 * sigma * sigma));
      }
    }
  }
  const cards = [
    'SIMPLE  =                    T'.padEnd(80),
    fitsIntegerCard('BITPIX', -32),
    fitsIntegerCard('NAXIS', 2),
    fitsIntegerCard('NAXIS1', width),
    fitsIntegerCard('NAXIS2', height),
    'EXTEND  =                    T'.padEnd(80),
    fitsStringCard('CTYPE1', 'RA---TAN'),
    fitsStringCard('CTYPE2', 'DEC--TAN'),
    fitsStringCard('CUNIT1', 'deg'),
    fitsStringCard('CUNIT2', 'deg'),
    fitsStringCard('RADESYS', 'ICRS'),
    fitsFloatCard('CRVAL1', 130.1),
    fitsFloatCard('CRVAL2', 19.66),
    fitsFloatCard('CRPIX1', width / 2 + 0.5),
    fitsFloatCard('CRPIX2', height / 2 + 0.5),
    fitsFloatCard('CD1_1', -0.0004160277777778),
    fitsFloatCard('CD1_2', 0),
    fitsFloatCard('CD2_1', 0),
    fitsFloatCard('CD2_2', -0.0004160277777778),
    'END'.padEnd(80),
  ];
  const headerText = cards.join('');
  const header = Buffer.alloc(Math.ceil(headerText.length / 2880) * 2880, 0x20);
  header.write(headerText, 0, 'ascii');
  const pixels = Buffer.alloc(values.length * 4);
  for (let index = 0; index < values.length; index += 1) {
    pixels.writeFloatBE(values[index], index * 4);
  }
  const padding = Buffer.alloc((2880 - (pixels.length % 2880)) % 2880);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, Buffer.concat([header, pixels, padding]));
}

function seedSyntheticColorStacks(databaseId: string, projectId: number): void {
  const cacheRoot = path.join(
    process.env.PSF_GUARD_E2E_TMP!, 'cache', databaseId, 'stack-previews'
  );
  const filters = ['L', 'R', 'G', 'B', 'Ha', 'OIII', 'SII'];
  const groups = filters.map((filterName, index) => {
    const jobId = (index + 1).toString(16).padStart(64, '0');
    writeSyntheticMonoStack(path.join(cacheRoot, jobId, 'group-0.fits'), index);
    return {
      job_id: jobId,
      artifact_revision: `synthetic-${index}`,
      accepted_only: false,
      created_unix_seconds: 1_760_000_000 + index,
      // Must track STACK_PREVIEW_CACHE_VERSION in src/server/stack_preview.rs.
      // The server hides remembered stacks from an older build, so a stale
      // number here leaves the color panel with no channels to combine.
      cache_version: 12,
      group: {
        index: 0,
        target_id: 2,
        target_name: 'Beta Field',
        filter_name: filterName,
        state: 'ready',
        phase: 'ready',
        total_candidates: 3,
        eligible_frames: 3,
        quality_excluded: 0,
        missing_files: 0,
        processed_frames: 3,
        accepted_frames: 3,
        rejected_frames: 0,
        output_channels: 1,
        sky_orientation: {
          convention: 'source_frame',
          version: 1,
          source: 'sky_anchor',
          output_width: 512,
          output_height: 384,
          source_to_output: {
            matrix: [[1, 0], [0, 1]],
            translation_x: 0,
            translation_y: 0,
          },
        },
        reference_image_id: 4,
        total_exposure_seconds: 180,
        preview_url: null,
        fits_url: null,
        error: null,
        input_images: [],
        frames: [],
      },
    };
  });
  fs.mkdirSync(cacheRoot, { recursive: true });
  fs.writeFileSync(
    path.join(cacheRoot, `latest-project-${projectId}.json`),
    JSON.stringify({
      schema_version: 1,
      database_id: databaseId,
      project_id: projectId,
      updated_unix_seconds: 1_760_000_100,
      groups,
    }, null, 2)
  );
}

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
  await expect(panel).toContainText('Stack preview');
  // Builds keep the reference frame's rotation, so no sky-orientation marker.
  await expect(panel.locator('.stack-preview-orientation')).toHaveCount(0);

  const preview = panel.getByRole('img', { name: /stack preview/i });
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
  const jobId = fitsHref!.match(/\/stack-previews\/([a-f0-9]{64})\/0\/fits/)![1];
  const fitsHead = await page.request.head(fitsHref!);
  expect(fitsHead.status()).toBe(200);
  expect(fitsHead.headers()['content-type']).toContain('application/fits');
  expect(fitsHead.headers()['content-disposition']).toMatch(/attachment; filename=.*\.fits/);
  expect(Number(fitsHead.headers()['content-length'])).toBeGreaterThan(10_000_000);
  const fitsResponse = await page.request.get(fitsHref!);
  const fitsHeaderText = (await fitsResponse.body()).subarray(0, 2880).toString('ascii');
  expect(fitsHeaderText).toContain('STACKCNT');
  expect(fitsHeaderText).toContain('STACKREJ');
  // The build keeps the reference frame's rotation, so it neither reprojects
  // the pixels nor stamps the canonical sky-up WCS over them.
  expect(fitsHeaderText).not.toContain('SKYORIEN');
  expect(fitsHeaderText).not.toContain('N-UP E-LEFT');

  // The published mapping is what artifact search and background protection
  // read back, so the build must record which way it laid the pixels out.
  const jobResponse = await page.request.get(
    `/api/db/${encodeURIComponent(dbId)}/projects/1/stack-previews/${jobId}`
  );
  expect(jobResponse.status()).toBe(200);
  const orientation = (await jobResponse.json()).data.groups[0].sky_orientation;
  expect(orientation.convention).toBe('source_frame');
  expect(['sky_anchor', 'pier_side', 'exposure_majority']).toContain(orientation.source);
  expect(orientation.output_width).toBeGreaterThan(0);
  expect(orientation.output_height).toBeGreaterThan(0);

  const defaultPreviewSrc = await preview.getAttribute('src');
  const stretchControls = panel.locator('.stack-preview-card .stack-stretch-controls');
  await stretchControls.locator('summary').click();
  await expect(
    stretchControls.locator('.processing-setups-bar').getByRole('combobox', {
      name: 'Saved view processing setups',
    })
  ).toBeVisible();
  await expect(stretchControls.getByRole('checkbox', { name: 'Deconvolution' }))
    .not.toBeChecked();
  await expect(stretchControls).toContainText('this is off unless enabled');
  await stretchControls.getByRole('checkbox', { name: 'Deconvolution' }).check();
  await stretchControls.getByRole('spinbutton', { name: 'Deconvolution Iterations' }).fill('1');
  await stretchControls.getByRole('spinbutton', { name: 'Alpha M44 B Target median' }).fill('0.25');
  await stretchControls.getByRole('button', { name: 'Apply processing' }).click();
  await expect(stretchControls).toContainText(
    '3.1px deconv · Auto MTF applied',
    { timeout: 30_000 }
  );
  await expect(preview).toHaveAttribute(
    'src',
    /\/stack-previews\/stretch\/[a-f0-9]{64}\/preview$/
  );
  if (process.env.PSF_GUARD_CAPTURE_DOCS === '1') {
    const docs = path.resolve(process.cwd(), '..', 'docs');
    fs.mkdirSync(docs, { recursive: true });
    await panel.locator('.stack-preview-card').screenshot({
      path: path.join(docs, 'stack-preview-stretch.png'),
    });
  }

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
  expect(fullSizeSrc).toContain('/stack-previews/stretch/');
  expect(fullSizeSrc).toContain('size=original');
  const fullSizeHead = await page.request.head(fullSizeSrc!);
  expect(fullSizeHead.status()).toBe(200);
  expect(fullSizeHead.headers()['content-type']).toContain('image/png');

  const deconvolvedFits = inspector.getByRole('link', {
    name: 'Download deconvolved linear FITS',
  });
  const deconvolvedFitsHref = await deconvolvedFits.getAttribute('href');
  expect(deconvolvedFitsHref).toMatch(
    /\/stack-previews\/stretch\/[a-f0-9]{64}\/fits$/
  );
  const deconvolvedResponse = await page.request.get(deconvolvedFitsHref!);
  expect(deconvolvedResponse.status()).toBe(200);
  const deconvolvedHeader = (await deconvolvedResponse.body())
    .subarray(0, 2880)
    .toString('ascii');
  expect(deconvolvedHeader).toContain('SEIZADC');
  expect(deconvolvedHeader).toContain('RL-GAUSS');
  expect(deconvolvedHeader).toContain('DCFWHM');
  expect(deconvolvedHeader).toContain('DCITER');

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

  await inspector.getByRole('button', { name: 'Find source artifact' }).click();

  // Dragging past the largest searchable region clamps the box. It used to
  // vanish mid-drag, which read as the selection cancelling itself. The
  // inspector is at 100%, so one screen pixel is one image pixel here.
  const marquee = inspector.getByTestId('stack-artifact-region');
  const longDragX = Math.min(canvasBox!.width - 20, 760);
  const longDragY = Math.min(canvasBox!.height - 20, 660);
  expect(longDragX, 'canvas too narrow to drag past the limit').toBeGreaterThan(560);
  expect(longDragY, 'canvas too short to drag past the limit').toBeGreaterThan(560);
  await page.mouse.move(canvasBox!.x + 10, canvasBox!.y + 10);
  await page.mouse.down();
  await page.mouse.move(canvasBox!.x + longDragX, canvasBox!.y + longDragY, { steps: 6 });
  await expect(marquee).toBeVisible();
  const midDrag = await marquee.boundingBox();
  expect(Math.round(midDrag!.width)).toBe(512);
  expect(Math.round(midDrag!.height)).toBe(512);
  await expect(inspector.locator('.stack-inspector-hint')).toContainText('512px is as wide');
  await page.mouse.up();
  await expect(marquee).toBeVisible();
  await expect(inspector.getByRole('button', { name: 'Search this region' })).toBeVisible();

  await inspector.getByRole('button', { name: 'Find source artifact' }).click();
  await page.mouse.move(canvasBox!.x + 160, canvasBox!.y + 140);
  await page.mouse.down();
  await page.mouse.move(canvasBox!.x + 280, canvasBox!.y + 230, { steps: 4 });
  await page.mouse.up();
  await expect(inspector.getByTestId('stack-artifact-region')).toBeVisible();
  await inspector.getByRole('button', { name: 'Search this region' }).click();
  const suspects = inspector.getByRole('complementary', { name: 'Source-frame search results' });
  await expect(suspects).toContainText('Source-frame ranking');
  await expect(suspects.locator('.stack-artifact-result')).toHaveCount(3, { timeout: 90_000 });
  await expect(suspects.locator('.stack-artifact-result').first()).toContainText('σ peak');
  await expect(suspects.getByRole('button', { name: 'Inspect source image' })).toHaveCount(3);
  for (const crop of await suspects.locator('.stack-artifact-result > img').all()) {
    await expect.poll(() => crop.evaluate((image) => (
      image instanceof HTMLImageElement && image.complete && image.naturalWidth > 0
    ))).toBe(true);
  }

  if (process.env.PSF_GUARD_CAPTURE_DOCS === '1') {
    const docs = path.resolve(process.cwd(), '..', 'docs');
    fs.mkdirSync(docs, { recursive: true });
    await inspector.screenshot({ path: path.join(docs, 'stack-artifact-finder.png') });
  }

  await page.keyboard.press('Escape');
  await expect(inspector).toHaveCount(0);
  const firstProcessedSrc = await preview.getAttribute('src');
  await stretchControls.getByRole('spinbutton', { name: 'Alpha M44 B Target median' }).fill('0.3');
  await stretchControls.getByRole('button', { name: 'Apply processing' }).click();
  await expect.poll(() => preview.getAttribute('src')).not.toBe(firstProcessedSrc);
  const deconvolutionRoot = path.join(
    process.env.PSF_GUARD_E2E_TMP!, 'cache', dbId, 'stack-previews', 'deconvolution'
  );
  expect(fs.readdirSync(deconvolutionRoot)).toHaveLength(1);
  await stretchControls.getByRole('button', { name: 'Revert processing' }).click();
  await expect(preview).toHaveAttribute('src', defaultPreviewSrc!);
  await expect(stretchControls).toContainText('Deconvolution off');

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
  await expect(panel.getByRole('img', { name: /stack preview/i })).toBeVisible();
  await acceptedOnly.uncheck();
  await expect(panel.locator('.stack-preview-card')).toHaveAttribute('data-outdated', 'false');

  await page.goto(
    `/#/grid?db=${encodeURIComponent(dbId)}&project=1&search=no-such-stack-target`
  );
  await expect(page.locator('.stack-preview-outdated')).toContainText(
    'not in the current input'
  );
  await expect(page.getByRole('img', { name: /stack preview/i })).toBeVisible();
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  await expect(page.locator('.stack-preview-card')).toHaveAttribute('data-outdated', 'false');

  // The last successful per-channel result survives navigation and restart-like
  // page reloads without starting another stack job.
  const rememberedSrc = await panel
    .getByRole('img', { name: /stack preview/i })
    .getAttribute('src');
  await page.reload();
  const reloadedPanel = page.locator('.stack-preview-panel');
  await expect(reloadedPanel.locator('.stack-preview-results')).toHaveAttribute(
    'data-job-state', 'remembered'
  );
  await expect(reloadedPanel.getByRole('img', { name: /stack preview/i })).toBeVisible();
  expect(
    await reloadedPanel.getByRole('img', { name: /stack preview/i }).getAttribute('src')
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
    .getByRole('img', { name: /stack preview/i })
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
    .getByRole('img', { name: /stack preview/i })
    .getAttribute('src');
  expect(rebuiltSrc).not.toBe(cachedSrc);
});

test('keeps a running stack visible in the header and re-attaches the panel', async ({
  page,
}) => {
  // A real fixture stack finishes in seconds, so the in-flight report is
  // served from a stub. The server side of the same view is covered by the
  // `StackPreviewManager::active` unit tests.
  const jobId = 'a'.repeat(64);
  const group = {
    index: 0,
    target_id: 1,
    target_name: 'Alpha M44',
    filter_name: 'B',
    state: 'running',
    phase: 'stacking',
    total_candidates: 3,
    eligible_frames: 3,
    quality_excluded: 0,
    missing_files: 0,
    processed_frames: 1,
    accepted_frames: 1,
    rejected_frames: 0,
    output_channels: 1,
    reference_image_id: 1,
    total_exposure_seconds: 180,
    preview_url: null,
    fits_url: null,
    error: null,
    input_images: [],
    frames: [],
  };
  await page.route('**/api/stack-activity', (route) => route.fulfill({
    json: {
      success: true,
      data: {
        schema_version: 1,
        active: [{
          kind: 'mono',
          job_id: jobId,
          database_id: dbId,
          project_id: 1,
          state: 'running',
          label: 'Alpha M44 · B',
          detail: 'Registering frames',
          processed_units: 1,
          total_units: 3,
          created_unix_seconds: 1_760_000_000,
        }],
      },
      error: null,
      status: 'ready',
    },
  }));
  await page.route(`**/stack-previews/${jobId}`, (route) => route.fulfill({
    json: {
      success: true,
      data: {
        schema_version: 2,
        job_id: jobId,
        database_id: dbId,
        project_id: 1,
        state: 'running',
        accepted_only: false,
        created_unix_seconds: 1_760_000_000,
        artifact_revision: 'stub',
        cache_version: 7,
        stacking_version: '0.2.0',
        groups: [group],
        error: null,
      },
      error: null,
      status: 'ready',
    },
  }));

  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);

  // The panel re-attaches to a build it did not start.
  const progress = page.locator('.stack-preview-progress');
  await expect(progress).toHaveAttribute('data-stack-state', 'running', { timeout: 15_000 });
  await expect(progress).toContainText('1/3 frames');

  // The header keeps reporting it from every view.
  const headerStacking = page.locator('.header-cache-slot .stack-activity-status');
  await expect(headerStacking).toContainText('Stacking');
  await expect(headerStacking).toContainText('Alpha M44 · B · 1/3 frames');
  await page.getByRole('button', { name: 'Sequence' }).click();
  await expect(headerStacking).toContainText('Stacking');
  await page.getByRole('button', { name: 'Overview' }).click();
  await expect(headerStacking).toContainText('Stacking');
});

test('composes cached channel stacks into RGB, LRGB, and selectable narrowband previews', async ({
  page,
}) => {
  test.setTimeout(180_000);
  seedSyntheticColorStacks(dbId, 2);
  await page.setViewportSize({ width: 1440, height: 1800 });
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=2`);

  const section = page.locator('.stack-color-section');
  await expect(section).toBeVisible();
  await expect(section).toContainText('Combine channel stacks');
  const rgbButton = section.getByRole('button', { name: 'Build RGB color preview' });
  const rgbCard = section.locator('.stack-color-card[data-color-kind="rgb"]');
  const lrgbButton = section.getByRole('button', { name: 'Build LRGB color preview' });
  const lrgbCard = section.locator('.stack-color-card[data-color-kind="lrgb"]');
  const palette = section.getByRole('combobox', { name: 'Beta Field narrowband palette' });
  await expect(palette.locator('option')).toHaveCount(9);
  await expect(palette).toHaveValue('sho');

  await rgbButton.click();
  await expect(rgbCard.locator('.stack-preview-progress')).toHaveAttribute(
    'data-stack-color-state', 'completed', { timeout: 90_000 }
  );
  await expect(rgbCard.locator('.stack-preview-progress')).toContainText('23/23 steps');
  await expect(rgbCard.getByRole('img', { name: /RGB color stack preview/i })).toBeVisible();
  const rgbFits = rgbCard.getByRole('link', { name: 'Download RGB FITS' });
  const rgbResponse = await page.request.get((await rgbFits.getAttribute('href'))!);
  expect(rgbResponse.status()).toBe(200);
  const rgbHeader = (await rgbResponse.body()).subarray(0, 2880).toString('ascii');
  expect(rgbHeader).toContain('COLORSPC');
  expect(rgbHeader).toContain('RGB');
  expect(rgbHeader).toContain('DISPLAY');
  expect(rgbHeader).not.toContain('SKYORIEN');
  // The composite still inherits the reference channel's WCS.
  expect(fitsNumericValue(rgbHeader, 'CD1_1')).toBeLessThan(0);
  expect(fitsNumericValue(rgbHeader, 'CD2_2')).toBeLessThan(0);

  const rgbImage = rgbCard.getByRole('img', { name: /RGB color stack preview/i });
  const defaultRgbSrc = await rgbImage.getAttribute('src');
  const rgbProcessing = rgbCard.locator('.stack-color-processing');
  await rgbProcessing.locator('summary').click();
  const backgroundControls = rgbProcessing.getByRole('region', { name: 'Background extraction' });
  await expect(backgroundControls.getByRole('checkbox', { name: 'Background extraction' }))
    .toBeChecked();
  await expect(backgroundControls.getByLabel('Background fit diagnostics')).toContainText('samples');
  await expect(backgroundControls.getByRole('combobox', { name: 'Background surface model' }))
    .toHaveValue('automatic');
  await expect(backgroundControls.getByRole('checkbox', {
    name: 'Allow radial-basis background model',
  })).not.toBeChecked();
  await backgroundControls.getByRole('spinbutton', { name: 'Background Maximum degree' })
    .fill('1');
  await backgroundControls.getByRole('spinbutton', { name: 'Background Strength' }).fill('0.8');
  await expect(rgbProcessing.getByRole('region', { name: 'R input stretch stack' }))
    .toContainText('1 stage');
  await expect(rgbProcessing.getByRole('region', { name: 'G input stretch stack' }))
    .toContainText('1 stage');
  await expect(rgbProcessing.getByRole('region', { name: 'B input stretch stack' }))
    .toContainText('1 stage');
  const greenLane = rgbProcessing.getByRole('region', { name: 'G input stretch stack' });
  const greenDeconvolution = greenLane.getByRole('region', { name: 'G input deconvolution' });
  await expect(greenDeconvolution.getByRole('checkbox', { name: 'Deconvolution' }))
    .not.toBeChecked();
  await greenDeconvolution.getByRole('checkbox', { name: 'Deconvolution' }).check();
  await greenDeconvolution.getByRole('spinbutton', { name: 'Deconvolution Iterations' }).fill('2');
  const outputLane = rgbProcessing.getByRole('region', { name: 'RGB output stretch stack' });
  await outputLane.getByRole('button', { name: 'Add stage' }).click();
  await outputLane.getByRole('combobox', { name: 'RGB output stage 1 stretch color strategy' })
    .selectOption('luminance-preserving');
  await outputLane.getByRole('spinbutton', { name: 'RGB output stage 1 Target median' })
    .fill('0.25');
  await rgbProcessing.getByRole('button', { name: 'Apply processing stack' }).click();
  await expect(rgbCard.locator('.stack-preview-progress')).toHaveAttribute(
    'data-stack-color-state', 'completed', { timeout: 90_000 }
  );
  await expect.poll(() => rgbImage.getAttribute('src')).not.toBe(defaultRgbSrc);
  await expect(rgbCard.locator('.stack-preview-progress')).toContainText('25/25 steps');
  const phaseDetails = rgbCard.locator('.stack-color-phase-details');
  await phaseDetails.locator('summary').click();
  await expect(phaseDetails.locator('li')).toHaveCount(12);
  await expect(phaseDetails.locator('li[data-phase="background_preparation"]'))
    .toHaveAttribute('data-phase-state', 'completed');
  await expect(phaseDetails.locator('li[data-phase="background_preparation"]'))
    .toContainText('Correcting B background');
  await expect(phaseDetails.locator('li[data-phase="stretching_output"]'))
    .toHaveAttribute('data-phase-state', 'completed');
  await expect(phaseDetails.locator('li[data-phase="stretching_output"]'))
    .toContainText('Applied output stretch 1/1');
  await expect(phaseDetails.locator('li[data-phase="deconvolving_inputs"]'))
    .toHaveAttribute('data-phase-state', 'completed');
  await expect(phaseDetails.locator('li[data-phase="deconvolving_inputs"]'))
    .toContainText('Deconvolving G');

  const colorInputRoot = path.join(
    process.env.PSF_GUARD_E2E_TMP!, 'cache', dbId, 'stack-previews', 'color-inputs'
  );
  const colorInputsBeforeStretchEdit = fs.readdirSync(colorInputRoot).sort();
  const currentProcessing = rgbCard.locator('.stack-color-processing');
  await currentProcessing.locator(':scope > summary').click();
  const currentOutputLane = currentProcessing.getByRole('region', {
    name: 'RGB output stretch stack',
  });
  await currentOutputLane.getByRole('spinbutton', { name: 'RGB output stage 1 Target median' })
    .fill('0.3');
  await currentProcessing.getByRole('button', { name: 'Apply processing stack' }).click();
  await expect(rgbCard.locator('.stack-preview-progress')).toHaveAttribute(
    'data-stack-color-state', 'completed', { timeout: 90_000 }
  );
  for (const phase of [
    'loading_sources',
    'background_preparation',
    'registering_sources',
    'deconvolving_inputs',
    'normalizing_inputs',
  ]) {
    await expect(phaseDetails.locator(`li[data-phase="${phase}"]`))
      .toHaveAttribute('data-phase-state', 'reused');
  }
  expect(fs.readdirSync(colorInputRoot).sort()).toEqual(colorInputsBeforeStretchEdit);

  await lrgbButton.click();
  await expect(lrgbCard.locator('.stack-preview-progress')).toHaveAttribute(
    'data-stack-color-state', 'completed', { timeout: 90_000 }
  );
  await expect(lrgbCard.locator('.stack-preview-progress')).toContainText('29/29 steps');
  const lrgbImage = lrgbCard.getByRole('img', { name: /LRGB color stack preview/i });
  await expect(lrgbImage).toBeVisible();
  const lrgbFits = lrgbCard.getByRole('link', { name: 'Download LRGB RGB FITS' });
  const lrgbResponse = await page.request.get((await lrgbFits.getAttribute('href'))!);
  expect(lrgbResponse.status()).toBe(200);
  const lrgbHeader = (await lrgbResponse.body()).subarray(0, 2880).toString('ascii');
  expect(lrgbHeader).toContain('COLORSPC');
  expect(lrgbHeader).toContain('LRGB');
  expect(lrgbHeader).toContain('DISPLAY');

  await lrgbCard.getByRole('button', { name: 'Inspect LRGB full size' }).click();
  const inspector = page.getByRole('dialog', { name: /Beta Field/i });
  await expect(inspector).toBeVisible();
  const inspectorImage = inspector.getByTestId('stack-inspector-image');
  await page.waitForFunction(
    (element) => element instanceof HTMLImageElement && element.naturalWidth === 512,
    await inspectorImage.elementHandle(),
    { timeout: 30_000 }
  );
  await expect(inspector).toContainText('512 × 384');
  await page.keyboard.press('Escape');

  await palette.selectOption('foraxx-hoo');
  const foraxxButton = section.getByRole('button', { name: 'Build Foraxx HOO color preview' });
  const narrowbandCard = section.locator('.stack-color-card[data-color-kind="narrowband"]');
  await expect(narrowbandCard.locator('.stack-preview-progress')).toContainText('0/2 steps');
  await foraxxButton.click();
  await expect(narrowbandCard.locator('.stack-preview-progress')).toHaveAttribute(
    'data-stack-color-state', 'completed', { timeout: 90_000 }
  );
  await expect(narrowbandCard.locator('.stack-preview-progress')).toContainText('17/17 steps');
  await expect(
    narrowbandCard.getByRole('img', { name: /Foraxx HOO color stack preview/i })
  ).toBeVisible();
  const foraxxFits = narrowbandCard.getByRole('link', {
    name: 'Download Foraxx HOO RGB FITS',
  });
  const foraxxResponse = await page.request.get((await foraxxFits.getAttribute('href'))!);
  expect(foraxxResponse.status()).toBe(200);
  const foraxxHeader = (await foraxxResponse.body()).subarray(0, 2880).toString('ascii');
  expect(foraxxHeader).toContain('FORAXX-HOO');
  expect(foraxxHeader).toContain('DISPLAY');

  // The narrowband header's worst case: a built card carries a status badge
  // and three buttons, and SHO is the widest palette label. The colour grid is
  // two fixed columns, so a laptop-width window is what users actually get.
  await palette.selectOption('sho');
  await section.getByRole('button', { name: 'Build SHO color preview' }).click();
  await expect(narrowbandCard.locator('.stack-preview-progress')).toHaveAttribute(
    'data-stack-color-state', 'completed', { timeout: 90_000 }
  );
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.waitForTimeout(300);
  const header = await narrowbandCard.evaluate((card) => {
    const row = card.querySelector('header')!;
    const left = row.querySelector(':scope > div:first-child')!.getBoundingClientRect();
    const actions = row.querySelector('.stack-preview-card-actions')!.getBoundingClientRect();
    const title = card.querySelector('h3')!.getBoundingClientRect();
    return {
      overlap: Math.round(left.right - actions.left),
      sameRow: Math.abs(left.top - actions.top) < 4,
      overflow: row.scrollWidth - row.clientWidth,
      titleWidth: Math.round(title.width),
      actionItems: row.querySelectorAll('.stack-preview-card-actions > *').length,
    };
  });
  expect(header.actionItems, 'expected a built card with its full actions').toBeGreaterThan(3);
  // Either the two groups share a row without touching, or the actions wrapped
  // onto their own. Neither may push the row wider than the card.
  if (header.sameRow) {
    expect(header.overlap, 'header groups overlap').toBeLessThanOrEqual(1);
  }
  expect(header.overflow, 'narrowband header overflows its card').toBeLessThanOrEqual(1);
  // The card has to say which target it belongs to. A longer target name than
  // this fixture's squeezes the title to nothing when the row cannot wrap.
  expect(header.titleWidth, 'target name squeezed out of the header').toBeGreaterThan(0);

  // Named processing setups: save the RGB pipeline, see it offered on the
  // narrowband card (setups are global, scoped only by kind), survive a
  // reload, round-trip through export/import, and delete cleanly.
  if (!(await currentProcessing.getAttribute('open'))) {
    await currentProcessing.locator(':scope > summary').click();
  }
  const rgbSetupsBar = currentProcessing.locator('.processing-setups-bar');
  await rgbSetupsBar.getByRole('button', { name: 'Save as…' }).click();
  await rgbSetupsBar.getByRole('textbox', { name: 'New setup name' }).fill('E2E color pipeline');
  await rgbSetupsBar.getByRole('button', { name: 'Save current settings' }).click();
  await expect(rgbSetupsBar).toContainText('Saved “E2E color pipeline”');

  const setupsResponse = await page.request.get('/api/processing-setups');
  expect(setupsResponse.status()).toBe(200);
  const setupsDocument = (await setupsResponse.json()).data;
  const savedSetup = setupsDocument.setups.find(
    (setup: { name: string }) => setup.name === 'E2E color pipeline'
  );
  expect(savedSetup.kind).toBe('color');
  // The saved pipeline carries the edited output stretch, canonicalized.
  expect(JSON.stringify(savedSetup.settings)).toContain('"target_median":0.3');

  // The same setup is offered on the narrowband card and applies to its
  // different channel set.
  const narrowbandProcessing = narrowbandCard.locator('.stack-color-processing');
  if (!(await narrowbandProcessing.getAttribute('open'))) {
    await narrowbandProcessing.locator(':scope > summary').click();
  }
  const narrowbandBar = narrowbandProcessing.locator('.processing-setups-bar');
  const narrowbandSelect = narrowbandBar.getByRole('combobox', {
    name: 'Saved color processing setups',
  });
  await narrowbandSelect.selectOption({ label: 'E2E color pipeline' });
  await narrowbandBar.getByRole('button', { name: 'Apply setup' }).click();
  await expect(narrowbandBar).toContainText('Applied “E2E color pipeline”');
  await expect(
    narrowbandProcessing.getByRole('spinbutton', { name: 'RGB output stage 1 Target median' })
  ).toHaveValue('0.3');

  // The registry survives a reload: the file sits beside the e2e registry.
  await page.reload();
  await expect(section).toBeVisible({ timeout: 15_000 });
  const reloadedProcessing = rgbCard.locator('.stack-color-processing');
  await reloadedProcessing.locator(':scope > summary').click();
  const reloadedBar = reloadedProcessing.locator('.processing-setups-bar');
  const reloadedSelect = reloadedBar.getByRole('combobox', {
    name: 'Saved color processing setups',
  });
  await expect(reloadedSelect.locator('option', { hasText: 'E2E color pipeline' }))
    .toHaveCount(1);

  // Management moved to Settings → Setups: export the collection, re-import
  // a renamed copy, and delete it from the table.
  await page.getByRole('button', { name: 'Settings' }).click();
  const settingsModal = page.locator('.tauri-settings .modal-content');
  await settingsModal.getByRole('tab', { name: 'Setups' }).click();
  const manager = settingsModal.locator('.processing-setups-manager');
  await expect(manager.locator('tbody tr')).toHaveCount(1);
  await expect(manager).toContainText('E2E color pipeline');
  await expect(manager).toContainText('Color pipeline');

  const downloadPromise = page.waitForEvent('download');
  await manager.getByRole('button', { name: 'Export all' }).click();
  const download = await downloadPromise;
  const exportPath = path.join(process.env.PSF_GUARD_E2E_TMP!, 'setups-export.json');
  await download.saveAs(exportPath);
  const exported = JSON.parse(fs.readFileSync(exportPath, 'utf8'));
  expect(exported.setups.map((setup: { name: string }) => setup.name))
    .toContain('E2E color pipeline');

  // A row exports on its own, in the same document shape.
  const singlePromise = page.waitForEvent('download');
  await manager
    .locator('tbody tr', { hasText: 'E2E color pipeline' })
    .getByRole('button', { name: 'Export' })
    .click();
  const single = await singlePromise;
  expect(single.suggestedFilename()).toBe('psf-guard-setup-e2e-color-pipeline.json');
  const singlePath = path.join(process.env.PSF_GUARD_E2E_TMP!, 'setup-single.json');
  await single.saveAs(singlePath);
  const singleDocument = JSON.parse(fs.readFileSync(singlePath, 'utf8'));
  expect(singleDocument.setups).toHaveLength(1);
  expect(singleDocument.setups[0].name).toBe('E2E color pipeline');

  const renamed = {
    ...exported,
    setups: exported.setups
      .filter((setup: { name: string }) => setup.name === 'E2E color pipeline')
      .map((setup: { name: string }) => ({ ...setup, name: 'Imported pipeline' })),
  };
  const importPath = path.join(process.env.PSF_GUARD_E2E_TMP!, 'setups-import.json');
  fs.writeFileSync(importPath, JSON.stringify(renamed));
  await manager.locator('input[type="file"]').setInputFiles(importPath);
  await expect(manager).toContainText('Imported 1 new, replaced 0');
  await expect(manager.locator('tbody tr')).toHaveCount(2);

  const importedRow = manager.locator('tbody tr', { hasText: 'Imported pipeline' });
  await importedRow.getByRole('button', { name: 'Delete' }).click();
  await expect(manager).toContainText('Deleted “Imported pipeline”');
  await expect(manager.locator('tbody tr')).toHaveCount(1);
  await settingsModal.locator('.close-button').click();

  // The whole stack panel collapses like the detail sections do, and the
  // preference survives a reload.
  const stackPanel = page.locator('.stack-preview-panel');
  const collapseToggle = stackPanel.locator('.stack-preview-collapse');
  await collapseToggle.click();
  await expect(stackPanel).toHaveAttribute('data-collapsed', 'true');
  await expect(stackPanel.locator('.stack-color-card')).toHaveCount(0);
  await expect(stackPanel).toContainText('remembered channel');
  await page.reload();
  await expect(stackPanel).toBeVisible({ timeout: 15_000 });
  await expect(stackPanel).toHaveAttribute('data-collapsed', 'true');
  await collapseToggle.click();
  await expect(stackPanel).not.toHaveAttribute('data-collapsed', 'true');
  await expect(stackPanel.locator('.stack-color-card').first()).toBeVisible();

  // At a large grid zoom the result cards follow: one full-width column for
  // both the mono and the color grids. Zooming back returns the two-column
  // layout — the cards never get narrower than it.
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=2&size=700`);
  await expect(stackPanel).toBeVisible({ timeout: 15_000 });
  await expect(stackPanel).toHaveAttribute('data-wide', 'true');
  const wideColumns = await stackPanel.locator('.stack-color-grid').first().evaluate(
    (grid) => getComputedStyle(grid).gridTemplateColumns.split(' ').filter(Boolean).length
  );
  expect(wideColumns).toBe(1);
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=2&size=300`);
  await expect(stackPanel).toBeVisible({ timeout: 15_000 });
  await expect(stackPanel).not.toHaveAttribute('data-wide', 'true');
  const normalColumns = await stackPanel.locator('.stack-color-grid').first().evaluate(
    (grid) => getComputedStyle(grid).gridTemplateColumns.split(' ').filter(Boolean).length
  );
  expect(normalColumns).toBe(2);

  if (process.env.PSF_GUARD_CAPTURE_DOCS === '1') {
    const docs = path.resolve(process.cwd(), '..', 'docs');
    fs.mkdirSync(docs, { recursive: true });
    await section.screenshot({ path: path.join(docs, 'stack-color-previews.png') });
    const currentRgbProcessing = rgbCard.locator('.stack-color-processing');
    if (!(await currentRgbProcessing.getAttribute('open'))) {
      await currentRgbProcessing.locator('summary').click();
    }
    await rgbCard.screenshot({ path: path.join(docs, 'stack-color-processing.png') });
  }
});
