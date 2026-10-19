import { expect, test, type Locator, type Page } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import type { StackCalibrationMasters } from '../src/api/types';
import { registerFixtureDb, resetDatabases, waitForCacheReady, openSettings } from './helpers';

let dbId: string;

/**
 * Rebuild the panel's one channel and wait until that run has finished.
 * Until the new run's status reaches the page, the results still read the
 * last run's `completed`, so waiting on the page alone can return before
 * the rebuild has begun. The run is followed through the API instead.
 */
async function rebuildChannel(page: Page, panel: Locator): Promise<void> {
  const started = page.waitForResponse((response) =>
    response.request().method() === 'POST'
    && /\/projects\/\d+\/stack-previews(\/channels)?$/.test(new URL(response.url()).pathname));
  await panel.getByRole('button', { name: 'Rebuild channel', exact: true }).click();
  const body = await (await started).json();
  const [job] = Array.isArray(body.data) ? body.data : [body.data];
  expect(job?.job_id, JSON.stringify(body)).toBeTruthy();
  // The click queues the build as being prepared; once prepared it names
  // the build it became, which is the one to follow.
  const jobUrl = (jobId: string) =>
    `/api/db/${encodeURIComponent(dbId)}/projects/${job.project_id}/stack-previews/${encodeURIComponent(jobId)}`;
  expect(job.state).toBe('preparing');
  let built = '';
  await expect
    .poll(async () => {
      const prepared = (await (await page.request.get(jobUrl(job.job_id))).json()).data;
      built = prepared?.replaced_by?.[0] ?? '';
      return prepared?.state;
    }, { timeout: 60_000, intervals: [100, 250, 500] })
    .toBe('replaced');
  expect(built).toBeTruthy();
  await expect
    .poll(async () => (await (await page.request.get(jobUrl(built))).json()).data?.state, {
      timeout: 210_000,
      intervals: [250, 500, 1_000, 2_000],
    })
    .toBe('completed');
  await expect(panel.locator('.stack-preview-results')).toHaveAttribute('data-job-state', 'completed', {
    timeout: 30_000,
  });
}

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

function writeSyntheticMonoStack(destination: string, variant: number, calibrationMaster = false): void {
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
    ...(calibrationMaster ? [fitsStringCard('BAYERPAT', 'RGGB')] : []),
    'END'.padEnd(80),
  ];
  const headerText = cards.join('');
  const header = Buffer.alloc(Math.ceil(headerText.length / 2880) * 2880, 0x20);
  header.write(headerText, 0, 'ascii');
  const pixels = Buffer.alloc(values.length * 4);
  for (let index = 0; index < values.length; index += 1) {
    pixels.writeFloatBE(calibrationMaster ? values[index] / 10_000 : values[index], index * 4);
  }
  const padding = Buffer.alloc((2880 - (pixels.length % 2880)) % 2880);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, Buffer.concat([header, pixels, padding]));
}

