import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { openSettings, resetDatabases, stubPixelWork, tmpBase, waitForCacheReady, selectSettingsSection } from './helpers';

/**
 * A light the calibration library flags becomes unflagged when the limit is
 * loosened for every rig, and again when it is loosened for its own rig
 * only, while a light from another rig stays flagged.
 *
 * Two rigs share one camera: an Askar and a RedCat. The library holds three
 * darks, so every light's dark master is thin (fewer than five frames).
 */

let root: string;

const RIGS = [
  { telescope: 'Askar 120', object: 'M 44', ra: 130.1, dec: 19.67, prefix: 'askar' },
  { telescope: 'RedCat 51', object: 'M 45', ra: 56.75, dec: 24.12, prefix: 'redcat' },
];
const CAMERA = 'Synthetic Cam';
const ASKAR_RIG = `${RIGS[0].telescope} · ${CAMERA}`;

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
    ...Object.entries(cards).map(([key, value]) => card(key, value)),
    'END'.padEnd(80),
  ].join('');
  const pixels = Buffer.alloc(Math.ceil((64 * 64 * 2) / 2880) * 2880);
  for (let pixel = 0; pixel < 64 * 64; pixel++) {
    pixels.writeInt16BE(level + (pixel % 11) - 32768, pixel * 2);
  }
  fs.writeFileSync(file, Buffer.concat([Buffer.from(header.padEnd(Math.ceil(header.length / 2880) * 2880), 'ascii'), pixels]));
}

/** Two lights from each rig, and three darks from the camera they share. */
function writeLibrary() {
  fs.mkdirSync(path.join(root, 'LIGHT'), { recursive: true });
  fs.mkdirSync(path.join(root, 'DARK'), { recursive: true });
  for (const rig of RIGS) {
    for (let index = 0; index < 2; index++) {
      writeFrame(path.join(root, 'LIGHT', `${rig.prefix}-${index}.fits`), {
        IMAGETYP: 'LIGHT',
        OBJECT: rig.object,
        FILTER: 'L',
        EXPTIME: 60,
        'DATE-OBS': `2026-09-10T04:0${index}:00.000`,
        RA: rig.ra,
        DEC: rig.dec,
        TELESCOP: rig.telescope,
        INSTRUME: CAMERA,
      }, 1_100);
    }
  }
  for (let index = 0; index < 3; index++) {
    writeFrame(path.join(root, 'DARK', `dark-${index}.fits`), {
      IMAGETYP: 'DARK',
      EXPTIME: 60,
      'DATE-OBS': `2026-09-09T12:0${index}:00.000`,
      TELESCOP: RIGS[0].telescope,
      INSTRUME: CAMERA,
    }, 500);
  }
}

interface ImageRow {
  id: number;
  project_id: number;
  metadata: unknown;
}

/** Each rig's first light: its id and project. */
async function lights(request: APIRequestContext, dbId: string) {
  const images = (await (await request.get(`/api/db/${dbId}/images?limit=1000`)).json()).data as ImageRow[];
  const of = (prefix: string) => {
    const image = images.find((row) => JSON.stringify(row.metadata).includes(`${prefix}-0.fits`));
    expect(image, `a light from ${prefix}`).toBeTruthy();
    return { id: image!.id, project: image!.project_id };
  };
  return { askar: of('askar'), redcat: of('redcat') };
}

/** Each project's calibration health level, once the background pass has judged it. */
async function levels(request: APIRequestContext, dbId: string): Promise<Record<number, string | null>> {
  const health = (await (await request.get(`/api/db/${dbId}/calibration-health`)).json()).data;
  return Object.fromEntries(
    (health.projects as { project_id: number; level: string | null }[]).map((project) => [project.project_id, project.level]),
  );
}

async function expectLevels(
  request: APIRequestContext,
  dbId: string,
  expected: Record<number, string | null>,
) {
  await expect.poll(() => levels(request, dbId), { timeout: 60_000 }).toEqual(expected);
}

/** The light's Calibration panel, opened. */
async function calibrationPanel(page: Page, dbId: string, light: { id: number; project: number }) {
  await page.goto(`/#/detail/${light.id}?db=${encodeURIComponent(dbId)}&project=${light.project}`);
  const panel = page.getByTestId('calibration-panel');
  const heading = panel.getByRole('button', { name: /^Calibration/ });
  await expect(heading).toBeVisible({ timeout: 15_000 });
  if ((await heading.getAttribute('aria-expanded')) !== 'true') await heading.click();
  await expect(panel.getByText('Matching the light against the library…')).toHaveCount(0, { timeout: 15_000 });
  return panel;
}

