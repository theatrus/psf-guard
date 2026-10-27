import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import * as fs from 'fs';
import * as path from 'path';
import { resetDatabases, tmpBase, openSettings, selectSettingsSection } from './helpers';

let counter = 0;
let root: string;

/** A small FITS frame with the headers import reads; any of OBJECT and the
 *  coordinates can be left out, as Hocus Focus leaves them out of its
 *  autofocus frames. */
function writeFrame(
  file: string,
  frame: { object?: string; ra?: number; dec?: number; dateObs: string; exposure: number }
) {
  const card = (text: string) => text.padEnd(80, ' ').slice(0, 80);
  const value = (key: string, text: string) => card(`${key.padEnd(8)}= ${text}`);
  const number = (key: string, n: number) => value(key, String(n).padStart(20));
  const cards = [
    value('SIMPLE', 'T'.padStart(20)),
    number('BITPIX', 16),
    number('NAXIS', 2),
    number('NAXIS1', 10),
    number('NAXIS2', 10),
    value('IMAGETYP', "'LIGHT   '"),
    ...(frame.object ? [value('OBJECT', `'${frame.object}'`)] : []),
    value('FILTER', "'L       '"),
    value('DATE-OBS', `'${frame.dateObs}'`),
    number('EXPTIME', frame.exposure),
    number('GAIN', 100),
    ...(frame.ra !== undefined && frame.dec !== undefined
      ? [number('RA', frame.ra), number('DEC', frame.dec)]
      : []),
    value('TELESCOP', "'TestScope'"),
    value('INSTRUME', "'TestCam '"),
    card('END'),
  ];
  let header = cards.join('');
  header = header.padEnd(Math.ceil(header.length / 2880) * 2880, ' ');
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, Buffer.concat([Buffer.from(header, 'latin1'), Buffer.alloc(2880)]));
}

function targetNames(databasePath: string): string[] {
  const reader = new Database(databasePath, { readonly: true });
  try {
    return (reader.prepare('SELECT name FROM target ORDER BY name').all() as { name: string }[]).map(
      (row) => row.name
    );
  } finally {
    reader.close();
  }
}

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  counter += 1;
  root = path.join(tmpBase(), `scan-rules-${counter}`);
  fs.rmSync(root, { recursive: true, force: true });
});

test.afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

