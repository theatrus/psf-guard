import { expect, test, type Locator } from '@playwright/test';
import Database from 'better-sqlite3';
import * as fs from 'fs';
import * as path from 'path';
import type { Image } from '../src/api/types';
import { fixtureDbPath, fixtureImageDir, resetDatabases, tmpBase, waitForCacheReady } from './helpers';

function syntheticFits(): Buffer {
  const cards = ['SIMPLE  =                    T', 'BITPIX  =                  -32',
    'NAXIS   =                    2', 'NAXIS1  =                   32',
    'NAXIS2  =                   24', 'END'];
  const header = Buffer.from(cards.map((card) => card.padEnd(80)).join('').padEnd(2880), 'ascii');
  const pixels = Buffer.alloc(5760);
  for (let index = 0; index < 32 * 24; index++) pixels.writeFloatBE(1 + index / 768, index * 4);
  return Buffer.concat([header, pixels]);
}

async function positionBelowToolbar(locator: Locator) {
  await locator.evaluate((element) => {
    const scroller = element.closest('.app-main');
    if (!scroller) return;
    const toolbar = scroller.querySelector('.image-controls');
    const stickyHeight = toolbar && getComputedStyle(toolbar).position === 'sticky'
      ? toolbar.getBoundingClientRect().height : 0;
    scroller.scrollTop += element.getBoundingClientRect().top
      - scroller.getBoundingClientRect().top - stickyHeight - 12;
  });
}

function seedExposureStacks(databaseId: string, images: Image[]) {
  const root = path.join(tmpBase(), 'cache', databaseId, 'stack-previews');
  const partitions = new Map<string, Image[]>();
  for (const image of images) {
    const key = JSON.stringify([image.target_id, image.filter_name, image.exposure_group?.key]);
    partitions.set(key, [...(partitions.get(key) ?? []), image]);
  }
  const groups = [...partitions.values()].map((frames, index) => {
    const first = frames[0];
    const jobId = (index + 100).toString(16).padStart(64, '0');
    fs.mkdirSync(path.join(root, jobId), { recursive: true });
    fs.writeFileSync(path.join(root, jobId, 'group-0.fits'), syntheticFits());
    const remembered = {
      job_id: jobId, artifact_revision: `exposure-fixture-${index}`, accepted_only: false,
      created_unix_seconds: 1_760_000_000 + index, cache_version: 15,
      group: {
        index: 0, target_id: first.target_id, target_name: first.target_name,
        filter_name: first.filter_name, exposure_group: first.exposure_group,
        state: 'ready', phase: 'ready', total_candidates: frames.length, eligible_frames: frames.length,
        quality_excluded: 0, missing_files: 0, processed_frames: frames.length,
        accepted_frames: frames.length, rejected_frames: 0, output_channels: 1,
        sky_orientation: { convention: 'source_frame', version: 1, source: 'sky_anchor',
          output_width: 32, output_height: 24,
          source_to_output: { matrix: [[1, 0], [0, 1]], translation_x: 0, translation_y: 0 } },
        reference_image_id: first.id, total_exposure_seconds: frames.reduce((sum, image) => sum + Number(image.metadata.ExposureDuration), 0),
        preview_url: null, fits_url: null, error: null,
        input_images: frames.map((image) => ({ image_id: image.id, grading_status: image.grading_status })), frames: [],
      },
    };
    fs.writeFileSync(path.join(root, jobId, 'manifest.json'), JSON.stringify({
      schema_version: 2, database_id: databaseId, project_id: 2,
      job_id: jobId, artifact_revision: remembered.artifact_revision,
      state: 'completed', accepted_only: remembered.accepted_only,
      created_unix_seconds: remembered.created_unix_seconds,
      cache_version: remembered.cache_version, stacking_version: '0.14.0',
      order: 'capture', groups: [remembered.group], error: null,
    }));
    return remembered;
  });
  fs.mkdirSync(root, { recursive: true });
  fs.writeFileSync(path.join(root, 'latest-project-2.json'), JSON.stringify({
    schema_version: 1, database_id: databaseId, project_id: 2, updated_unix_seconds: 1_760_000_100, groups,
  }));
  return groups;
}

