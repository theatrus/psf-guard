import { expect, test, type APIRequestContext } from '@playwright/test';
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

/** Two nights of three Hα flats, the second with new dust, imported and
 *  measured into two groups split by one change. */
async function twoGroups(request: APIRequestContext): Promise<string> {
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
  return dbId;
}

test('browses the flat groups themselves, one group at a time', async ({ page, request }) => {
  test.setTimeout(150_000);
  const dbId = await twoGroups(request);

  await page.goto(`/#/calibration?db=${dbId}`);
  const toggle = page.getByRole('group', { name: 'List' });
  await toggle.getByRole('button', { name: 'Groups' }).click();
  await expect(page).toHaveURL(/calBrowse=groups/);
  const list = page.getByRole('navigation', { name: 'Flat groups' });
  const entries = list.getByTestId('flat-group-entry');
  await expect(entries).toHaveCount(2);
  await expect(entries.first()).toHaveAccessibleName(/^Group 2 · ha · no rotator · 09-14 · 1 night · 3 flats$/i);
  // The newest is chosen first and is the current group; the change sits
  // between the two.
  await expect(entries.first()).toHaveAttribute('aria-current', 'true');
  await expect(entries.first()).toContainText('Current');
  await expect(entries.nth(1)).toContainText('Change after');
  await expect(list.getByTestId('flat-group-edge')).toContainText(/Change · new dust/);
  const detail = page.getByTestId('flat-group-detail');
  const heading = page.getByTestId('flat-group-header').getByRole('heading', { level: 2 });
  await expect(heading).toHaveText(/^Group 2 of 2 · ha · no rotator$/i);

  // Its flats in the strip, as a session's are: its night marked, the
  // first in the viewer with its numbers, and ← → step through them.
  const strip = page.getByRole('list', { name: 'Frames' });
  await expect(strip.getByTestId('calibration-strip-run')).toHaveText(['2026-09-143 flats']);
  await expect(strip.getByRole('button')).toHaveCount(3);
  await expect(strip.getByRole('button').first()).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('.calibration-frame')).toBeVisible();
  await expect(page.getByText(/^Frame 1 of 3 · ← → frames · ↑ ↓ groups/)).toBeVisible();
  await page.keyboard.press('ArrowRight');
  await expect(strip.getByRole('button').nth(1)).toHaveAttribute('aria-pressed', 'true');
  await expect(page).toHaveURL(/calFrame=/);
  await expect(page.getByText(/^Frame 2 of 3 /)).toBeVisible();

  // ↓ moves to the older group, which the address keeps across a reload.
  await page.keyboard.press('ArrowDown');
  await expect(entries.nth(1)).toHaveAttribute('aria-current', 'true');
  const key = new URL(page.url().replace('/#/', '/')).searchParams.get('calGroup');
  expect(key).toBeTruthy();
  await page.reload();
  await expect(heading).toHaveText(/^Group 1 of 2 · ha · no rotator$/i);
  await expect(entries.nth(1)).toHaveAttribute('aria-current', 'true');

  // Its edge compares across the change with the shared picture.
  const edge = detail.getByTestId('flat-group-newer-edge');
  await expect(edge).toContainText(/After it · Change · new dust/);
  await edge.getByTestId('flat-group-compare').click();
  const image = page.getByTestId('flat-ratio-viewer').getByTestId('flat-ratio-image');
  await expect
    .poll(() => image.evaluate((element: HTMLImageElement) => element.naturalWidth), { timeout: 60_000 })
    .toBe(SIDE);
  await page.getByTestId('flat-ratio-viewer').getByRole('button', { name: 'Close' }).click();
  await expect(detail.getByTestId('flat-group-feeds')).toContainText(/master/i);

  // A phone stacks the picker above the group, and nothing runs off the side.
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(list).toBeVisible();
  await expect(detail).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.setViewportSize({ width: 1280, height: 800 });

  // A member's card shows its night in the strip; Open session drops into
  // its session in the session list.
  await detail.getByTestId('flat-group-member').first().click();
  await expect(detail.getByTestId('flat-group-member').first()).toHaveAttribute('aria-current', 'true');
  await expect(page).toHaveURL(/calBrowse=groups/);
  await detail.getByTestId('flat-group-open-session').first().click();
  await expect(page).not.toHaveURL(/calBrowse=/);
  const sessions = page.getByRole('navigation', { name: 'Sessions' });
  await expect(sessions.locator('[aria-current="true"]')).toContainText('2026-09-10');
  await expect(toggle.getByRole('button', { name: 'Sessions' })).toHaveAttribute('aria-pressed', 'true');
  // Groups opens on the same group again.
  await toggle.getByRole('button', { name: 'Groups' }).click();
  await expect(heading).toHaveText(/^Group 1 of 2 · ha · no rotator$/i);
});