test('auto-import keeps autofocus frames out and holds a new target for Review', async ({
  page,
  request,
}) => {
  const name = `Scan rules ${counter}`;
  writeFrame(path.join(root, 'M31/LIGHT/m31_001.fits'), {
    object: 'M31',
    ra: 10.6847,
    dec: 41.2687,
    dateObs: '2026-01-10T01:00:00.000',
    exposure: 300,
  });
  const created = await request.post('/api/databases/create', {
    data: { name, image_dirs: [root], slug: `scan-rules-${counter}` },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const database = (await created.json()).data.database;
  const dbId: string = database.id;
  const databasePath: string = database.database_path;
  const progress = async () => (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress;
  await expect.poll(async () => (await progress()).stage, { timeout: 60_000 }).toBe('complete');

  // The night's new frames, filed the way N.I.N.A. files them: a target the
  // catalog does not have, and Hocus Focus autofocus frames with no OBJECT
  // beside them.
  for (const [index, time] of ['02:00', '02:05'].entries()) {
    writeFrame(path.join(root, `NGC 7000/2026-01-11/LIGHT/n7000_${index + 1}.fits`), {
      object: 'NGC 7000',
      ra: 314.68,
      dec: 44.3,
      dateObs: `2026-01-11T${time}:00.000`,
      exposure: 300,
    });
  }
  for (const step of [1, 2, 3]) {
    writeFrame(path.join(root, `Hocus Focus/AutoFocus_1/attempt01/af_0${step}.fits`), {
      dateObs: `2026-01-11T01:5${step}:00.000`,
      exposure: 2,
    });
  }

  const saved = await request.put(`/api/databases/${dbId}`, {
    data: {
      scan_rules: { exclude: ['**/Hocus Focus/**'] },
      autoimport: { enabled: true, on_open: false, interval_minutes: 60, backfill: false },
    },
  });
  expect(saved.ok(), await saved.text()).toBeTruthy();
  expect((await saved.json()).data.autoimport.new_targets).toBe('ask');
  const run = await request.post(`/api/db/${dbId}/autoimport/run`);
  expect(run.ok(), await run.text()).toBeTruthy();
  await expect
    .poll(async () => {
      const current = await progress();
      return current.running ? 'running' : `${current.trigger} ${current.stage}`;
    })
    .toBe('automatic complete');
  const outcome = (await progress()).outcome;
  expect(outcome.skipped_unmatched).toBe(2);
  expect(outcome.scan_rules.folders_skipped).toBe(1);
  expect(targetNames(databasePath)).toEqual(['M31']);

  await page.goto('/');
  const alreadyOpen = await page
    .getByRole('tab', { name: 'Overview' })
    .waitFor({ state: 'visible', timeout: 2000 })
    .then(
      () => true,
      () => false
    );
  if (!alreadyOpen) await openSettings(page);
  await selectSettingsSection(page, 'database-import', name);
  const row = page.getByRole('region', { name: `${name} import`, exact: true });
  await expect(row).toContainText('1 folder(s) skipped by scan rules');
  await expect(row).toContainText('2 held: no matching project');

  await row.getByRole('button', { name: 'Review', exact: true }).click();
  const panel = page.locator('.import-progress-panel');
  await expect(panel.getByRole('checkbox', { name: /^Skip processed files/ })).toBeVisible();
  await expect(panel).toContainText('When pairing is off, also skip calibrated and registered copies.');
  await expect(panel).toContainText('NEW NGC 7000 — 1 target(s), 2 frame(s)', { timeout: 30_000 });
  // The held folder sits below the two levels the folder tree lists, so the
  // preview shows it on its own, ticked.
  await expect(
    panel.getByRole('checkbox', { name: path.join(root, 'NGC 7000/2026-01-11/LIGHT') })
  ).toBeChecked();
  await panel.getByRole('button', { name: 'Import 2 frame(s)' }).click();
  await expect
    .poll(() => targetNames(databasePath), { timeout: 30_000 })
    .toEqual(['M31', 'NGC 7000']);
});

test('new image databases apply scan rules immediately and keep them editable on Import', async ({ page, request }, testInfo) => {
  for (const [folder, object] of [['M31/LIGHT', 'M31'], ['Hocus Focus/LIGHT', 'Focus'], ['Other', 'Other'], ['Held/LIGHT', 'Held']]) {
    writeFrame(path.join(root, folder, `${object}.fits`), {
      object, ra: object === 'M31' ? 10.6847 : 50, dec: 41.2687,
      dateObs: '2026-01-10T01:00:00.000', exposure: 300,
    });
  }
  fs.writeFileSync(path.join(root, 'Held/.psfguardignore'), '');
  await page.goto('/');
  await page.locator('.tauri-settings').getByRole('button', { name: /New Database from Images/ }).click();
  await page.getByLabel('Display name (optional):').fill('Filtered first import');
  await page.getByPlaceholder('Type an absolute path and press Add').fill(root);
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await page.getByText('Scan rules · Defaults', { exact: true }).click();
  await page.getByLabel('Exclude:', { exact: true }).fill('**/Hocus Focus/**');
  await page.getByLabel('Include only:', { exact: true }).fill('LIGHT');
  await page.getByRole('checkbox', { name: 'Require LIGHT in the image-type header' }).check();
  await page.getByRole('checkbox', { name: 'Lights need a target name' }).check();
  await page.getByRole('checkbox', { name: 'Match targets by position' }).uncheck();
  await page.getByLabel('Minimum light exposure (s):').fill('60');
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
    await page.getByRole('group', { name: 'Scan rules', exact: true }).evaluate(element => {
      const scroller = element.closest('.modal-body')!;
      scroller.scrollTop += element.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    });
    await page.screenshot({ path: testInfo.outputPath(`create-scan-rules-${width}.png`), animations: 'disabled' });
    expect(await page.locator('.tauri-settings').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  }
  const responsePromise = page.waitForResponse(response => response.url().endsWith('/api/databases/create') && response.request().method() === 'POST');
  await page.getByRole('button', { name: 'Create & Import', exact: true }).click();
  const created = await responsePromise;
  expect(created.ok(), await created.text()).toBe(true);
  const summary = (await created.json()).data.database;
  const dbId = summary.id;
  const rules = { exclude: ['**/Hocus Focus/**'], include: ['LIGHT'], lights_must_say_light: true,
    require_object: true, min_exposure_seconds: 60, match_by_position: false };
  expect(summary.scan_rules).toEqual(rules);
  await expect.poll(async () => (await (await request.get(`/api/db/${dbId}/import`)).json()).data.progress.stage).toBe('complete');
  expect(targetNames(summary.database_path)).toEqual(['M31']);
  const editor = page.getByRole('region', { name: 'Filtered first import import', exact: true });
  await expect(editor).toBeVisible();
  const savedRules = editor.getByText('Scan rules · Saved', { exact: true });
  await expect(savedRules.locator('..')).not.toHaveAttribute('open');
  await savedRules.click();
  await expect(editor.getByLabel('Exclude:', { exact: true })).toHaveValue('**/Hocus Focus/**');
  await expect(editor.getByLabel('Include only:', { exact: true })).toHaveValue('LIGHT');
  await expect(editor.getByRole('checkbox', { name: 'Lights need a target name' })).toBeChecked();
  await expect(editor.getByLabel('Minimum light exposure (s):')).toHaveValue('60');
  await editor.getByLabel('Exclude:', { exact: true }).fill('**/Other/**');
  const savedPromise = page.waitForResponse(response => response.url().endsWith(`/api/databases/${dbId}`) && response.request().method() === 'PUT');
  await editor.getByRole('button', { name: 'Save import settings' }).click();
  const saved = await savedPromise;
  expect(saved.ok(), await saved.text()).toBe(true);
  expect(saved.request().postDataJSON()).toEqual({ scan_rules: { ...rules, exclude: ['**/Other/**'] } });
  await expect(editor.getByRole('button', { name: 'Save import settings' })).toBeDisabled();
  expect(targetNames(summary.database_path)).toEqual(['M31']);
  await expect(savedRules.locator('..')).not.toHaveAttribute('open');
  await expect(editor.getByRole('button', { name: 'Preview import' })).toBeEnabled();
  await editor.getByRole('button', { name: 'Preview import' }).click();
  const preview = page.locator('.import-progress-panel');
  await expect(preview.locator('.import-project-list')).toContainText('NEW Focus', { timeout: 30_000 });
  await expect(preview.getByRole('button', { name: 'Import 1 frame(s)', exact: true })).toBeVisible();
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
    await preview.evaluate(element => {
      const scroller = element.closest('.modal-body')!;
      scroller.scrollTop += element.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    });
    await expect(page.getByRole('button', { name: 'Close settings' })).toBeInViewport();
    await page.screenshot({ path: testInfo.outputPath(`manual-import-preview-${width}.png`), animations: 'disabled' });
  }
  expect(targetNames(summary.database_path)).toEqual(['M31']);
});
