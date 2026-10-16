import { expect, test } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { resetDatabases, tmpBase } from './helpers';

let root: string;

const SIDE = 64;
const NIGHTS = 24;

/** A small 16-bit flat from one camera, on its own night. */
function writeFlat(file: string, night: number) {
  const day = String(1 + night).padStart(2, '0');
  const cards = [
    'SIMPLE  =                    T',
    'BITPIX  =                   16',
    'NAXIS   =                    2',
    `NAXIS1  = ${String(SIDE).padStart(20)}`,
    `NAXIS2  = ${String(SIDE).padStart(20)}`,
    'BZERO   =                32768',
    "IMAGETYP= 'FLAT'",
    `DATE-OBS= '2026-08-${day}T04:00:00'`,
    'EXPTIME =                  2.0',
    "FILTER  = 'Ha'",
    'GAIN    =                  100',
    'OFFSET  =                   20',
    'CCD-TEMP=                -10.0',
    "INSTRUME= 'Sidebar Camera'",
    'END',
  ];
  const header = Buffer.from(cards.map((line) => line.padEnd(80)).join('').padEnd(2880), 'ascii');
  const pixels = Buffer.alloc(Math.ceil((SIDE * SIDE * 2) / 2880) * 2880);
  for (let index = 0; index < SIDE * SIDE; index++) {
    pixels.writeInt16BE(24000 + ((index * 7 + night) % 50) - 32768, index * 2);
  }
  fs.writeFileSync(file, Buffer.concat([header, pixels]));
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  root = path.join(tmpBase(), 'calibration-sidebar');
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

test('keeps the whole sidebar in reach while the frame side scrolls the page', async ({ page, request }) => {
  fs.mkdirSync(path.join(root, 'FLAT'), { recursive: true });
  for (let night = 0; night < NIGHTS; night++) writeFlat(path.join(root, 'FLAT', `flat-${night}.fits`), night);
  const created = await request.post('/api/databases/create', {
    data: { name: 'Calibration sidebar', image_dirs: [root], slug: 'calibration-sidebar-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(async () => {
      const progress = (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress;
      if (progress.running || !progress.outcome) return progress.error ?? 'running';
      return progress.outcome.calibration.flat;
    }, { timeout: 60_000 })
    .toBe(NIGHTS);

  // A short window: the frame's side runs past it, so the page scrolls.
  await page.setViewportSize({ width: 1280, height: 620 });
  await page.goto(`/#/calibration?db=${dbId}&calKind=flat`);
  const sessions = page.getByRole('navigation', { name: 'Sessions' });
  await expect(sessions.getByRole('button')).toHaveCount(NIGHTS);
  const main = page.locator('.app-main');
  // A flat group's detail runs well below its image; a block that tall
  // stands in for it, as these flats are too few to group.
  await page.locator('.calibration-main').evaluate((element) => {
    const detail = document.createElement('div');
    detail.style.height = '2000px';
    element.append(detail);
  });
  expect(await main.evaluate((element) => element.scrollHeight - element.clientHeight > 1000)).toBe(true);

  // At the page's foot the sidebar still fits the scroll area, from its top
  // to its bottom, rather than scrolling away above it.
  await main.evaluate((element) => { element.scrollTop = element.scrollHeight; });
  const area = (await main.boundingBox())!;
  const column = (await page.locator('.calibration-sessions-column').boundingBox())!;
  expect(column.y).toBeGreaterThanOrEqual(area.y - 1);
  expect(column.y + column.height).toBeLessThanOrEqual(area.y + area.height + 1);

  // Its list scrolls on its own to the last session.
  const last = sessions.getByRole('button').last();
  await last.scrollIntoViewIfNeeded();
  await expect(last).toBeInViewport({ ratio: 1 });
  expect(await main.evaluate((element) => element.scrollTop + element.clientHeight >= element.scrollHeight - 1)).toBe(true);
});
