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

/** A small 16-bit bias frame from a camera the fixture lights do not use. */
function writeBias(file: string, index: number) {
  const cards = [
    'SIMPLE  =                    T',
    'BITPIX  =                   16',
    'NAXIS   =                    2',
    'NAXIS1  =                   64',
    'NAXIS2  =                   64',
    'BZERO   =                32768',
    "IMAGETYP= 'BIAS'",
    `DATE-OBS= '2026-01-0${index + 1}T12:00:00'`,
    'GAIN    =                  100',
    'OFFSET  =                   20',
    "INSTRUME= 'Other Camera'",
    'END',
  ];
  const header = Buffer.from(cards.map((card) => card.padEnd(80)).join('').padEnd(2880), 'ascii');
  const pixels = Buffer.alloc(Math.ceil((64 * 64 * 2) / 2880) * 2880);
  for (let pixel = 0; pixel < 64 * 64; pixel++) {
    pixels.writeInt16BE(500 + (pixel % 7) - 32768, pixel * 2);
  }
  fs.writeFileSync(file, Buffer.concat([header, pixels]));
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  root = path.join(tmpBase(), 'calibration-health');
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

test('a project whose camera the calibration library has nothing for is flagged Poor', async ({ page, request }) => {
  // One real light, and a library of bias frames from another camera only.
  const fixture = fixtureFrame(fixtureImageDir());
  expect(fixture, 'a fixture FITS frame').toBeTruthy();
  fs.mkdirSync(path.join(root, 'LIGHT'), { recursive: true });
  fs.copyFileSync(fixture, path.join(root, 'LIGHT', path.basename(fixture)));
  fs.mkdirSync(path.join(root, 'BIAS'), { recursive: true });
  for (let index = 0; index < 3; index++) writeBias(path.join(root, 'BIAS', `bias-${index}.fits`), index);

  const created = await request.post('/api/databases/create', {
    data: { name: 'Calibration health', image_dirs: [root], slug: 'calibration-health-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(async () => {
      const progress = (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress;
      if (progress.running || !progress.outcome) return progress.error ?? 'running';
      return `${progress.outcome.imported} light, ${progress.outcome.calibration.bias} bias`;
    }, { timeout: 60_000 })
    .toBe('1 light, 3 bias');
  await waitForCacheReady(request, dbId);
  // Grading follows the import, and the health pass follows grading.
  await expect
    .poll(async () => {
      const health = (await (await request.get(`/api/db/${dbId}/calibration-health`)).json()).data;
      return health.projects.map((project: { level: string | null }) => project.level).join(',');
    }, { timeout: 60_000 })
    .toBe('poor');

  await page.goto('/');
  const pill = page.getByRole('button', { name: /^Calibration Poor for / });
  await expect(pill).toBeVisible({ timeout: 15_000 });
  await expect(pill).toHaveText(/Poor/);
  await pill.click();
  const quality = page.getByRole('region', { name: 'Quality' });
  await expect(quality.getByText(/No calibration frames match its camera and settings: 1 light/)).toBeVisible();

  // The Library's Show filter keeps the project.
  await page.keyboard.press('Escape');
  await page.goto('/#/?show=calibration');
  await expect(page.getByRole('button', { name: /^Calibration Poor for / })).toBeVisible();
});
