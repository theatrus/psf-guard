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
      group: {
        index: 0,
        target_id: 2,
        target_name: 'Beta Field',
        filter_name: filterName,
        state: 'ready',
        total_candidates: 3,
        eligible_frames: 3,
        quality_excluded: 0,
        missing_files: 0,
        processed_frames: 3,
        accepted_frames: 3,
        rejected_frames: 0,
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

  const defaultPreviewSrc = await preview.getAttribute('src');
  const stretchControls = panel.locator('.stack-preview-card .stack-stretch-controls');
  await stretchControls.locator('summary').click();
  await expect(stretchControls.getByRole('checkbox', { name: 'Deconvolution' }))
    .not.toBeChecked();
  await expect(stretchControls).toContainText('this is off unless enabled');
  await stretchControls.getByRole('checkbox', { name: 'Deconvolution' }).check();
  await stretchControls.getByRole('spinbutton', { name: 'Deconvolution Iterations' }).fill('1');
  await stretchControls.getByRole('spinbutton', { name: 'Alpha M44 B Target median' }).fill('0.25');
  await stretchControls.getByRole('button', { name: 'Apply processing' }).click();
  await expect.poll(() => preview.getAttribute('src')).toMatch(
    /\/stack-previews\/stretch\/[a-f0-9]{64}\/preview$/
  );
  await expect(stretchControls).toContainText('3.1px deconv · Auto MTF applied');
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

  await page.keyboard.press('Escape');
  await expect(inspector).toHaveCount(0);
  await stretchControls.getByRole('button', { name: 'Revert processing' }).click();
  await expect(preview).toHaveAttribute('src', defaultPreviewSrc!);
  await expect(stretchControls).toContainText('Deconvolution off');

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

  const rgbImage = rgbCard.getByRole('img', { name: /RGB color stack preview/i });
  const defaultRgbSrc = await rgbImage.getAttribute('src');
  const rgbProcessing = rgbCard.locator('.stack-color-processing');
  await rgbProcessing.locator('summary').click();
  const backgroundControls = rgbProcessing.getByRole('region', { name: 'Background extraction' });
  await expect(backgroundControls.getByRole('checkbox', { name: 'Background extraction' }))
    .toBeChecked();
  await expect(backgroundControls.getByLabel('Background fit diagnostics')).toContainText('samples');
  await backgroundControls.getByRole('spinbutton', { name: 'Background Polynomial degree' })
    .fill('1');
  await expect(rgbProcessing.getByRole('region', { name: 'R input stretch stack' }))
    .toContainText('1 stage');
  await expect(rgbProcessing.getByRole('region', { name: 'G input stretch stack' }))
    .toContainText('1 stage');
  await expect(rgbProcessing.getByRole('region', { name: 'B input stretch stack' }))
    .toContainText('1 stage');
  const redLane = rgbProcessing.getByRole('region', { name: 'R input stretch stack' });
  const redDeconvolution = redLane.getByRole('region', { name: 'R input deconvolution' });
  await expect(redDeconvolution.getByRole('checkbox', { name: 'Deconvolution' }))
    .not.toBeChecked();
  await redDeconvolution.getByRole('checkbox', { name: 'Deconvolution' }).check();
  await redDeconvolution.getByRole('spinbutton', { name: 'Deconvolution Iterations' }).fill('2');
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
    .toContainText('Deconvolving R');

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