test('persists project exposure grouping and separates mono and color choices', async ({ page, request }, testInfo) => {
  test.setTimeout(120_000);
  await resetDatabases(request);
  const databasePath = path.join(tmpBase(), `exposure-groups-${testInfo.repeatEachIndex}-${testInfo.retry}.sqlite`);
  fs.copyFileSync(fixtureDbPath(), databasePath);
  const database = new Database(databasePath);
  try {
    const existing = database.prepare('SELECT * FROM acquiredimage WHERE projectId=2 LIMIT 1').get() as {
      acquireddate: number; metadata: string;
    };
    database.exec('DELETE FROM acquiredimage');
    const insert = database.prepare('INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata) VALUES(?,2,2,?,?,1,?)');
    let id = 100;
    for (const filter of ['R', 'G', 'B']) for (const exposure of [30, 300]) for (let repeat = 0; repeat < 2; repeat++) {
      insert.run(id++, existing.acquireddate + id, filter, JSON.stringify({
        ...JSON.parse(existing.metadata), ExposureDuration: exposure, ExposureTime: exposure, Filter: filter,
      }));
    }
  } finally {
    database.close();
  }
  const registered = await request.post('/api/databases', { data: {
    name: 'Exposure groups fixture', db_path: databasePath, image_dirs: [fixtureImageDir()],
    slug: `exposure-groups-${testInfo.repeatEachIndex}-${testInfo.retry}`,
  } });
  expect(registered.ok()).toBe(true);
  const dbId = (await registered.json()).data.id as string;
  await waitForCacheReady(request, dbId);
  const settingsPath = `/api/db/${encodeURIComponent(dbId)}/projects/2/processing-settings`;
  const initial = await request.get(settingsPath);
  expect((await initial.json()).data.split_exposure_groups).toBe(false);
  await page.setViewportSize({ width: 1440, height: 1050 });
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=2&groupingMode=filter`);
  const toggle = page.getByRole('checkbox', { name: 'Separate exposure groups' });
  await expect(toggle).toBeEnabled();
  await expect(toggle).not.toBeChecked();
  await expect(page.locator('.stack-preview-card')).toHaveCount(3);
  await toggle.click();
  await expect(toggle).toBeChecked();
  await expect(page.locator('.stack-preview-card')).toHaveCount(6);
  const images = (await (await request.get(`/api/db/${dbId}/images?project_id=2&limit=1000`)).json()).data as Image[];
  expect(images).toHaveLength(12);
  expect(images.every((image) => image.exposure_group)).toBe(true);
  const savedStacks = seedExposureStacks(dbId, images);
  for (const stack of savedStacks) {
    const processing = await request.get(`/api/db/${dbId}/stack-previews/${stack.job_id}/0/stretch?v=${stack.artifact_revision}`);
    expect(processing.status()).toBe(200);
    expect((await processing.json()).data).toBeNull();
  }

  // Synthetic preview pixels isolate the UI contract from expensive image processing.
  const preview = await page.evaluate(() => {
    const canvas = document.createElement('canvas');
    canvas.width = 512; canvas.height = 320;
    const context = canvas.getContext('2d')!;
    const bitmap = context.createImageData(canvas.width, canvas.height);
    for (let y = 0; y < canvas.height; y++) for (let x = 0; x < canvas.width; x++) {
      const radius = ((x - 240) / 130) ** 2 + ((y - 165) / 65) ** 2;
      const brightness = 15 + 90 * Math.exp(-radius) + ((x * 37 + y * 13) % 11);
      const index = (y * canvas.width + x) * 4;
      bitmap.data.set([brightness, brightness, brightness, 255], index);
    }
    context.putImageData(bitmap, 0, 0);
    context.fillStyle = '#ddd';
    for (let star = 0; star < 60; star++) {
      context.beginPath(); context.arc((star * 83 + 13) % 512, (star * 47 + 11) % 320, 0.8 + star % 2, 0, Math.PI * 2); context.fill();
    }
    return canvas.toDataURL('image/png').split(',')[1];
  });
  await page.route('**/stack-previews/*/*/preview?*', (route) => route.fulfill({
    status: 200, contentType: 'image/png', body: Buffer.from(preview, 'base64'),
  }));
  await page.reload();
  await expect(toggle).toBeChecked();
  await expect(page.locator('.stack-preview-card')).toHaveCount(6);
  await expect(page.locator('.stack-preview-card img').first()).toBeVisible();
  await expect(page.getByText('Loading saved processing...', { exact: true })).toHaveCount(0);
  await expect(page.getByText(/Saved processing could not be loaded:/)).toHaveCount(0);
  const rgb = page.locator('.stack-color-card[data-color-kind="rgb"][data-exposure-set]');
  await expect(rgb).toHaveCount(2);
  const shortRgb = rgb.filter({ has: page.getByRole('button', { name: 'Build RGB 30 s color preview', exact: true }) });
  const longRgb = rgb.filter({ has: page.getByRole('button', { name: 'Build RGB 300 s color preview', exact: true }) });
  await expect(shortRgb.getByRole('button', { name: 'Build RGB 30 s color preview', exact: true })).toBeEnabled();
  await expect(longRgb.getByRole('button', { name: 'Build RGB 300 s color preview', exact: true })).toBeEnabled();
  await expect(rgb.locator('.stack-color-source-selectors')).toHaveCount(0);
  await expect(page.getByRole('combobox', { name: 'Beta Field rgb R source stack' })).toHaveCount(0);
  const custom = page.locator('details[data-color-kind="rgb"][data-target-id="2"]');
  const customSummary = custom.getByText('Custom combination', { exact: true });
  await customSummary.click();
  const customRgb = custom.locator('.stack-color-card[data-color-kind="rgb"]');
  await expect(customRgb.getByRole('button', { name: 'Build RGB custom color preview', exact: true })).toBeDisabled();
  for (const role of ['R', 'G', 'B']) {
    const selector = customRgb.getByRole('combobox', { name: `Beta Field rgb ${role} source stack` });
    await expect(selector.locator('option')).toHaveCount(3);
    await selector.selectOption({ label: `${role} (${role === 'R' ? 30 : 300} s) · 2 frames` });
  }
  await expect(customRgb.getByRole('button', { name: 'Build RGB custom color preview', exact: true })).toBeEnabled();
  await customSummary.click();
  await page.locator('.app-main').evaluate((element) => { element.scrollTop = 0; });
  expect(await page.locator('.project-exposure-grouping label').evaluate((element) => element.getBoundingClientRect().height)).toBeLessThan(28);
  await page.screenshot({ path: testInfo.outputPath('exposure-groups-desktop.png'), fullPage: false });
  await positionBelowToolbar(page.locator('.stack-preview-card').first());
  await page.screenshot({ path: testInfo.outputPath('exposure-stack-cards-desktop.png'), fullPage: false });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.locator('.app-main').evaluate((element) => { element.scrollTop = 0; });
  await expect(page.locator('.grid-stats')).toContainText('6 groups');
  expect(await page.locator('.grid-stats').evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath('exposure-groups-mobile.png'), fullPage: false });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  expect(await page.locator('.app-main').evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await positionBelowToolbar(page.locator('.stack-preview-card').first());
  await page.screenshot({ path: testInfo.outputPath('exposure-stack-cards-mobile.png'), fullPage: false });
  for (const header of await page.locator('.stack-preview-card > header').all()) {
    expect(await header.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  }
  for (const [card, duration] of [[shortRgb, 30], [longRgb, 300]] as const) {
    await positionBelowToolbar(card);
    await card.screenshot({ path: testInfo.outputPath(`exposure-color-${duration}-mobile.png`) });
    expect(await card.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  }
  await customSummary.click();
  await positionBelowToolbar(customRgb);
  await customRgb.screenshot({ path: testInfo.outputPath('exposure-color-sources-mobile.png') });
  expect(await customRgb.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);

  // Leave the short RGB set complete while removing only the remembered long B stack.
  const indexPath = path.join(tmpBase(), 'cache', dbId, 'stack-previews', 'latest-project-2.json');
  const remembered = JSON.parse(fs.readFileSync(indexPath, 'utf8'));
  fs.writeFileSync(indexPath, JSON.stringify({ ...remembered, groups: savedStacks.filter((stack) =>
    !(stack.group.filter_name === 'B' && stack.group.exposure_group?.min_seconds === 300)) }));
  await page.reload();
  await expect(rgb).toHaveCount(2);
  await expect(shortRgb.getByRole('button', { name: 'Build RGB 30 s color preview', exact: true })).toBeEnabled();
  await expect(longRgb.getByRole('button', { name: 'Build RGB 300 s color preview', exact: true })).toBeDisabled();
  await expect(longRgb).toContainText('Missing B');
  await toggle.click();
  await expect(toggle).not.toBeChecked();
  await expect(page.locator('.stack-preview-card')).toHaveCount(3);
  expect((await (await request.get(settingsPath)).json()).data.split_exposure_groups).toBe(false);
  const unsplit = (await (await request.get(`/api/db/${dbId}/images?project_id=2&limit=1000`)).json()).data as Image[];
  expect(unsplit.every((image) => !image.exposure_group)).toBe(true);
});
