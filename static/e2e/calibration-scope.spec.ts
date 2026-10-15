import { expect, test, type APIRequestContext } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { resetDatabases, stubPixelWork, tmpBase, waitForCacheReady } from './helpers';

/**
 * Calibration opened from a project shows the calibration that project's
 * lights use; its chip's ✕ widens to the rig's whole library.
 *
 * One rig shoots M 44 in Ha at 300 s and M 67 in OIII at 120 s. The library
 * holds Ha and OIII flats and 300 s and 120 s darks: M 44 uses half of it.
 */

let root: string;

const TELESCOPE = 'Askar 120';
const CAMERA = 'Synthetic Cam';

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
    ...Object.entries({ TELESCOP: TELESCOPE, INSTRUME: CAMERA, ...cards }).map(([key, value]) => card(key, value)),
    'END'.padEnd(80),
  ].join('');
  const pixels = Buffer.alloc(Math.ceil((64 * 64 * 2) / 2880) * 2880);
  for (let pixel = 0; pixel < 64 * 64; pixel++) {
    pixels.writeInt16BE(level + (pixel % 11) - 32768, pixel * 2);
  }
  fs.writeFileSync(file, Buffer.concat([Buffer.from(header.padEnd(Math.ceil(header.length / 2880) * 2880), 'ascii'), pixels]));
}

function writeLibrary() {
  for (const folder of ['LIGHT', 'FLAT', 'DARK']) fs.mkdirSync(path.join(root, folder), { recursive: true });
  const targets = [
    { object: 'M 44', filter: 'Ha', exposure: 300, ra: 130.1, dec: 19.67, hour: 4 },
    { object: 'M 67', filter: 'OIII', exposure: 120, ra: 132.85, dec: 11.81, hour: 5 },
  ];
  for (const target of targets) {
    for (let index = 0; index < 2; index++) {
      writeFrame(path.join(root, 'LIGHT', `${target.object.replace(' ', '')}-${index}.fits`), {
        IMAGETYP: 'LIGHT',
        OBJECT: target.object,
        FILTER: target.filter,
        EXPTIME: target.exposure,
        'DATE-OBS': `2026-09-10T0${target.hour}:0${index}:00.000`,
        RA: target.ra,
        DEC: target.dec,
      }, 1_100);
    }
    for (let index = 0; index < 3; index++) {
      writeFrame(path.join(root, 'FLAT', `flat-${target.filter}-${index}.fits`), {
        IMAGETYP: 'FLAT',
        FILTER: target.filter,
        EXPTIME: 2,
        'DATE-OBS': `2026-09-10T1${target.hour}:0${index}:00.000`,
      }, 24_000);
      writeFrame(path.join(root, 'DARK', `dark-${target.exposure}-${index}.fits`), {
        IMAGETYP: 'DARK',
        EXPTIME: target.exposure,
        'DATE-OBS': `2026-09-09T1${target.hour}:0${index}:00.000`,
      }, 500);
    }
  }
}

async function projectId(request: APIRequestContext, dbId: string, name: string): Promise<number> {
  const projects = (await (await request.get(`/api/db/${dbId}/projects`)).json()).data as { id: number; name: string }[];
  const found = projects.find((project) => project.name === name);
  expect(found, `project ${name}`).toBeTruthy();
  return found!.id;
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  root = path.join(tmpBase(), 'calibration-scope');
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

test('Calibration opened from a project shows its calibration, and ✕ widens to the rig', async ({ page, request }) => {
  test.setTimeout(120_000);
  writeLibrary();
  const created = await request.post('/api/databases/create', {
    data: { name: 'Askar 120', image_dirs: [root], slug: 'calibration-scope-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(async () => {
      const progress = (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress;
      if (progress.running || !progress.outcome) return progress.error ?? 'running';
      const { imported, calibration } = progress.outcome;
      return `${imported} lights, ${calibration.flat} flats, ${calibration.dark} darks`;
    }, { timeout: 60_000 })
    .toBe('4 lights, 6 flats, 6 darks');
  await waitForCacheReady(request, dbId);
  const m44 = await projectId(request, dbId, 'M 44');
  await stubPixelWork(page);

  // Open the project, then Calibration from the header.
  await page.goto(`/#/grid?db=${dbId}&project=${m44}`);
  await page.getByRole('button', { name: 'Calibration', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`calProject=${m44}`));

  // Scoped: the Ha flats and the 300 s darks its lights use.
  const chip = page.getByRole('group', { name: 'Scope: M 44 on Askar 120' });
  await expect(chip).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId('calibration-scope-heading')).toHaveText(/for M 44/);
  // The header names the rig only.
  await expect(page.getByTestId('calibration-rig')).toHaveText(/Askar 120/);
  const sessions = page.getByRole('navigation', { name: 'Sessions' });
  await expect(sessions.getByRole('button')).toHaveCount(2, { timeout: 30_000 });
  await expect(sessions).toContainText('Ha');
  await expect(sessions).not.toContainText('OIII');

  // ✕ widens to the rig's whole library.
  await chip.getByRole('button', { name: "Show all of Askar 120's calibration" }).click();
  await expect(chip).toHaveCount(0);
  await expect(page).not.toHaveURL(/calProject=/);
  await expect(sessions.getByRole('button')).toHaveCount(4);
  await expect(sessions).toContainText('OIII');

  // A reload keeps the wide view.
  await page.reload();
  await expect(sessions.getByRole('button')).toHaveCount(4);
  await expect(page.getByRole('group', { name: /^Scope: / })).toHaveCount(0);
});