function seedSyntheticColorStacks(databaseId: string, projectId: number, withCalibration = false): void {
  const cacheRoot = path.join(
    process.env.PSF_GUARD_E2E_TMP!, 'cache', databaseId, 'stack-previews'
  );
  const filters = ['L', 'R', 'G', 'B', 'Ha', 'OIII', 'SII'];
  const groups = filters.map((filterName, index) => {
    const jobId = (index + 1).toString(16).padStart(64, '0');
    writeSyntheticMonoStack(path.join(cacheRoot, jobId, 'group-0.fits'), index);
    const masterRoot = path.join(process.env.PSF_GUARD_E2E_TMP!, 'cache', databaseId, 'calibration-masters');
    const bias = `bias-${'1'.repeat(64)}.fits`;
    const dark = `dark-${'2'.repeat(64)}.fits`;
    const darkFlat = `dark_flat-${'3'.repeat(64)}.fits`;
    const flat = `flat-${(40 + index).toString(16).padStart(64, '0')}.fits`;
    const missingFlat = `flat-${(50 + index).toString(16).padStart(64, '0')}.fits`;
    const signature = `bias=${bias};dark=${dark};dark_flat=${darkFlat};flat=${flat}`;
    const missingSignature = `bias=${bias};dark=${dark};dark_flat=${darkFlat};flat=${missingFlat}`;
    const calibration = {
      mode: 'auto', state: 'applied', bias_frames: 8, dark_frames: 8, dark_flat_frames: 8, flat_frames: 8,
      bias_master: null, dark_master: null, dark_flat_master: null, flat_master: null,
      warning: null, fingerprint: `fixture-${index}`, masters_signature: `${signature}||${missingSignature}`,
      estimated_pedestal_adu: null, sessions: 2,
      session_details: [
        { fingerprint: `session-one-${index}`, masters_signature: signature, estimated_pedestal_adu: null, lights: 2 },
        { fingerprint: `session-two-${index}`, masters_signature: missingSignature, estimated_pedestal_adu: null, lights: 1 },
      ],
    };
    if (withCalibration) {
      for (const [masterIndex, name] of [bias, dark, darkFlat, flat].entries()) {
        writeSyntheticMonoStack(path.join(masterRoot, name), masterIndex, true);
      }
    }
    const remembered = {
      job_id: jobId,
      artifact_revision: `synthetic-${index}`,
      accepted_only: false,
      created_unix_seconds: 1_760_000_000 + index,
      // Must track STACK_PREVIEW_CACHE_VERSION in src/server/stack_preview.rs.
      // The server hides remembered stacks from an older build, so a stale
      // number here leaves the color panel with no channels to combine.
      cache_version: 15,
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
        ...(withCalibration ? { calibration } : {}),
      },
    };
    if (withCalibration) {
      fs.writeFileSync(path.join(cacheRoot, jobId, 'manifest.json'), JSON.stringify({
        schema_version: 1, database_id: databaseId, project_id: projectId,
        job_id: jobId, artifact_revision: remembered.artifact_revision,
        state: 'completed', accepted_only: false,
        created_unix_seconds: remembered.created_unix_seconds,
        cache_version: remembered.cache_version, stacking_version: '0.14.0',
        groups: [remembered.group], error: null,
      }));
    }
    return remembered;
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

/** A one-shot-color stack: three planes of a small star field, as a debayered
 *  build leaves it, with its manifest and the project's latest index. */
function seedSyntheticOscStack(databaseId: string, projectId: number): void {
  const cacheRoot = path.join(process.env.PSF_GUARD_E2E_TMP!, 'cache', databaseId, 'stack-previews');
  const jobId = 'c'.repeat(64);
  const width = 96;
  const height = 64;
  const values = new Float32Array(width * height * 3);
  for (let plane = 0; plane < 3; plane += 1) {
    for (let y = 0; y < height; y += 1) {
      for (let x = 0; x < width; x += 1) {
        const dx = x - 40;
        const dy = y - 30;
        values[plane * width * height + y * width + x] =
          [120, 100, 90][plane] + 900 * Math.exp(-(dx * dx + dy * dy) / 8);
      }
    }
  }
  const cards = [
    'SIMPLE  =                    T'.padEnd(80),
    fitsIntegerCard('BITPIX', -32),
    fitsIntegerCard('NAXIS', 3),
    fitsIntegerCard('NAXIS1', width),
    fitsIntegerCard('NAXIS2', height),
    fitsIntegerCard('NAXIS3', 3),
    'END'.padEnd(80),
  ];
  const headerText = cards.join('');
  const header = Buffer.alloc(Math.ceil(headerText.length / 2880) * 2880, 0x20);
  header.write(headerText, 0, 'ascii');
  const pixels = Buffer.alloc(values.length * 4);
  values.forEach((value, index) => pixels.writeFloatBE(value, index * 4));
  const padding = Buffer.alloc((2880 - (pixels.length % 2880)) % 2880);
  fs.mkdirSync(path.join(cacheRoot, jobId), { recursive: true });
  fs.writeFileSync(path.join(cacheRoot, jobId, 'group-0.fits'), Buffer.concat([header, pixels, padding]));
  const group = {
    index: 0, target_id: 2, target_name: 'Beta Field', filter_name: 'B', state: 'ready',
    phase: 'ready', total_candidates: 3, eligible_frames: 3, quality_excluded: 0,
    missing_files: 0, processed_frames: 3, accepted_frames: 3, rejected_frames: 0,
    output_channels: 3,
    sky_orientation: {
      convention: 'source_frame', version: 1, source: 'sky_anchor',
      output_width: width, output_height: height,
      source_to_output: { matrix: [[1, 0], [0, 1]], translation_x: 0, translation_y: 0 },
    },
    reference_image_id: 4, total_exposure_seconds: 180, preview_url: null, fits_url: null,
    error: null, input_images: [], frames: [],
    // Built when a Gaia catalog was installed that the test server lacks,
    // so the calibrated preview is out of date.
    color_calibration: { status: 'skipped', reason: 'too_few_stars', detail: 'only 3 stars' },
    color_calibration_basis: {
      reference_image_id: 4, solution_fingerprint: null,
      catalog: {
        name: 'gaia_photometry', path: '/gone/stars-gaia-photometry.bin', format: 'SEIZAPH1',
        size_bytes: 1, modified_unix_seconds: 1,
      },
    },
  };
  const remembered = {
    job_id: jobId, artifact_revision: 'synthetic-osc', accepted_only: false,
    created_unix_seconds: 1_760_000_200, cache_version: 15, group,
  };
  fs.writeFileSync(path.join(cacheRoot, jobId, 'manifest.json'), JSON.stringify({
    schema_version: 1, database_id: databaseId, project_id: projectId, job_id: jobId,
    artifact_revision: remembered.artifact_revision, state: 'completed', accepted_only: false,
    created_unix_seconds: remembered.created_unix_seconds, cache_version: 15,
    stacking_version: '0.14.0', groups: [group], error: null,
  }));
  fs.writeFileSync(path.join(cacheRoot, `latest-project-${projectId}.json`), JSON.stringify({
    schema_version: 1, database_id: databaseId, project_id: projectId,
    updated_unix_seconds: 1_760_000_300, groups: [remembered],
  }, null, 2));
}

test('refuses RC-Astro values past the tool range beside Apply, where the server refusals show too', async ({ page }, testInfo) => {
  test.setTimeout(120_000);
  seedSyntheticColorStacks(dbId, 2);
  await page.route('**/api/tools/rc-astro', (route) => route.fulfill({ json: {
    success: true,
    data: {
      available: true,
      executable: 'e2e-capabilities-fixture',
      tools: [{
        schema_version: 6, cli_version: '2.6.6', key: 'bxt',
        name: 'RC-Astro BlurXTerminator', ml_version: 5, licensed: true, license_message: null,
        parameters: [
          { name: 'ss', flag: '--ss', label: 'Sharpen Stars', description: 'Amount of stellar sharpening',
            kind: { type: 'float', default: 0.5, min: 0, max: 0.7 } },
          { name: 'sn', flag: '--sn', label: 'Sharpen Nonstellar', description: 'Amount of nonstellar sharpening',
            kind: { type: 'float', default: 0.5, min: 0, max: 1 } },
        ],
      }],
    }, error: null,
  } }));
  await page.setViewportSize({ width: 1440, height: 1100 });
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=2`);
  const card = page.locator('.stack-color-card[data-color-kind="rgb"]');
  await card.getByRole('button', { name: 'Build RGB color preview' }).click();
  await expect(card.locator('.stack-preview-progress')).toHaveAttribute(
    'data-stack-color-state', 'completed', { timeout: 90_000 }
  );
  let starts = 0;
  await page.route(/\/stack-previews\/color$/, (route) => {
    if (route.request().method() !== 'POST') return route.continue();
    starts += 1;
    return route.fulfill({ status: 400, json: {
      success: false, data: null, error: 'RC-Astro BlurXTerminator is not licensed on this server',
    } });
  });
  const processing = card.locator('.stack-color-processing');
  await processing.locator('summary').click();
  const red = processing.getByRole('region', { name: 'R input stretch stack' });
  await red.getByRole('checkbox', { name: 'RC-Astro BlurXTerminator' }).check();
  await red.getByRole('spinbutton', { name: 'Sharpen Stars' }).fill('0.9');
  await red.getByRole('spinbutton', { name: 'Sharpen Nonstellar' }).fill('0.9');
  await expect(red.getByRole('spinbutton', { name: 'Sharpen Stars' })).toHaveAttribute('aria-invalid', 'true');
  await processing.getByRole('button', { name: 'Apply processing stack' }).click();
  const refusal = processing.getByRole('alert');
  await expect(refusal).toHaveText('R: RC-Astro BlurXTerminator: Sharpen Stars must be between 0 and 0.7');
  await expect(refusal).toBeInViewport();
  expect(starts).toBe(0);
  await processing.screenshot({ path: testInfo.outputPath('rc-astro-range-refusal.png') });

  // Within range, the request goes out; the server's refusal lands beside the button.
  await red.getByRole('spinbutton', { name: 'Sharpen Stars' }).fill('0.6');
  await processing.getByRole('button', { name: 'Apply processing stack' }).click();
  await expect(refusal).toHaveText('RC-Astro BlurXTerminator is not licensed on this server');
  await expect(refusal).toBeInViewport();
  expect(starts).toBe(1);
  await processing.screenshot({ path: testInfo.outputPath('rc-astro-server-refusal.png') });
});

test("copies a channel card's deconvolution into its color input", async ({ page }, testInfo) => {
  test.setTimeout(180_000);
  seedSyntheticColorStacks(dbId, 2);
  await page.setViewportSize({ width: 1440, height: 1100 });
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=2`);

  // The R card shows a deconvolution. The synthetic stacks have no stars to
  // measure a PSF from, so its saved processing comes from a route.
  const redJob = (2).toString(16).padStart(64, '0');
  const deconvolution = {
    psf_fwhm_pixels: 3.1, iterations: 1, amount: 0.35, noise_fraction: 0.001, max_correction: 2,
  };
  await page.route(new RegExp(`/stack-previews/${redJob}/0/stretch\\?`), (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    const model = { type: 'auto-mtf', target_median: 0.2, shadows_clip: -2.8 };
    const stretchId = 'f'.repeat(64);
    return route.fulfill({ json: { success: true, error: null, data: {
      schema_version: 2, stretch_id: stretchId, stretch_version: '0.1.0',
      deconvolution_version: null, deconvolution_id: null,
      config: { model, color_strategy: 'linked', max_analysis_samples: 1 },
      request: { model, color_strategy: 'linked', deconvolution, rc_astro: null },
      resolved_plan: {}, source_transfer: 'linear', input_range: null,
      linked_statistics: { min: 0, max: 1, median: 0.2, mad: 0.1, count: 4 },
      channel_statistics: [], luminance_statistics: null, deconvolution: null,
      preview_url: `/api/db/${dbId}/stack-previews/stretch/${stretchId}/preview`,
      original_preview_url: `/api/db/${dbId}/stack-previews/stretch/${stretchId}/preview?size=original`,
      fits_url: null,
    } } });
  });

  const card = page.locator('.stack-color-card[data-color-kind="rgb"]');
  await card.getByRole('button', { name: 'Build RGB color preview' }).click();
  await expect(card.locator('.stack-preview-progress')).toHaveAttribute(
    'data-stack-color-state', 'completed', { timeout: 90_000 }
  );
  const processing = card.locator('.stack-color-processing');
  await processing.locator('summary').click();
  const channels = processing.getByRole('group', { name: 'Channel processing' });
  await expect(channels).toContainText(
    "Sets each input's deconvolution and RC-Astro steps to what its channel card shows: R: deconvolution."
  );
  const redLane = processing.getByRole('region', { name: 'R input stretch stack' });
  const redDeconvolution = redLane.getByRole('region', { name: 'R input deconvolution' });
  await expect(redDeconvolution.getByRole('checkbox', { name: 'Deconvolution' })).not.toBeChecked();
  await processing.getByRole('button', { name: "Use each channel's processing" }).click();
  await expect(redDeconvolution.getByRole('checkbox', { name: 'Deconvolution' })).toBeChecked();
  await expect(redDeconvolution.getByRole('spinbutton', { name: 'Deconvolution Iterations' }))
    .toHaveValue('1');
  // Only R's card deconvolves, so G and B are left as they were.
  await expect(processing.getByRole('region', { name: 'G input deconvolution' })
    .getByRole('checkbox', { name: 'Deconvolution' })).not.toBeChecked();
  await processing.screenshot({ path: testInfo.outputPath('color-channel-processing.png') });
  await processing.getByRole('button', { name: 'Apply processing stack' }).click();
  // The new composition deconvolves R, which the first one skipped.
  const phaseDetails = card.locator('.stack-color-phase-details');
  await phaseDetails.locator('summary').click();
  await expect(phaseDetails.locator('li[data-phase="deconvolving_inputs"]'))
    .toHaveAttribute('data-phase-state', 'completed', { timeout: 90_000 });
  await expect(card.locator('.stack-preview-progress')).toHaveAttribute(
    'data-stack-color-state', 'completed', { timeout: 90_000 }
  );
});

test('retains RC-Astro color steps in saved setups on desktop and mobile', async ({ page }, testInfo) => {
  test.setTimeout(120_000);
  seedSyntheticColorStacks(dbId, 2);
  await page.route('**/api/tools/rc-astro', (route) => route.fulfill({ json: {
    success: true,
    data: {
      available: true,
      executable: 'e2e-capabilities-fixture',
      tools: [{
        schema_version: 6, cli_version: '2.6.6', key: 'bxt',
        name: 'RC-Astro BlurXTerminator', ml_version: 4, licensed: true,
        license_message: 'Test capability fixture',
        parameters: [{ name: 'ss', flag: '--ss', label: 'Sharpen Stars',
          description: 'Amount of stellar sharpening',
          kind: { type: 'float', default: 0.5, min: 0, max: 0.7 } }],
      }],
    }, error: null,
  } }));
  await page.setViewportSize({ width: 1440, height: 1100 });
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=2`);
  const card = page.locator('.stack-color-card[data-color-kind="rgb"]');
  await card.getByRole('button', { name: 'Build RGB color preview' }).click();
  await expect(card.locator('.stack-preview-progress')).toHaveAttribute(
    'data-stack-color-state', 'completed', { timeout: 90_000 }
  );
  const processing = card.locator('.stack-color-processing');
  await processing.locator('summary').click();
  const red = processing.getByRole('region', { name: 'R input stretch stack' });
  await red.getByRole('checkbox', { name: 'RC-Astro BlurXTerminator' }).check();
  await red.getByRole('spinbutton', { name: 'Sharpen Stars' }).fill('0.2');
  const setups = processing.locator('.processing-setups-bar');
  await setups.getByRole('button', { name: /Save as/ }).click();
  await setups.getByRole('textbox', { name: 'New setup name' }).fill('RC color test');
  await setups.getByRole('button', { name: 'Save current settings', exact: true }).click();
  await expect(setups.getByRole('combobox', { name: 'Saved color processing setups' })
    .locator('option', { hasText: 'RC color test' })).toHaveCount(1);
  const setupsDocument = (await (await page.request.get('/api/processing-setups')).json()).data;
  const saved = setupsDocument.setups.find((entry: { name: string }) => entry.name === 'RC color test');
  expect(saved.settings.input_rc_astro.red.steps).toEqual([{ tool: 'bxt', parameters: { ss: 0.2 } }]);
  await page.reload();
  await processing.locator('summary').click();
  await setups.getByRole('combobox', { name: 'Saved color processing setups' })
    .selectOption({ label: 'RC color test' });
  await setups.getByRole('button', { name: 'Apply setup' }).click();
  await expect(red.getByRole('checkbox', { name: 'RC-Astro BlurXTerminator' })).toBeChecked();
  await expect(red.getByRole('spinbutton', { name: 'Sharpen Stars' })).toHaveValue('0.2');
  const preview = card.locator('.stack-color-image img');
  await expect.poll(() => preview.evaluate((element) => {
    const image = element as HTMLImageElement;
    return image.complete && image.naturalWidth > 0 && image.naturalHeight > 0;
  })).toBe(true);
  await red.evaluate((element) => element.scrollIntoView({ block: 'center' }));
  await red.screenshot({ path: testInfo.outputPath('rc-astro-color-desktop.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(red.getByRole('spinbutton', { name: 'Sharpen Stars' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  const mobileTools = red.locator('.stack-rc-astro-controls');
  await mobileTools.evaluate((element) => element.scrollIntoView({ block: 'center' }));
  expect(await mobileTools.evaluate((element) => element.scrollWidth <= element.clientWidth + 1))
    .toBe(true);
  await mobileTools.screenshot({ path: testInfo.outputPath('rc-astro-color-mobile.png') });
});

test.beforeEach(async ({ request }, testInfo) => {
  await resetDatabases(request);
  // Setups are global and survive a database reset.
  const setups = (await (await request.get('/api/processing-setups')).json()).data.setups;
  for (const setup of setups) {
    const response = await request.delete(`/api/processing-setups/${encodeURIComponent(setup.name)}`);
    expect(response.ok()).toBe(true);
  }
  const entry = await registerFixtureDb(request, {
    name: 'Stack Preview e2e',
    // Removing a registry entry preserves its cache; each test needs its own slug.
    slug: `stack-preview-e2e-${testInfo.line}-${testInfo.repeatEachIndex}-${testInfo.retry}`,
  });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
  // The fixture mixes exposure lengths within a filter. These tests cover
  // one stack per channel, so both projects choose to stack exposures together.
  for (const projectId of [1, 2]) {
    const mixed = await request.put(
      `/api/db/${encodeURIComponent(dbId)}/projects/${projectId}/processing-settings`,
      { data: { split_exposure_groups: false } },
    );
    expect(mixed.ok()).toBe(true);
  }
});

test('inspects exact calibration masters across mono sessions and color channels', async ({ page, request }, testInfo) => {
  test.setTimeout(180_000);
  seedSyntheticColorStacks(dbId, 2, true);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.route('**/images/*/preview?*', (route) => route.abort());
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=2`);
  const catalogResponse = page.waitForResponse((response) => response.url().includes('/calibration-masters?'));
  await page.getByRole('button', { name: 'Inspect calibration masters for R', exact: true }).click();
  const catalog: StackCalibrationMasters = (await (await catalogResponse).json()).data;
  expect(catalog.masters).toHaveLength(5);
  expect(catalog.masters.filter((master) => master.available)).toHaveLength(4);
  expect(catalog.masters.find((master) => master.kind === 'bias')?.usages).toHaveLength(2);
  const dialog = page.getByRole('dialog', { name: /Beta Field/ });
  const selector = dialog.getByRole('combobox', { name: 'Master' });
  const image = dialog.getByTestId('stack-inspector-image');

  for (const master of catalog.masters.filter((entry) => entry.available)) {
    await selector.selectOption(master.id);
    await expect(image).toHaveAttribute('src', new RegExp(master.id));
    await expect(image).toBeVisible({ timeout: 30_000 });
    await expect.poll(() => image.evaluate((element: HTMLImageElement) => element.complete && element.naturalWidth)).toBe(512);
    const pixels = await image.evaluate((element: HTMLImageElement) => {
      const canvas = document.createElement('canvas');
      canvas.width = element.naturalWidth;
      canvas.height = element.naturalHeight;
      const context = canvas.getContext('2d')!;
      context.drawImage(element, 0, 0);
      const values = context.getImageData(0, 0, canvas.width, canvas.height).data;
      const levels = new Set<number>();
      let mono = true;
      for (let index = 0; index < values.length; index += 4) {
        levels.add(values[index]);
        mono &&= values[index] === values[index + 1] && values[index] === values[index + 2];
      }
      return { levels: levels.size, mono };
    });
    expect(pixels.levels).toBeGreaterThan(32);
    expect(pixels.mono).toBe(true);
    const download = await request.get(master.fits_url!);
    expect(download.ok()).toBe(true);
    const original = fs.readFileSync(path.join(process.env.PSF_GUARD_E2E_TMP!, 'cache', dbId, 'calibration-masters', master.label));
    expect(Buffer.compare(await download.body(), original)).toBe(0);
  }

  const availableFlat = catalog.masters.find((master) => master.kind === 'flat' && master.available)!;
  await selector.selectOption(availableFlat.id);
  await expect(image).toHaveAttribute('src', new RegExp(availableFlat.id));
  await expect(image).toBeVisible();
  await dialog.getByRole('button', { name: '100%', exact: true }).click();
  const transform = await image.evaluate((element) => element.style.transform);
  const midtone = dialog.getByRole('slider', { name: 'Master midtone' });
  await midtone.focus();
  for (let step = 0; step < 15; step += 1) await midtone.press('ArrowRight');
  await expect(image).toHaveAttribute('src', /midtone=0.35/);
  await expect(image).toBeVisible({ timeout: 30_000 });
  expect(await image.evaluate((element) => element.style.transform)).toBe(transform);
  await dialog.screenshot({ path: testInfo.outputPath('calibration-masters-desktop.png') });

  await page.setViewportSize({ width: 390, height: 844 });
  await dialog.getByRole('button', { name: 'Fit', exact: true }).click();
  await expect(dialog).toBeInViewport();
  await expect(dialog.getByRole('link', { name: 'Download master FITS' })).toBeVisible();
  expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  expect(await dialog.locator('.calibration-master-controls').evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  expect(await image.evaluate((element) => element.getBoundingClientRect().height)).toBeGreaterThan(100);
  await dialog.screenshot({ path: testInfo.outputPath('calibration-masters-mobile.png') });
  await selector.selectOption(catalog.masters.find((master) => !master.available)!.id);
  await expect(dialog.getByRole('status').filter({ hasText: /master|file|available/i })).toBeVisible();
  await expect(image).toHaveCount(0);
  await expect(dialog.getByRole('link', { name: 'Download master FITS' })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Close calibration master inspector' }).click();

  await page.setViewportSize({ width: 1440, height: 1000 });
  const rgb = page.locator('.stack-color-card[data-color-kind="rgb"]').first();
  await rgb.getByRole('button', { name: 'Build RGB color preview', exact: true }).click();
  await expect(rgb.locator('[data-stack-color-state]')).toHaveAttribute('data-stack-color-state', 'completed', { timeout: 90_000 });
  const colorResponse = page.waitForResponse((response) => response.url().includes('/color/') && response.url().includes('/calibration-masters?'));
  await rgb.getByRole('button', { name: 'Inspect calibration masters for RGB', exact: true }).click();
  const colorCatalog: StackCalibrationMasters = (await (await colorResponse).json()).data;
  const sharedBias = colorCatalog.masters.filter((master) => master.kind === 'bias');
  expect(sharedBias).toHaveLength(1);
  expect(sharedBias[0].usages).toHaveLength(6);
  await dialog.getByRole('combobox', { name: 'Master' }).selectOption(sharedBias[0].id);
  await expect(dialog.getByTestId('stack-inspector-image')).toBeVisible({ timeout: 30_000 });
  await expect(dialog.locator('.calibration-master-provenance')).toContainText(/Session 2/);
  await dialog.getByRole('button', { name: 'Close calibration master inspector' }).click();
});

test('builds a real three-frame Seiza stack and exposes its frame decisions', async ({
  page,
}) => {
  // Two real builds (each bounded at 210 s below), three artifact searches
  // and several reloads: 240 s left a slow runner timing out in the last
  // steps although every one of them passed.
  test.setTimeout(480_000);
  await page.setViewportSize({ width: 1440, height: 1600 });

  // This spec is about the stack queue. Keep the ordinary image-preview queue
  // out of the way so the large FITS fixture is not decoded twice in parallel.
  await page.route('**/images/*/preview?*', (route) => route.abort());
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=1`);

  const panel = page.locator('.stack-preview-panel');
  await expect(panel).toBeVisible({ timeout: 15_000 });
  await expect(panel).toContainText('integrate the 3 images');
  // With no size chosen, a card fills a 1440px window's row.
  const gridColumns = await panel.locator('.stack-preview-grid').evaluate(
    (grid) => getComputedStyle(grid).gridTemplateColumns.split(' ').filter(Boolean).length
  );
  expect(gridColumns).toBe(1);
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
  await expect(panel.getByRole('heading', { name: /Stacks/ })).toBeVisible();
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

  // The build measured its signal-to-noise against depth on the way past, so
  // the card carries the curve, the ideal it is read against, and a verdict.
  const curve = panel.locator('.stack-snr');
  await expect(curve).toBeVisible();
  await expect(curve.locator('.stack-snr-order')).toHaveText('Reference-first capture');
  await expect(curve.locator('.stack-snr-chart .stack-snr-measured')).toBeVisible();
  await expect(curve.locator('.stack-snr-chart .stack-snr-ideal')).toBeVisible();
  await expect(curve.locator('.stack-snr-verdict')).toHaveCount(1);
  await expect(curve).toContainText('noise exponent');

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
  // The curve's summary travels with the pixels.
  expect(fitsHeaderText).toContain('SNRORDER');
  expect(fitsHeaderText).toContain('SNRSLOPE');
  expect(fitsHeaderText).toContain('SNRVERDT');
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
  const builtGroup = (await jobResponse.json()).data.groups[0];
  const orientation = builtGroup.sky_orientation;
  // The curve is published beside the FITS, so it outlives the job list.
  expect(builtGroup.snr.order).toBe('capture');
  expect(builtGroup.snr.points.length).toBeGreaterThanOrEqual(3);
  const snrHref = builtGroup.snr_url as string;
  const snrResponse = await page.request.get(snrHref);
  expect(snrResponse.status()).toBe(200);
  const sidecar = await snrResponse.json();
  expect(sidecar.points.map((point: { frames: number }) => point.frames)).toEqual(
    builtGroup.snr.points.map((point: { frames: number }) => point.frames)
  );
  // Noise has to fall as frames are added, and the reading has to follow it.
  const noise = sidecar.points.map((point: { noise: number }) => point.noise);
  expect(noise[noise.length - 1]).toBeLessThan(noise[0]);
  expect(sidecar.analysis.noise_exponent).toBeLessThan(0);
  expect(['improving', 'diminishing', 'plateau', 'degrading']).toContain(
    sidecar.analysis.verdict
  );
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
  const persistedProcessingSrc = await preview.getAttribute('src');
  await page.reload();
  await expect(preview).toHaveAttribute('src', persistedProcessingSrc!);
  await stretchControls.locator('summary').click();
  await expect(stretchControls.getByRole('checkbox', { name: 'Deconvolution' })).toBeChecked();
  await expect(stretchControls.getByRole('spinbutton', { name: 'Alpha M44 B Target median' }))
    .toHaveValue('0.3');
  const selectionUrl = `/api/db/${encodeURIComponent(dbId)}/stack-previews/${jobId}/0/stretch`;
  const staleRevert = await page.request.delete(`${selectionUrl}?v=old-revision`);
  expect(staleRevert.status()).toBe(409);
  expect((await (await page.request.get(selectionUrl)).json()).data.request.deconvolution)
    .not.toBeNull();
  await stretchControls.getByRole('button', { name: 'Revert processing' }).click();
  await expect(preview).toHaveAttribute('src', defaultPreviewSrc!);
  await expect(stretchControls).toContainText('Deconvolution off');
  await page.reload();
  await expect(preview).toHaveAttribute('src', defaultPreviewSrc!);
  expect((await (await page.request.get(selectionUrl)).json()).data).toBeNull();

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

  // A selection sent from Images is another input: the result built from
  // all three frames is out of date for it, and still shown.
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  const gridCards = page.locator('.image-card');
  await expect(gridCards).toHaveCount(3, { timeout: 15_000 });
  await gridCards.nth(0).click();
  await gridCards.nth(1).click({ modifiers: [process.platform === 'darwin' ? 'Meta' : 'Control'] });
  await page.getByRole('button', { name: 'Stack selected' }).click();
  await expect(page.locator('.stacks-selection')).toContainText('2 images selected in Images');
  await expect(page.locator('.stack-preview-outdated')).toContainText('input images changed');
  await expect(page.getByRole('img', { name: /stack preview/i })).toBeVisible();
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=1`);
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
  await rebuildChannel(page, page.locator('.stack-preview-panel'));
  const rebuiltSrc = await page.locator('.stack-preview-panel')
    .getByRole('img', { name: /stack preview/i })
    .getAttribute('src');
  expect(rebuiltSrc).not.toBe(cachedSrc);

  // The chart folds away from its own heading, and the choice is remembered:
  // someone who folds it should not fold it again every session, or per card.
  const curveSection = page.locator('.stack-preview-panel .stack-snr');
  const curveHeading = curveSection.getByText('Signal-to-noise vs depth');
  await expect(curveSection).toHaveAttribute('open', '');
  await expect(curveSection.locator('.stack-snr-chart')).toBeVisible();

  await curveHeading.click();
  await expect(curveSection).not.toHaveAttribute('open', '');
  await expect(curveSection.locator('.stack-snr-chart')).toBeHidden();
  // Folded is not hidden: the heading and the verdict stay on the card, so
  // the answer is still one line rather than gone.
  await expect(curveHeading).toBeVisible();
  await expect(curveSection.locator('.stack-snr-verdict')).toBeVisible();

  // The native details toggle dispatches after its open attribute changes.
  // Let the preference handler finish before navigating away.
  await expect.poll(() => page.evaluate(() =>
    localStorage.getItem('psf-guard.stack-snr-curve')
  )).toBe('off');
  await page.reload();
  await expect(page.locator('.stack-preview-card')).toHaveCount(1);
  await expect(page.locator('.stack-preview-panel .stack-snr')).not.toHaveAttribute('open', '');

  // Folding changes nothing that was measured or published.
  const stillPublished = await page.request.get(snrHref!);
  expect(stillPublished.status()).toBe(200);

  await page.locator('.stack-preview-panel .stack-snr')
    .getByText('Signal-to-noise vs depth')
    .click();
  await expect(page.locator('.stack-preview-panel .stack-snr')).toHaveAttribute('open', '');
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

  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=1`);

  // The panel re-attaches to a build it did not start.
  const progress = page.locator('.stack-preview-progress');
  await expect(progress).toHaveAttribute('data-stack-state', 'running', { timeout: 15_000 });
  await expect(progress).toContainText('1/3 frames');

  // The header keeps reporting it from every view.
  const activity = page.getByRole('button', { name: /Background jobs/ });
  const queue = page.getByRole('region', { name: 'Background jobs' });
  await activity.click();
  await expect(queue).toContainText('Stacking');
  await expect(queue).toContainText('Alpha M44 · B');
  await expect(queue).toContainText('1/3 frames');
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Sequence' }).click();
  await expect(activity).toBeVisible();
  await page.getByRole('button', { name: 'Library' }).click();
  await activity.hover();
  await expect(queue).toContainText('Stacking');
});

test('re-applies processing to a rebuilt stack, showing the previous result meanwhile', async ({
  page,
}, testInfo) => {
  // Two real builds of the three-frame fixture and two real processing runs.
  test.setTimeout(480_000);
  await page.setViewportSize({ width: 1440, height: 1600 });
  await page.route('**/images/*/preview?*', (route) => route.abort());
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=1`);
  const panel = page.locator('.stack-preview-panel');
  await panel.getByRole('button', { name: 'Build channel', exact: true }).click();
  const results = panel.locator('.stack-preview-results');
  await expect(results).toHaveAttribute('data-job-state', 'completed', { timeout: 210_000 });

  // A person processes the stack.
  const controls = panel.locator('.stack-preview-card .stack-stretch-controls');
  await controls.locator('summary').click();
  await controls.getByRole('checkbox', { name: 'Deconvolution' }).check();
  await controls.getByRole('spinbutton', { name: 'Alpha M44 B Target median' }).fill('0.3');
  await controls.getByRole('button', { name: 'Apply processing' }).click();
  await expect(controls.locator('summary')).toContainText('3.1px deconv · Auto MTF applied', {
    timeout: 60_000,
  });
  const preview = panel.getByRole('img', { name: /stack preview/i });
  const processedSrc = await preview.getAttribute('src');
  expect(processedSrc).toMatch(/\/stack-previews\/stretch\/[a-f0-9]{64}\/preview$/);

  // Once the card has shown the carried result, hold its next looks until
  // this test has seen it: a fast re-apply could otherwise land first.
  let release!: () => void;
  const released = new Promise<void>((resolve) => { release = resolve; });
  let carriedSeen = false;
  await page.route(/\/stack-previews\/[0-9a-f]{64}\/0\/stretch\?/, async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    const response = await route.fetch();
    const body = await response.json();
    if (body.data?.reprocessing) {
      carriedSeen = true;
    } else if (carriedSeen) {
      await released;
    }
    return route.fulfill({ response, json: body });
  });

  // The stack is rebuilt: a new revision, with no processing of its own yet.
  await rebuildChannel(page, panel);
  const carried = panel.locator('.stack-preview-carried');
  await expect(carried).toContainText('From the previous stack · reprocessing', { timeout: 30_000 });
  await expect(preview).toHaveAttribute('src', processedSrc!);
  await expect(controls.locator('summary')).toContainText('applied to the previous stack');
  await panel.locator('.stack-preview-card').screenshot({
    path: testInfo.outputPath('reapply-carried.png'),
  });

  // The server re-applies it on its own and the card swaps in the result.
  release();
  await expect(carried).toHaveCount(0, { timeout: 60_000 });
  await expect(preview).toHaveAttribute('src', /\/stack-previews\/stretch\/[a-f0-9]{64}\/preview$/);
  expect(await preview.getAttribute('src')).not.toBe(processedSrc);
  await expect(controls.locator('summary')).toContainText('3.1px deconv · Auto MTF applied');
  await expect(controls.locator('summary')).not.toContainText('previous stack');

  // A reload restores the new stack's own processing.
  await page.reload();
  await expect(page.locator('.stack-preview-card .stack-stretch-controls summary'))
    .toContainText('3.1px deconv · Auto MTF applied');
  await expect(page.locator('.stack-preview-carried')).toHaveCount(0);
});