test('splits two nights of flats at new dust and compares them', async ({ page, request }) => {
  test.setTimeout(120_000);
  const dbId = await twoGroups(request);

  // Flat groups live in Frames: each flat session carries its group, and
  // the split sits between the two nights.
  await page.goto(`/#/calibration?db=${dbId}`);
  await page.getByRole('combobox', { name: 'Type' }).selectOption('flat');
  await expect(page).toHaveURL(/calKind=flat/);
  const list = page.getByRole('navigation', { name: 'Sessions' });
  await expect(list.getByTestId('flat-group-badge')).toHaveText([
    'Group 2 · 09-14, 1 night',
    'Group 1 · 09-10, 1 night',
  ]);
  // One filter at one angle: the change is the filter's, a split in Hα.
  const split = list.getByTestId('flat-split');
  await expect(split).toContainText(/dust/i);
  await expect(page.getByTestId('flat-rig-summary')).toContainText('Splits at 1% dust, 2% light pattern');

  // Compare draws the later night over the earlier through the queue.
  const compare = split.getByTestId('flat-compare');
  await compare.click();
  await expect(compare).toHaveAttribute('aria-pressed', 'true');
  await expect(page).toHaveURL(/calEarlier=.*calLater=|calLater=.*calEarlier=/);
  const viewer = page.getByTestId('flat-ratio-viewer');
  const image = viewer.getByTestId('flat-ratio-image');
  await expect(image).toBeVisible({ timeout: 60_000 });
  await expect
    .poll(() => image.evaluate((element: HTMLImageElement) => element.naturalWidth), { timeout: 60_000 })
    .toBe(SIDE);
  await expect(viewer.getByTestId('flat-ratio-spots')).toBeVisible();
  // The synthetic shadow is a filled disk, named by its shape.
  await expect(viewer.getByRole('list', { name: 'Spots that changed' })).toContainText('New disk: darker');
  const stretch = viewer.getByRole('group', { name: 'Stretch' });
  await stretch.getByRole('button', { name: 'Tight' }).click();
  await expect(stretch.getByRole('button', { name: 'Tight' })).toHaveAttribute('aria-pressed', 'true');

  // A reload keeps the view and the pair; the page fits a phone.
  await page.reload();
  await expect(page.getByTestId('flat-ratio-viewer')).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(list.getByTestId('flat-split')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.setViewportSize({ width: 1280, height: 800 });

  // A link to the old Flat groups tab opens Frames on flats with its pair.
  const pair = new URL(page.url().replace('/#/', '/')).searchParams;
  await page.goto(`/#/calibration?db=${dbId}&calTab=groups&calEarlier=${pair.get('calEarlier')}&calLater=${pair.get('calLater')}`);
  await expect(page).toHaveURL(/calKind=flat/);
  await expect(page).not.toHaveURL(/calTab=/);
  await expect(page.getByRole('tab', { name: 'Frames' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByTestId('flat-ratio-viewer')).toBeVisible();

  // Edit limits lands on the dust limit in Settings › Calibration.
  await page.getByRole('button', { name: /^Edit limits for / }).click();
  const dust = page.locator('.calibration-matching-settings').getByLabel('Dust split');
  await expect(dust).toBeFocused();
  await expect(dust).toBeInViewport();
});
