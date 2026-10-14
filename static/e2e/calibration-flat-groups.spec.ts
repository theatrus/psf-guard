import { expect, test } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { resetDatabases, tmpBase } from './helpers';

let root: string;

const SIDE = 256;

/** A 16-bit Hα flat; `dusty` adds a 3% shadow that the first night lacks. */
function writeFlat(file: string, date: string, seed: number, dusty: boolean) {
  const cards = [
    'SIMPLE  =                    T',
    'BITPIX  =                   16',
    'NAXIS   =                    2',
    `NAXIS1  = ${String(SIDE).padStart(20)}`,
    `NAXIS2  = ${String(SIDE).padStart(20)}`,
    'BZERO   =                32768',
    "IMAGETYP= 'FLAT'",
    `DATE-OBS= '${date}'`,
    'EXPTIME =                  2.0',
    "FILTER  = 'Ha'",
    'GAIN    =                  100',
    'OFFSET  =                   20',
    'CCD-TEMP=                -10.0',
    "INSTRUME= 'Groups Camera'",
    'END',
  ];
  const header = Buffer.from(cards.map((line) => line.padEnd(80)).join('').padEnd(2880), 'ascii');
  const pixels = Buffer.alloc(Math.ceil((SIDE * SIDE * 2) / 2880) * 2880);
  const centre = SIDE / 2;
  const reach = Math.hypot(centre, centre);
  for (let y = 0; y < SIDE; y++) {
    for (let x = 0; x < SIDE; x++) {
      const r = Math.hypot(x - centre, y - centre) / reach;
      let value = 30000 * (1 - 0.3 * r * r);
      if (dusty) value *= 1 - 0.03 * Math.min(1, Math.max(0, (12 - Math.hypot(x - 80, y - 90)) / 2));
      const noise = Math.sin(x * 12.9898 + y * 78.233 + seed * 37.719) * 43758.5453;
      value *= 1 + 0.0005 * (noise - Math.floor(noise) - 0.5) * 3.46;
      pixels.writeInt16BE(Math.round(value) - 32768, (y * SIDE + x) * 2);
    }
  }
  fs.writeFileSync(file, Buffer.concat([header, pixels]));
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  root = path.join(tmpBase(), 'calibration-flat-groups');
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

test('splits two nights of flats at new dust and compares them', async ({ page, request }) => {
  test.setTimeout(120_000);
  fs.mkdirSync(path.join(root, 'FLAT'), { recursive: true });
  for (let index = 0; index < 3; index++) {
    writeFlat(path.join(root, 'FLAT', `clean-${index}.fits`), `2026-09-10T14:0${index}:00`, index, false);
    writeFlat(path.join(root, 'FLAT', `dusty-${index}.fits`), `2026-09-14T14:0${index}:00`, 10 + index, true);
  }
  const created = await request.post('/api/databases/create', {
    data: { name: 'Flat groups', image_dirs: [root], slug: 'calibration-flat-groups-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(async () => {
      const progress = (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress;
      if (progress.running || !progress.outcome) return progress.error ?? 'running';
      return `${progress.outcome.calibration.flat} flats`;
    }, { timeout: 60_000 })
    .toBe('6 flats');
  // The grading pass measures and compares the two nights after grading.
  await expect
    .poll(async () => {
      const view = (await (await request.get(`/api/db/${dbId}/calibrations/flat-groups`)).json()).data;
      const family = view.rigs[0]?.families[0];
      return `${family?.groups.length ?? 0} groups, ${view.rigs[0]?.events.length ?? 0} changes`;
    }, { timeout: 90_000 })
    .toBe('2 groups, 1 changes');

  await page.goto(`/#/calibration?db=${dbId}`);
  await page.getByRole('tab', { name: 'Flat groups' }).click();
  await expect(page).toHaveURL(/calTab=groups/);
  const panel = page.getByRole('tabpanel', { name: 'Flat groups' });
  await expect(panel.getByTestId('flat-group')).toHaveCount(2);
  const split = panel.getByTestId('flat-split');
  await expect(split).toContainText(/dust/i);
  await expect(panel.getByTestId('flat-change-event')).toContainText(/new dust/i);

  // Compare draws the later night over the earlier through the queue.
  await split.getByTestId('flat-compare').click();
  await expect(page).toHaveURL(/calEarlier=.*calLater=|calLater=.*calEarlier=/);
  const viewer = page.getByTestId('flat-ratio-viewer');
  const image = viewer.getByTestId('flat-ratio-image');
  await expect(image).toBeVisible({ timeout: 60_000 });
  await expect
    .poll(() => image.evaluate((element: HTMLImageElement) => element.naturalWidth), { timeout: 60_000 })
    .toBe(SIDE);
  await expect(viewer.getByRole('img', { name: /spot/ })).toBeVisible();
  const stretch = viewer.getByRole('group', { name: 'Stretch' });
  await stretch.getByRole('button', { name: 'Tight' }).click();
  await expect(stretch.getByRole('button', { name: 'Tight' })).toHaveAttribute('aria-pressed', 'true');

  // A reload keeps the tab and the pair; the page fits a phone.
  await page.reload();
  await expect(page.getByTestId('flat-ratio-viewer')).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(panel.getByTestId('flat-group').first()).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