test('says what a processing run waits for, then its progress, and lists it in the header', async ({
  page,
}, testInfo) => {
  // No rc-astro is installed here, so the run's waiting and running answers
  // are served from stubs; the last poll goes to the server, which renders
  // the stretch for real. The server side of the queue is covered by the
  // `StackPreviewManager` processing-run unit tests.
  test.setTimeout(90_000);
  seedSyntheticColorStacks(dbId, 2, true);
  const redJob = (2).toString(16).padStart(64, '0');
  const runId = 'c'.repeat(64);
  let phase: 'idle' | 'queued' | 'running' | 'done' = 'idle';
  const build = {
    kind: 'mono', job_id: 'a'.repeat(64), database_id: dbId, project_id: 1, target_id: 1,
    state: 'running', label: 'Alpha M44 · B', detail: 'Registering frames',
    processed_units: 2, total_units: 3, created_unix_seconds: 1_760_000_000,
    progress_label: '2/3 frames',
  };
  const run = (running: boolean) => ({
    kind: 'processing', job_id: runId, database_id: dbId, project_id: 2, target_id: 2,
    state: running ? 'running' : 'queued', label: 'Beta Field · R',
    detail: running ? 'BlurXTerminator' : 'Waiting for the stack build of Alpha M44 · B to finish',
    processed_units: running ? 420 : 0, total_units: running ? 1000 : 0,
    created_unix_seconds: 1_760_000_100, queue_position: running ? null : 0,
    progress_label: running ? 'BlurXTerminator · 42%' : undefined,
    processing: {
      operation: 'BlurXTerminator', channel: 'R', stoppable: true,
      ...(running
        ? { stage: 'BlurXTerminator', fraction: 0.42, started_unix_seconds: 1_760_000_200 }
        : { waiting_for: 'the stack build of Alpha M44 · B' }),
    },
  });
  await page.route('**/api/stack-activity', (route) => route.fulfill({
    json: {
      success: true,
      data: {
        schema_version: 1,
        active: phase === 'queued' ? [build, run(false)] : phase === 'running' ? [run(true)] : [],
      },
      error: null,
    },
  }));
  const posts: Array<string | null> = [];
  await page.route(`**/stack-previews/${redJob}/0/stretch*`, async (route) => {
    if (route.request().method() !== 'POST') return route.continue();
    const url = new URL(route.request().url());
    posts.push(url.searchParams.get('poll'));
    if (phase === 'queued') {
      return route.fulfill({ status: 202, json: { success: true, error: null, data: {
        pending: true, queued: true, queue_position: 0,
        stage: 'Waiting for the stack build of Alpha M44 · B to finish',
        waiting_for: 'the stack build of Alpha M44 · B',
      } } });
    }
    if (phase === 'running') {
      return route.fulfill({ status: 202, json: { success: true, error: null, data: {
        pending: true, queued: false, stage: 'RC-Astro BlurXTerminator', fraction: 0.42,
      } } });
    }
    // The run is done: the server renders and answers with the result.
    url.searchParams.delete('poll');
    return route.continue({ url: url.toString() });
  });

  await page.setViewportSize({ width: 1440, height: 1100 });
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=2&target=2`);
  const card = page.locator('.stack-preview-card').filter({
    has: page.locator('.stack-preview-channel', { hasText: /^R$/ }),
  });
  await expect(card).toBeVisible({ timeout: 15_000 });
  const controls = card.locator('.stack-stretch-controls');
  await controls.locator('summary').click();
  const apply = controls.locator('.stack-stretch-actions button').first();
  await expect(apply).toHaveText('Apply processing');
  await expect(apply).toBeEnabled();

  phase = 'queued';
  await apply.click();
  await expect(apply).toHaveText('Waiting for the stack build of Alpha M44 · B to finish');
  await expect(apply).toBeDisabled();
  await expect(apply).toHaveAttribute('aria-busy', 'true');
  await controls.locator('.stack-stretch-actions').scrollIntoViewIfNeeded();
  await card.screenshot({ path: testInfo.outputPath('processing-queued.png') });

  // The header lists the run behind the build it waits for.
  const activity = page.getByRole('button', { name: /Background jobs/ });
  const queue = page.getByRole('region', { name: 'Background jobs' });
  // The chip counts the running build and the queued run (#869).
  await expect(activity).toHaveAccessibleName(/1 running, 1 queued/);
  await activity.click();
  const rows = queue.getByRole('listitem');
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText('Stacking');
  await expect(rows.nth(0)).toContainText('Alpha M44 · B');
  await expect(rows.nth(1)).toContainText('BlurXTerminator');
  await expect(rows.nth(1)).toContainText('Beta Field · R');
  await expect(rows.nth(1)).toContainText('queued · 1');
  await expect(rows.nth(1)).toContainText('Waiting for the stack build of Alpha M44 · B');
  await expect(queue.getByRole('link', { name: 'Go to BlurXTerminator: Beta Field · R' }))
    .toHaveAttribute('href', `#/stacks?db=${encodeURIComponent(dbId)}&project=2&target=2`);
  await queue.screenshot({ path: testInfo.outputPath('processing-header-queue.png') });
  await page.keyboard.press('Escape');

  phase = 'running';
  await expect(apply).toHaveText('Applying… 42% · BlurXTerminator', { timeout: 15_000 });
  await expect(activity).toContainText('42%');
  await card.screenshot({ path: testInfo.outputPath('processing-running.png') });

  phase = 'done';
  await expect(apply).toHaveText('Apply processing', { timeout: 15_000 });
  await expect(apply).toBeEnabled();
  await expect(controls.locator('summary')).toContainText('applied');
  await expect(card.locator('.stack-preview-image img')).toHaveAttribute(
    'src', /\/stack-previews\/stretch\/[a-f0-9]{64}\/preview$/
  );
  expect(posts[0]).toBeNull();
  expect(posts.slice(1).every((poll) => poll === 'true')).toBe(true);
});

