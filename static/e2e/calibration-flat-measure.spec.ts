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
    "INSTRUME= 'Measure Camera'",
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
  root = path.join(tmpBase(), 'calibration-flat-measure');
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(async ({ request }) => {
  expect((await request.put('/api/settings/calibration', {
    data: { rules: { flat_split_dust_pct: null } },
  })).ok()).toBeTruthy();
  fs.rmSync(root, { recursive: true, force: true });
});

test('measures flats as a job of its own, and a changed limit regroups without reading them', async ({
  page, request,
}) => {
  test.setTimeout(120_000);
  fs.mkdirSync(path.join(root, 'FLAT'), { recursive: true });
  for (let index = 0; index < 3; index++) {
    writeFlat(path.join(root, 'FLAT', `clean-${index}.fits`), `2026-09-10T14:0${index}:00`, index, false);
    writeFlat(path.join(root, 'FLAT', `dusty-${index}.fits`), `2026-09-14T14:0${index}:00`, 10 + index, true);
  }
  const created = await request.post('/api/databases/create', {
    data: { name: 'Flat measure', image_dirs: [root], slug: 'calibration-flat-measure-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  const job = async () => (await (await request.get(`/api/db/${dbId}/calibrations/flat-measure`)).json()).data;

  // The import queues the rig it touched; the job measures both nights and
  // compares them, then goes idle.
  await expect
    .poll(async () => {
      const status = await job();
      return status.state === 'idle' && status.last
        ? `${status.last.measured} measured, ${status.last.compared} compared, ${status.last.frames_read} read`
        : status.state;
    }, { timeout: 90_000 })
    .toBe('2 measured, 1 compared, 6 read');
  const groups = async () => {
    const view = (await (await request.get(`/api/db/${dbId}/calibrations/flat-groups`)).json()).data;
    return `${view.rigs[0]?.families[0]?.groups.length ?? 0} groups, ${view.rigs[0]?.events.length ?? 0} changes`;
  };
  expect(await groups()).toBe('2 groups, 1 changes');

  // Frames with one rig chosen says it shows that rig; with every rig in
  // view it lifts none.
  const rig: string = (await (await request.get(`/api/db/${dbId}/calibrations/flat-groups`)).json())
    .data.rigs[0].rig_uuid;
  const watched = page.waitForRequest((asked) =>
    asked.method() === 'POST' && asked.url().endsWith('/calibrations/flat-measure/watch'));
  await page.goto(`/#/calibration?db=${dbId}&calKind=flat&calRig=${rig}`);
  expect((await watched).postDataJSON()).toEqual({ rig });
  await expect(page.getByTestId('flat-rig-summary')).toContainText('Splits at 1% dust');
  await expect(page.getByTestId('flat-groups-measuring')).toHaveCount(0);
  // The look queued a check of the rig, which read nothing.
  await expect
    .poll(async () => {
      const status = await job();
      return status.state === 'idle' ? `${status.last.frames_read} read` : status.state;
    }, { timeout: 30_000 })
    .toBe('0 read');

  // A looser dust limit regroups at once from the stored numbers; the job
  // then finds nothing to read.
  const before = (await job()).finished_at;
  expect((await request.put('/api/settings/calibration', {
    data: { rules: { flat_split_dust_pct: 10 } },
  })).ok()).toBeTruthy();
  expect(await groups()).toBe('1 groups, 0 changes');
  await expect
    .poll(async () => {
      const status = await job();
      return status.state === 'idle' && status.finished_at !== before
        ? `${status.last.measured} measured, ${status.last.frames_read} read`
        : status.state;
    }, { timeout: 30_000 })
    .toBe('0 measured, 0 read');
  await page.reload();
  await expect(page.getByTestId('flat-rig-summary')).toContainText('Splits at 10% dust');
});
