import { expect, test, type APIRequestContext } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { resetDatabases, stubPixelWork, tmpBase, waitForCacheReady } from './helpers';

/**
 * A flat group's detail counts the lights that take its flats, and its
 * link opens Images on exactly those lights.
 *
 * M 44 has two Ha lights and one OIII light, shot the same night; the
 * library holds one night of Ha flats and one of OIII. The Ha group feeds
 * the two Ha lights and not the OIII one.
 */

let root: string;

function card(key: string, value: string | number | boolean): string {
  const text =
    typeof value === 'string'
      ? `${key.padEnd(8)}= '${value}'`
      : `${key.padEnd(8)}= ${(typeof value === 'boolean' ? (value ? 'T' : 'F') : String(value)).padStart(20)}`;
  return text.padEnd(80);
}

/** A small 16-bit frame with N.I.N.A.-style headers. */
function writeFrame(file: string, cards: Record<string, string | number>, level: number) {
  const header = [
    card('SIMPLE', true),
    card('BITPIX', 16),
    card('NAXIS', 2),
    card('NAXIS1', 64),
    card('NAXIS2', 64),
    card('BZERO', 32768),
    ...Object.entries({ TELESCOP: 'Askar 120', INSTRUME: 'Synthetic Cam', ...cards }).map(([key, value]) => card(key, value)),
    'END'.padEnd(80),
  ].join('');
  const pixels = Buffer.alloc(Math.ceil((64 * 64 * 2) / 2880) * 2880);
  for (let pixel = 0; pixel < 64 * 64; pixel++) {
    pixels.writeInt16BE(level + (pixel % 11) - 32768, pixel * 2);
  }
  fs.writeFileSync(file, Buffer.concat([Buffer.from(header.padEnd(Math.ceil(header.length / 2880) * 2880), 'ascii'), pixels]));
}

function writeLibrary() {
  for (const folder of ['LIGHT', 'FLAT']) fs.mkdirSync(path.join(root, folder), { recursive: true });
  const lights = [['Ha', 0], ['Ha', 1], ['OIII', 2]] as const;
  for (const [filter, index] of lights) {
    writeFrame(path.join(root, 'LIGHT', `M44-${filter}-${index}.fits`), {
      IMAGETYP: 'LIGHT',
      OBJECT: 'M 44',
      FILTER: filter,
      EXPTIME: 300,
      'DATE-OBS': `2026-09-10T04:0${index}:00.000`,
      RA: 130.1,
      DEC: 19.67,
    }, 1_100);
  }
  for (const filter of ['Ha', 'OIII']) {
    for (let index = 0; index < 3; index++) {
      writeFrame(path.join(root, 'FLAT', `flat-${filter}-${index}.fits`), {
        IMAGETYP: 'FLAT',
        FILTER: filter,
        EXPTIME: 2,
        'DATE-OBS': `2026-09-10T14:0${index}:00.000`,
      }, 24_000);
    }
  }
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  root = path.join(tmpBase(), 'calibration-flat-group-lights');
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

async function waitForUse(request: APIRequestContext, dbId: string): Promise<void> {
  await expect
    .poll(async () => {
      const use = (await (await request.get(`/api/db/${dbId}/calibrations/flat-groups/use`)).json()).data;
      if (!use.computed || use.refreshing) return 'working';
      return `${use.lights} lights, ${use.pending} pending`;
    }, { timeout: 90_000 })
    .toBe('3 lights, 0 pending');
}

test('a flat group links to exactly the lights that take its flats', async ({ page, request }) => {
  test.setTimeout(180_000);
  writeLibrary();
  const created = await request.post('/api/databases/create', {
    data: { name: 'Askar 120', image_dirs: [root], slug: 'calibration-flat-group-lights-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(async () => {
      const progress = (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress;
      if (progress.running || !progress.outcome) return progress.error ?? 'running';
      return `${progress.outcome.imported} lights, ${progress.outcome.calibration.flat} flats`;
    }, { timeout: 60_000 })
    .toBe('3 lights, 6 flats');
  await waitForCacheReady(request, dbId);
  // Both nights of flats measured into a group each.
  await expect
    .poll(async () => {
      const view = (await (await request.get(`/api/db/${dbId}/calibrations/flat-groups`)).json()).data;
      return view.rigs.flatMap((rig: { families: unknown[] }) => rig.families).length;
    }, { timeout: 90_000 })
    .toBe(2);
  // The calibration check reads the lights' headers after grading; until
  // it has, the counts say so.
  await waitForUse(request, dbId);
  await stubPixelWork(page);

  await page.goto(`/#/calibration?db=${dbId}&calBrowse=groups&calFilter=Ha`);
  const detail = page.getByTestId('flat-group-detail');
  await expect(page.getByTestId('flat-group-header').getByRole('heading', { level: 2 })).toHaveText(/ · ha · /i);
  const feeds = detail.getByTestId('flat-group-feeds');
  await expect(feeds.getByTestId('flat-group-lights')).toHaveText('2 lights use this flat group.');
  const link = feeds.getByTestId('flat-group-lights-link');
  await expect(link).toHaveText('M 44: 2 lights');
  await link.click();

  // Images on exactly those lights: M 44's OIII light is left out.
  await expect(page).toHaveURL(/#\/grid\?/);
  await expect(page).toHaveURL(new RegExp(`flatGroup=${dbId}%3A`));
  await expect(page.locator('.grid-stats')).toContainText('2 of 3 images', { timeout: 30_000 });
  await expect(page.locator('.grid-stats')).toContainText('Lights of one flat group');
});