test('composes cached channel stacks into RGB, LRGB, and selectable narrowband previews', async ({
  page,
}) => {
  test.setTimeout(180_000);
  seedSyntheticColorStacks(dbId, 2);
  await page.setViewportSize({ width: 1440, height: 1800 });
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=2`);

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
  await expect(rgbCard.locator('.stack-preview-progress')).toContainText('24/24 steps');
  await expect(rgbCard.getByRole('img', { name: /RGB color stack preview/i })).toBeVisible();
  // The synthetic stacks have neither a solved reference frame nor Gaia
  // colors to calibrate against; the card says so and keeps the preview.
  await expect(rgbCard.locator('.stack-color-calibration')).toContainText('Not color calibrated');
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
  await expect(rgbProcessing.getByRole('switch', { name: 'Color calibration' })).toBeChecked();
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
  await expect(rgbCard.locator('.stack-preview-progress')).toContainText('26/26 steps');
  const phaseDetails = rgbCard.locator('.stack-color-phase-details');
  await phaseDetails.locator('summary').click();
  await expect(phaseDetails.locator('li')).toHaveCount(14);
  await expect(phaseDetails.locator('li[data-phase="calibrating_color"]'))
    .toHaveAttribute('data-phase-state', 'skipped');
  await expect(phaseDetails.locator('li[data-phase="rc_astro_inputs"]'))
    .toHaveAttribute('data-phase-state', 'skipped');
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
  await expect(lrgbCard.locator('.stack-preview-progress')).toContainText('30/30 steps');
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
  await expect(narrowbandCard.locator('.stack-preview-progress')).toContainText('18/18 steps');
  await expect(narrowbandCard.locator('.stack-color-calibration')).toHaveCount(0);
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
  await openSettings(page);
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

  // Size sets how wide the stack and color cards are; a size wider than the
  // page fills it and goes no further.
  const cardWidth = async (size: number) => {
    await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=2&cardsize=${size}`);
    await expect(stackPanel).toBeVisible({ timeout: 15_000 });
    const card = stackPanel.locator('.stack-color-card').first();
    await expect(card).toBeVisible();
    const grid = stackPanel.locator('.stack-color-grid').first();
    return {
      card: (await card.boundingBox())!.width,
      grid: await grid.evaluate((element) => element.clientWidth),
    };
  };
  const narrow = await cardWidth(400);
  expect(Math.abs(narrow.card - 400)).toBeLessThan(2);
  const wide = await cardWidth(1600);
  expect(wide.card).toBeLessThanOrEqual(wide.grid + 1);
  expect(wide.card).toBeGreaterThan(narrow.card);

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

