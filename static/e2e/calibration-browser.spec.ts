import { expect, test } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { fixtureImageDir, resetDatabases, tmpBase, waitForCacheReady } from './helpers';

let root: string;

/** The first FITS frame among the fixtures. */
function fixtureFrame(directory: string): string {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      const found = fixtureFrame(full);
      if (found) return found;
    } else if (/\.fits?$/i.test(entry.name)) {
      return full;
    }
  }
  return '';
}

const SIDE = 128;

/** A small 16-bit calibration frame from the fixture lights' camera. */
function writeFrame(
  file: string,
  card: { kind: string; date: string; exposure: number; filter?: string },
  pixel: (x: number, y: number) => number,
) {
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
    "INSTRUME= 'Browser Camera'",
    'END',
  ];
  const header = Buffer.from(cards.map((line) => line.padEnd(80)).join('').padEnd(2880), 'ascii');
  const pixels = Buffer.alloc(Math.ceil((SIDE * SIDE * 2) / 2880) * 2880);
  for (let y = 0; y < SIDE; y++) {
    for (let x = 0; x < SIDE; x++) {
      const value = Math.max(0, Math.min(65535, Math.round(pixel(x, y))));
      pixels.writeInt16BE(value - 32768, (y * SIDE + x) * 2);
    }
  }
  fs.writeFileSync(file, Buffer.concat([header, pixels]));
}