async function openCalibrationSettings(page: Page) {
  // From the Library: the image detail covers the header's menu.
  await page.goto('/');
  await openSettings(page);
  await selectSettingsSection(page, 'calibration');
  return page.locator('.calibration-matching-settings');
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  root = path.join(tmpBase(), 'calibration-thresholds');
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(async ({ request }) => {
  // Every rig inherits again and the global limit is the default.
  const settings = (await (await request.get('/api/settings/calibration')).json()).data;
  for (const rig of settings.rigs as { rig_uuid: string; rules: object }[]) {
    if (Object.keys(rig.rules).length > 0) {
      expect((await request.delete(`/api/settings/calibration/rigs/${rig.rig_uuid}`)).ok()).toBeTruthy();
    }
  }
  expect((await request.put('/api/settings/calibration', {
    data: { rules: { thin_master_frames: null } },
  })).ok()).toBeTruthy();
  fs.rmSync(root, { recursive: true, force: true });
});

test('a thin-master flag clears when the limit is loosened globally, then for one rig only', async ({
  page, request,
}, testInfo) => {
  test.setTimeout(180_000);
  writeLibrary();
  const created = await request.post('/api/databases/create', {
    data: { name: 'Calibration thresholds', image_dirs: [root], slug: 'calibration-thresholds-e2e' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(async () => {
      const progress = (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress;
      if (progress.running || !progress.outcome) return progress.error ?? 'running';
      return `${progress.outcome.imported} light, ${progress.outcome.calibration.dark} dark`;
    }, { timeout: 60_000 })
    .toBe('4 light, 3 dark');
  await waitForCacheReady(request, dbId);
  const { askar, redcat } = await lights(request, dbId);
  expect(askar.project).not.toBe(redcat.project);
  await expectLevels(request, dbId, { [askar.project]: 'poor', [redcat.project]: 'poor' });
  await stubPixelWork(page);

  // Flagged: three darks make a thin master.
  let panel = await calibrationPanel(page, dbId, askar);
  await expect(panel.getByRole('button', { name: /Its dark master would use only 3 frames/ })).toBeVisible();
  await expect(panel.getByText('Checked by the global settings')).toBeVisible();
  await panel.screenshot({ path: testInfo.outputPath('panel-flagged.png') });

  // Loosened for every rig: the thin limit becomes three.
  let settings = await openCalibrationSettings(page);
  const thin = settings.getByLabel('Minimum frames in bias and dark masters', { exact: true });
  await expect(thin).toHaveAttribute('placeholder', '5');
  await thin.fill('3');
  const saved = page.waitForResponse((response) =>
    response.request().method() === 'PUT' && response.url().endsWith('/api/settings/calibration'));
  await settings.getByRole('button', { name: 'Save', exact: true }).click();
  expect((await saved).ok()).toBeTruthy();
  await expectLevels(request, dbId, { [askar.project]: null, [redcat.project]: null });
  await page.getByRole('button', { name: 'Close settings', exact: true }).click();
  panel = await calibrationPanel(page, dbId, askar);
  await expect(panel.getByText('The library calibrates this light with no findings.')).toBeVisible();
  await expect(panel.getByRole('list', { name: 'Settings that differ from the defaults' }))
    .toContainText('Minimum frames in bias and dark masters3 framesglobal');

  // Back to the default: both rigs are flagged again.
  expect((await request.put('/api/settings/calibration', {
    data: { rules: { thin_master_frames: null } },
  })).ok()).toBeTruthy();
  await expectLevels(request, dbId, { [askar.project]: 'poor', [redcat.project]: 'poor' });

  // Loosened for the Askar alone, from its own light's panel.
  panel = await calibrationPanel(page, dbId, askar);
  await panel.getByRole('button', { name: `Tune checks for ${ASKAR_RIG}` }).click();
  const editor = page.getByRole('region', { name: `${ASKAR_RIG} calibration settings` });
  await expect(editor).toBeVisible();
  await expect(page.getByRole('button', { name: new RegExp(`^${ASKAR_RIG}`) })).toHaveAttribute('aria-expanded', 'true');
  const own = editor.getByRole('button', { name: 'Minimum frames in bias and dark masters: override global setting' });
  await expect(own).toHaveAttribute('aria-pressed', 'false');
  await expect(own.locator('xpath=ancestor::div[contains(@class, "calibration-rule-row")]'))
    .toContainText('5 frames · default');
  await own.click();
  await editor.getByLabel('Minimum frames in bias and dark masters', { exact: true }).fill('3');
  const rigSaved = page.waitForResponse((response) =>
    response.request().method() === 'PUT' && response.url().includes('/api/settings/calibration/rigs/'));
  await editor.getByRole('button', { name: 'Save rig' }).click();
  expect((await rigSaved).ok()).toBeTruthy();
  await expect(page.getByRole('button', { name: new RegExp(`^${ASKAR_RIG}`) })).toContainText('1 override');
  await editor.screenshot({ path: testInfo.outputPath('rig-override.png') });
  await expectLevels(request, dbId, { [askar.project]: null, [redcat.project]: 'poor' });
  await page.getByRole('button', { name: 'Close settings', exact: true }).click();

  // The Askar's light is clear and says whose limit cleared it; the RedCat's is still flagged.
  panel = await calibrationPanel(page, dbId, askar);
  await expect(panel.getByText('The library calibrates this light with no findings.')).toBeVisible();
  await expect(panel.getByText(`Checked by ${ASKAR_RIG}'s own settings`)).toBeVisible();
  await expect(panel.getByRole('list', { name: 'Settings that differ from the defaults' }))
    .toContainText('Minimum frames in bias and dark masters3 framesthis rig');
  await panel.screenshot({ path: testInfo.outputPath('panel-cleared.png') });
  panel = await calibrationPanel(page, dbId, redcat);
  await expect(panel.getByRole('button', { name: /Its dark master would use only 3 frames/ })).toBeVisible();
  await expect(panel.getByText('Checked by the global settings')).toBeVisible();

  // The settings read the same after a reload.
  await page.reload();
  settings = await openCalibrationSettings(page);
  await expect(settings.getByLabel('Minimum frames in bias and dark masters', { exact: true })).toHaveValue('');
  await page.getByRole('button', { name: new RegExp(`^${ASKAR_RIG}`) }).click();
  await expect(page.getByRole('region', { name: `${ASKAR_RIG} calibration settings` }).getByLabel('Minimum frames in bias and dark masters', { exact: true }))
    .toHaveValue('3');
});

test('the calibration checks fit a phone', async ({ page, request }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  writeLibrary();
  const created = await request.post('/api/databases/create', {
    data: { name: 'Calibration thresholds', image_dirs: [root], slug: 'calibration-thresholds-phone' },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const dbId: string = (await created.json()).data.database.id;
  await expect
    .poll(async () => (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress.outcome?.imported ?? 0,
      { timeout: 60_000 })
    .toBe(4);
  await page.goto('/');
  const settings = await openCalibrationSettings(page);
  await expect(settings.getByLabel('Minimum frames in bias and dark masters', { exact: true })).toBeVisible();
  const fits = (locator: ReturnType<Page['locator']>) =>
    locator.evaluate((element) => element.scrollWidth <= element.clientWidth + 1);
  expect(await fits(settings)).toBe(true);
  await page.getByText('Frame grades').click();
  await expect(settings.getByLabel('Flat light pattern: suspect at')).toBeVisible();
  expect(await fits(settings)).toBe(true);
  const rig = page.getByRole('button', { name: new RegExp(`^${ASKAR_RIG}`) });
  await rig.click();
  const editor = page.getByRole('region', { name: `${ASKAR_RIG} calibration settings` });
  // The light checks start open; the other groups unfold.
  await editor.getByRole('button', { name: 'Minimum frames in bias and dark masters: override global setting' }).click();
  await expect(editor.getByLabel('Minimum frames in bias and dark masters', { exact: true })).toBeVisible();
  await editor.getByText('Dark masters', { exact: true }).click();
  await editor.getByRole('button', { name: 'Maximum time difference for darks: override global setting' }).click();
  await expect(editor.getByLabel('Maximum time difference for darks', { exact: true })).toBeVisible();
  expect(await fits(page.locator('.calibration-rig-settings'))).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