test('a phone folds the build options behind Options and gives the color cards the width', async ({ page }) => {
  seedSyntheticColorStacks(dbId, 2);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=2`);
  const panel = page.locator('.stack-preview-panel');
  const options = panel.getByRole('button', { name: /^Options/ });
  await expect(options).toHaveAttribute('aria-expanded', 'false');
  const calibration = panel.locator('.stack-preview-calibration-mode select');
  await expect(calibration).toBeHidden();
  await expect(panel.getByRole('checkbox', { name: 'Accepted only' })).toBeHidden();
  await options.click();
  await expect(calibration).toBeVisible();
  // Open, the options stay inside the panel.
  const optionsBox = await panel.locator('.stack-preview-options').boundingBox();
  const panelBox = await panel.boundingBox();
  expect(optionsBox!.x + optionsBox!.width).toBeLessThanOrEqual(panelBox!.x + panelBox!.width);
  await calibration.selectOption('off');
  // A choice off its default is named on the button, folded or not.
  await expect(options).toContainText('No calibration');
  await expect(options).toHaveClass(/is-set/);
  await options.click();
  await expect(calibration).toBeHidden();
  // The card size slider has nothing to do on a phone's one column.
  await expect(page.getByLabel('Card size')).toBeHidden();
  const card = await page.locator('.stack-color-card').first().boundingBox();
  expect(card!.width).toBeGreaterThan(340);
});

test('color calibrates a one-shot-color stack by default, says why it could not, and flags a stale calibration', async ({ page }) => {
  seedSyntheticOscStack(dbId, 2);
  await page.setViewportSize({ width: 1440, height: 1200 });
  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=2`);
  const card = page.locator('.stack-preview-card').filter({ hasText: 'Beta Field' }).first();
  // The build's calibration rested on a Gaia catalog that is gone. The
  // seeded stack records no input images, which the card reports first, so
  // read the server's verdict on the preview itself.
  const latest = await page.request.get(`/api/db/${encodeURIComponent(dbId)}/projects/2/stack-previews/latest`);
  const seeded = (await latest.json()).data.groups.find(
    (entry: { job_id: string }) => entry.job_id === 'c'.repeat(64)
  );
  expect(seeded.group.color_calibration_outdated).toBe(
    'Out of date — Gaia colors were removed; click Build current set to render the preview again'
  );
  const controls = card.locator('.stack-stretch-controls');
  await controls.locator('summary').click();
  const toggle = controls.getByRole('switch', { name: 'Color calibration' });
  await expect(toggle).toBeChecked();
  await controls.getByRole('button', { name: 'Apply processing' }).click();
  // The fixture's reference frame has no fresh solve and the test server no
  // Gaia catalog: the card says calibration did not run, and the view is
  // still processed.
  await expect(card.locator('.stack-color-calibration')).toContainText('Not color calibrated', {
    timeout: 60_000,
  });
  await expect(controls.locator('summary')).toContainText('applied');
  // Processing made now rests on what is installed now.
  const processing = await page.request.get(
    `/api/db/${encodeURIComponent(dbId)}/stack-previews/${'c'.repeat(64)}/0/stretch`
  );
  const applied = (await processing.json()).data;
  expect(applied.color_calibration?.status).toBe('skipped');
  expect(applied.color_calibration_outdated ?? null).toBeNull();
});
