import { expect, test, type Page, type TestInfo } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { resetDatabases, tmpBase } from './helpers';

let root: string;

const SIDE = 64;

/** A small 16-bit calibration frame. */
function writeFrame(file: string, card: { kind: string; date: string; exposure: number; filter?: string }, seed: number) {
  const cards = [
    'SIMPLE  =                    T',
    'BITPIX  =                   16',
    'NAXIS   =                    2',
    `NAXIS1  = ${String(SIDE).padStart(20)}`,
    `NAXIS2  = ${String(SIDE).padStart(20)}`,
    'BZERO   =                32768',
    `IMAGETYP= '${card.kind}'`,
    `DATE-OBS= '${card.date}'`,
    `EXPTIME = ${card.exposure.toFixed(1).padStart(20)}`,
    ...(card.filter ? [`FILTER  = '${card.filter}'`] : []),
    'GAIN    =                  100',
    'OFFSET  =                   20',
    'CCD-TEMP=                -10.0',
    "TELESCOP= 'Twin Scope'",
    "INSTRUME= 'Twin Camera'",
    'END',
  ];
  const header = Buffer.from(cards.map((line) => line.padEnd(80)).join('').padEnd(2880), 'ascii');
  const pixels = Buffer.alloc(Math.ceil((SIDE * SIDE * 2) / 2880) * 2880);
  const base = card.kind === 'FLAT' ? 24000 : 500;
  for (let index = 0; index < SIDE * SIDE; index++) {
    const value = Math.sin(index * 12.9898 + seed * 78.233) * 43758.5453;
    pixels.writeInt16BE(base + Math.round((value - Math.floor(value)) * 40) - 32768, index * 2);
  }
  fs.writeFileSync(file, Buffer.concat([header, pixels]));
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  await page.screenshot({ path: testInfo.outputPath(`${name}.png`), fullPage: true });
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  root = path.join(tmpBase(), 'calibration-duplicates');
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

test('finds exposures recorded twice, previews the repair and applies it', async ({ page, request }, testInfo) => {
  test.setTimeout(120_000);
  // A drop folder and a library, each holding its own frames under the
  // same names, imported as an older build would: one row per file.
  const drop = path.join(root, 'drop');
  const library = path.join(root, 'library');
  fs.mkdirSync(drop, { recursive: true });
  fs.mkdirSync(library, { recursive: true });
  const flats = [0, 1, 2].map((index) => `2026-09-11_04-0${index}-00_Ha_flat_000${index}.fits`);
  const darks = [0, 1].map((index) => `2026-09-11_06-0${index}-00_dark_000${index}.fits`);
  flats.forEach((name, index) => {
    const card = { kind: 'FLAT', date: `2026-09-11T04:0${index}:00`, exposure: 2, filter: 'Ha' };
    writeFrame(path.join(drop, name), card, index);
    writeFrame(path.join(library, name), card, 100 + index);
  });
  darks.forEach((name, index) => {
    const card = { kind: 'DARK', date: `2026-09-11T06:0${index}:00`, exposure: 300 };
    writeFrame(path.join(drop, name), card, 10 + index);
    writeFrame(path.join(library, name), card, 110 + index);
  });

  const created = await request.post('/api/databases/create', {
    data: { name: 'Calibration twins', image_dirs: [root], slug: 'calibration-twins-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(async () => {
      const progress = (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress;
      if (progress.running || !progress.outcome) return progress.error ?? 'running';
      return `${progress.outcome.calibration.imported} recorded`;
    }, { timeout: 60_000 })
    .toBe('10 recorded');

  // Now each drop-folder file becomes a byte copy of its library file; the
  // filer then deletes the drop-folder flats. Three rows lose their file to
  // a live twin; two darks are on disk twice.
  for (const name of [...flats, ...darks]) {
    fs.copyFileSync(path.join(library, name), path.join(drop, name));
  }
  for (const name of flats) fs.rmSync(path.join(drop, name));

  // A repair is a job: the request answers 202 at once. One that names no
  // previewed plan applies nothing, and leaves the plan it found and what
  // changed in the status.
  const refused = await request.post(`/api/db/${dbId}/calibrations/duplicates/repair`, {
    data: { token: 'not-a-preview' },
  });
  expect(refused.status()).toBe(202);
  const refusedRun: number = (await refused.json()).data.run;
  await expect
    .poll(async () => {
      const status = (await (await request.get(`/api/db/${dbId}/calibrations/duplicates`)).json()).data;
      return status.run === refusedRun ? status.state : 'other';
    })
    .toBe('done');
  const afterRefused = (await (await request.get(`/api/db/${dbId}/calibrations/duplicates`)).json()).data;
  expect(afterRefused.repaired).toBeNull();
  expect(afterRefused.changed.groups_after).toBe(5);
  expect(afterRefused.report.groups).toHaveLength(5);

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/#/calibration?db=${dbId}&calDupes=true`);
  const section = page.getByRole('region', { name: 'Duplicates' });
  await expect(section.getByText('5 exposures recorded more than once')).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole('checkbox', { name: 'Duplicates' })).toBeChecked();
  const flat = section.getByRole('listitem', { name: flats[0] });
  await expect(flat.getByText('Keep')).toBeVisible();
  await expect(flat.getByText('Missing')).toBeVisible();
  await shot(page, testInfo, 'duplicates-1440');

  // The type filter narrows the groups; the URL keeps both.
  await page.getByLabel('Type').selectOption('dark');
  await expect(page).toHaveURL(/calKind=dark/);
  await expect(section.getByText('2 of 5 exposures recorded more than once')).toBeVisible();
  await page.getByLabel('Type').selectOption('');
  await expect(section.getByText('5 exposures recorded more than once')).toBeVisible();

  await page.setViewportSize({ width: 390, height: 844 });
  await shot(page, testInfo, 'duplicates-390');

  await page.setViewportSize({ width: 1440, height: 900 });
  await section.getByRole('button', { name: 'Repair duplicates…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Repair duplicates' });
  const preview = dialog.getByRole('region', { name: 'Repair preview' });
  await expect(preview.getByText('5 exposures recorded more than once')).toBeVisible();
  await expect(preview.getByText(/5 rows merged into the row kept: 3 whose file is gone, 2 second copies on disk/)).toBeVisible();
  await shot(page, testInfo, 'repair-preview-1440');
  await page.setViewportSize({ width: 390, height: 844 });
  await shot(page, testInfo, 'repair-preview-390');

  await dialog.getByRole('button', { name: 'Apply' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Repaired 5 exposures' })).toBeVisible();
  // The repair leaves the plan stale; the check after it runs in the background.
  await expect(section.getByText('No exposure is recorded more than once.')).toBeVisible({ timeout: 30_000 });
  await shot(page, testInfo, 'after-apply-390');
  await page.setViewportSize({ width: 1440, height: 900 });
  await shot(page, testInfo, 'after-apply-1440');

  // One row per exposure, every file still there, and a second run finds
  // nothing to do.
  const frames = (await (await request.get(`/api/db/${dbId}/calibrations/frames?per_page=50`)).json()).data;
  expect(frames.total).toBe(5);
  expect(frames.frames.every((frame: { source_exists: boolean }) => frame.source_exists)).toBe(true);
  for (const name of [...flats, ...darks]) expect(fs.existsSync(path.join(library, name))).toBe(true);
  for (const name of darks) expect(fs.existsSync(path.join(drop, name))).toBe(true);
  await expect
    .poll(async () => {
      const status = (await (await request.get(`/api/db/${dbId}/calibrations/duplicates`)).json()).data;
      return status.report ? status.report.groups.length : 'checking';
    })
    .toBe(0);
});
