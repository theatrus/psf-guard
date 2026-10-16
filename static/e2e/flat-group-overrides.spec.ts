import { expect, test, type APIRequestContext } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { resetDatabases, tmpBase } from './helpers';

let root: string;

const SIDE = 128;

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
    "INSTRUME= 'Overrides Camera'",
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
      if (dusty) value *= 1 - 0.03 * Math.min(1, Math.max(0, (8 - Math.hypot(x - 40, y - 45)) / 2));
      const noise = Math.sin(x * 12.9898 + y * 78.233 + seed * 37.719) * 43758.5453;
      value *= 1 + 0.0005 * (noise - Math.floor(noise) - 0.5) * 3.46;
      pixels.writeInt16BE(Math.round(value) - 32768, (y * SIDE + x) * 2);
    }
  }
  fs.writeFileSync(file, Buffer.concat([header, pixels]));
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  root = path.join(tmpBase(), 'flat-group-overrides');
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

/** Two nights of Hα flats, the second with new dust: two measured groups. */
async function twoGroups(request: APIRequestContext): Promise<string> {
  fs.mkdirSync(path.join(root, 'FLAT'), { recursive: true });
  for (let index = 0; index < 3; index++) {
    writeFlat(path.join(root, 'FLAT', `clean-${index}.fits`), `2026-09-10T14:0${index}:00`, index, false);
    writeFlat(path.join(root, 'FLAT', `dusty-${index}.fits`), `2026-09-14T14:0${index}:00`, 10 + index, true);
  }
  const created = await request.post('/api/databases/create', {
    data: { name: 'Flat overrides', image_dirs: [root], slug: 'flat-group-overrides-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(async () => {
      const view = (await (await request.get(`/api/db/${dbId}/calibrations/flat-groups`)).json()).data;
      const family = view?.rigs[0]?.families[0];
      return `${family?.groups.length ?? 0} groups`;
    }, { timeout: 120_000 })
    .toBe('2 groups');
  return dbId;
}

test('joins two measured groups by hand, splits them again, and resets', async ({ page, request }) => {
  test.setTimeout(180_000);
  const dbId = await twoGroups(request);

  await page.goto(`/#/calibration?db=${dbId}&calBrowse=groups`);
  const list = page.getByRole('navigation', { name: 'Flat groups' });
  const entries = list.getByTestId('flat-group-entry');
  await expect(entries).toHaveCount(2);

  // Join at the measured split, from the list: one group, marked.
  await list.getByTestId('flat-group-list-join').click();
  await expect(entries).toHaveCount(1);
  await expect(entries.first()).toContainText('Joined by you');
  const detail = page.getByTestId('flat-group-detail');
  const join = detail.getByTestId('flat-group-join-note');
  // The measured numbers stay beside the person's choice.
  await expect(join).toContainText(/joined by you/);
  await expect(join).toContainText(/Measured \d+(\.\d+)?% dust/);

  // It is kept in the catalog: a reload shows it again.
  await page.reload();
  await expect(entries).toHaveCount(1);
  await expect(entries.first()).toContainText('Joined by you');

  // Split again at the second night: a split by hand where the
  // measurements split too.
  await detail.getByTestId('flat-group-split-here').click();
  await expect(entries).toHaveCount(2);
  await expect(list.getByTestId('flat-group-edge')).toHaveAttribute('data-hand', 'true');
  await entries.nth(1).click();
  await expect(detail.getByTestId('flat-group-hand')).toContainText('Split by you · measured too');

  // Reset: the measurements alone, no mark.
  await detail.getByTestId('flat-group-reset').click();
  await expect(detail.getByTestId('flat-group-hand')).toHaveCount(0);
  await expect(entries).toHaveCount(2);
  await expect(list.getByTestId('flat-group-list-join')).toBeVisible();
});
