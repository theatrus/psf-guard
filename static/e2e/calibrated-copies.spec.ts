import { expect, test } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { fixtureImageDir, resetDatabases, tmpBase, waitForCacheReady } from './helpers';

let slugCounter = 0;
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

/** A calibrated copy of a real fixture light: the same file with the
 *  HISTORY card Siril writes after calibration, under WBPP's `_c` name. */
function writeCalibratedCopy(rawPath: string, copyDir: string): string {
  const bytes = fs.readFileSync(rawPath);
  const cards: string[] = [];
  let headerEnd = 0;
  for (let block = 0; headerEnd === 0; block += 1) {
    for (let index = 0; index < 36; index += 1) {
      const start = block * 2880 + index * 80;
      const card = bytes.subarray(start, start + 80).toString('latin1');
      if (card.startsWith('END ')) {
        headerEnd = (block + 1) * 2880;
        break;
      }
      cards.push(card);
    }
  }
  cards.push('HISTORY Calibrated with a master dark'.padEnd(80));
  cards.push('END'.padEnd(80));
  let header = cards.join('');
  header = header.padEnd(Math.ceil(header.length / 2880) * 2880, ' ');
  fs.mkdirSync(copyDir, { recursive: true });
  const stem = path.basename(rawPath).replace(/\.[^.]+$/, '');
  const copyPath = path.join(copyDir, `${stem}_c.fits`);
  fs.writeFileSync(copyPath, Buffer.concat([Buffer.from(header, 'latin1'), bytes.subarray(headerEnd)]));
  return copyPath;
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  slugCounter += 1;
  root = path.join(tmpBase(), `calibrated-copies-${slugCounter}`);
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

test('a paired calibrated copy can be shown in the detail view', async ({ page, request }) => {
  // A catalog built from one raw frame and its calibrated copy: the copy
  // pairs with the light instead of becoming a second one.
  const fixture = fixtureFrame(fixtureImageDir());
  expect(fixture, 'a fixture FITS frame').toBeTruthy();
  const raw = path.join(root, 'LIGHT', path.basename(fixture));
  fs.mkdirSync(path.dirname(raw), { recursive: true });
  fs.copyFileSync(fixture, raw);
  writeCalibratedCopy(raw, path.join(root, 'calibrated'));

  const created = await request.post('/api/databases/create', {
    data: { name: `Calibrated copies ${slugCounter}`, image_dirs: [root], slug: `copies-e2e-${slugCounter}` },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(
      async () => {
        const status = await (await request.get(`/api/db/${dbId}/import`)).json();
        const progress = status.data.progress;
        if (progress.running || !progress.outcome) return progress.error ?? 'running';
        return `${progress.outcome.imported} light, ${progress.outcome.derivatives?.paired ?? 0} paired`;
      },
      { timeout: 60_000 }
    )
    .toBe('1 light, 1 paired');
  await waitForCacheReady(request, dbId);
  const images = (await (await request.get(`/api/db/${dbId}/images?limit=5`)).json()).data;
  expect(images).toHaveLength(1);
  expect(images[0].copies).toHaveLength(1);
  const { id, project_id: projectId } = images[0];

  await page.goto(`/#/detail/${id}?db=${encodeURIComponent(dbId)}&project=${projectId}`);
  const views = page.getByRole('group', { name: 'File shown' });
  await expect(views).toBeVisible({ timeout: 15_000 });
  await expect(views.getByRole('button', { name: 'Raw' })).toHaveAttribute('aria-pressed', 'true');

  await views.getByRole('button', { name: 'Calibrated' }).click();
  await expect(views.getByRole('button', { name: 'Calibrated' })).toHaveAttribute('aria-pressed', 'true');
  const image = page.locator('.zoom-container img.detail-main-image').first();
  await expect(image).toHaveAttribute('src', /copy=/, { timeout: 15_000 });
  await page.waitForFunction(
    (element) =>
      element instanceof HTMLImageElement &&
      element.complete &&
      element.naturalWidth > 0 &&
      element.src.includes('copy='),
    await image.elementHandle(),
    { timeout: 60_000 }
  );

  // V returns to the light's own file.
  await page.keyboard.press('v');
  await expect(views.getByRole('button', { name: 'Raw' })).toHaveAttribute('aria-pressed', 'true');
  await expect(image).not.toHaveAttribute('src', /copy=/);
});