/** A little repeatable noise. */
function noise(x: number, y: number, seed: number, spread: number) {
  const value = Math.sin(x * 12.9898 + y * 78.233 + seed * 37.719) * 43758.5453;
  return (value - Math.floor(value) - 0.5) * spread;
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  root = path.join(tmpBase(), 'calibration-browser');
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

test('browses flats by session, flips frames and compares one with its session', async ({ page, request }) => {
  const fixture = fixtureFrame(fixtureImageDir());
  expect(fixture, 'a fixture FITS frame').toBeTruthy();
  fs.mkdirSync(path.join(root, 'LIGHT'), { recursive: true });
  fs.copyFileSync(fixture, path.join(root, 'LIGHT', path.basename(fixture)));
  fs.mkdirSync(path.join(root, 'FLAT'), { recursive: true });
  fs.mkdirSync(path.join(root, 'DARK'), { recursive: true });
  // Four flats with the same dust; the third caught a droplet.
  for (let index = 0; index < 4; index++) {
    writeFrame(
      path.join(root, 'FLAT', `flat-${index}.fits`),
      { kind: 'FLAT', date: `2026-09-11T04:0${index}:00`, exposure: 2 + index * 0.1, filter: 'Ha' },
      (x, y) => {
        let value = (24000 + index * 400) * (1 - 0.0012 * x) + noise(x, y, index, 40);
        if (Math.hypot(x - 32, y - 32) < 6) value *= 0.9;
        if (index === 2 && Math.hypot(x - 88, y - 80) < 8) value *= 0.8;
        return value;
      },
    );
  }
  for (let index = 0; index < 3; index++) {
    writeFrame(
      path.join(root, 'DARK', `dark-${index}.fits`),
      { kind: 'DARK', date: `2026-09-11T06:0${index}:00`, exposure: 300 },
      (x, y) => 503 + noise(x, y, 10 + index, 10),
    );
  }

  const created = await request.post('/api/databases/create', {
    data: { name: 'Calibration browser', image_dirs: [root], slug: 'calibration-browser-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(async () => {
      const progress = (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress;
      if (progress.running || !progress.outcome) return progress.error ?? 'running';
      return `${progress.outcome.calibration.flat} flats, ${progress.outcome.calibration.dark} darks`;
    }, { timeout: 60_000 })
    .toBe('4 flats, 3 darks');
  await waitForCacheReady(request, dbId);
  // Grading follows the import; the panel shows its numbers.
  await expect
    .poll(async () => (await (await request.get(`/api/db/${dbId}/calibrations/quality`)).json()).data.counts.measured, { timeout: 60_000 })
    .toBe(7);

  await page.goto(`/#/calibration?db=${dbId}`);
  const sessions = page.getByRole('navigation', { name: 'Sessions' });
  await expect(sessions.getByRole('button')).toHaveCount(2);

  // Narrow to flats: the URL keeps the filter, and the flat session opens.
  await page.getByLabel('Type').selectOption('flat');
  await expect(page).toHaveURL(/calKind=flat/);
  await expect(sessions.getByRole('button')).toHaveCount(1);
  await sessions.getByRole('button').first().click();
  const strip = page.getByRole('list', { name: 'Frames' });
  await expect(strip.getByRole('button')).toHaveCount(4);

  // A thumbnail is drawn through the shared poller.
  await expect(strip.getByRole('button').first().locator('img')).toBeVisible({ timeout: 30_000 });

  // Open the droplet frame, then flip with the arrow keys.
  await strip.getByRole('button', { name: 'Frame 3: flat-2.fits' }).click();
  const panel = page.getByRole('complementary', { name: 'Frame' });
  await expect(panel.getByRole('heading', { name: 'flat-2.fits' })).toBeVisible();
  await expect(panel.getByRole('row', { name: /Level/ })).toBeVisible();
  await page.keyboard.press('ArrowRight');
  await expect(panel.getByRole('heading', { name: 'flat-3.fits' })).toBeVisible();
  await page.keyboard.press('ArrowLeft');
  await expect(panel.getByRole('heading', { name: 'flat-2.fits' })).toBeVisible();
  const image = page.getByTestId('calibration-frame-image');
  await expect(image).toHaveAttribute('data-view', 'frame', { timeout: 30_000 });

  // Against its session, the dust divides out and the droplet stays.
  await page.getByRole('button', { name: 'Compare with session' }).click();
  await expect(page).toHaveURL(/calView=session/);
  await expect(image).toHaveAttribute('data-view', 'session', { timeout: 60_000 });
  await expect(image).toHaveAttribute('src', /view=session/);
  const size = await image.evaluate((element: HTMLImageElement) => [element.naturalWidth, element.naturalHeight]);
  expect(size).toEqual([SIDE, SIDE]);

  // A reload restores the same view.
  await page.reload();
  await expect(panel.getByRole('heading', { name: 'flat-2.fits' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Compare with session' })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByTestId('calibration-frame-image')).toHaveAttribute('data-view', 'session', { timeout: 30_000 });
});

test('overrules a suspect flat with Use anyway, and Clear brings the grade back', async ({ page, request }) => {
  fs.mkdirSync(path.join(root, 'FLAT'), { recursive: true });
  // Twelve spots in every flat, fading from 16% to 3%: too few to make a
  // flat poor, enough to make most of them suspect.
  const depths = [0.16, 0.1, 0.05, 0.03];
  for (let index = 0; index < 4; index++) {
    writeFrame(
      path.join(root, 'FLAT', `spotted-${index}.fits`),
      { kind: 'FLAT', date: `2026-09-12T04:0${index}:00`, exposure: 2, filter: 'OIII' },
      (x, y) => {
        const spot = x >= 16 && x < 112 && y >= 16 && y < 88 && (x - 16) % 24 < 4 && (y - 16) % 24 < 4;
        return 30000 * (spot ? 1 - depths[index] : 1) + noise(x, y, index + 7, 10);
      },
    );
  }
  const created = await request.post('/api/databases/create', {
    data: { name: 'Suspect override', image_dirs: [root], slug: 'suspect-override-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  const counts = async () => (await (await request.get(`/api/db/${dbId}/calibrations/quality`)).json()).data.counts;
  await expect.poll(async () => (await counts()).measured, { timeout: 60_000 }).toBe(4);
  await expect.poll(async () => (await counts()).suspect, { timeout: 30_000 }).toBeGreaterThan(0);
  const suspect: number = (await counts()).suspect;

  await page.goto(`/#/calibration?db=${dbId}&calKind=flat`);
  const sessions = page.getByRole('navigation', { name: 'Sessions' });
  await expect(sessions.getByText(`${suspect} suspect`)).toBeVisible();
  await sessions.getByRole('button').first().click();
  const strip = page.getByRole('list', { name: 'Frames' });
  const checks = strip.getByRole('button').filter({ hasText: 'Check' });
  await expect(checks).toHaveCount(suspect);
  await checks.first().click();
  const panel = page.getByRole('complementary', { name: 'Frame' });
  await expect(panel.getByText(/^Suspect · /)).toBeVisible();

  // Overruled, it reads as OK: Used anyway, no suspect badge, no Check.
  await panel.getByRole('button', { name: 'Use anyway' }).click();
  await expect(panel.getByText('Used anyway')).toBeVisible();
  await expect(panel.getByText(/^Suspect · /)).toHaveCount(0);
  await expect(checks).toHaveCount(suspect - 1);
  await expect.poll(async () => (await counts()).used_anyway).toBe(1);
  expect((await counts()).suspect).toBe(suspect - 1);
  if (suspect > 1) await expect(sessions.getByText(`${suspect - 1} suspect`)).toBeVisible();
  else await expect(sessions.getByText(/suspect/)).toHaveCount(0);

  await panel.getByRole('button', { name: 'Clear' }).click();
  await expect(panel.getByText(/^Suspect · /)).toBeVisible();
  await expect(checks).toHaveCount(suspect);
});
