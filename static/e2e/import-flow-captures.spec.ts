import { expect, test, type Locator } from '@playwright/test';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import * as path from 'node:path';
import Database from 'better-sqlite3';
import { applyRealSchema } from './fixtures/sync';
import { fixtureDbPath, fixtureImageDir, resetDatabases, tmpBase, selectSettingsSection } from './helpers';

// Opt in to reshoot the import guide. The normal suite covers these paths in
// add-database.spec.ts; this run also saves each image's state and checksum.
const output = process.env.PSF_GUARD_CAPTURE_DIR;
test.skip(!output, 'Set PSF_GUARD_CAPTURE_DIR to reshoot the import guide');
test.use({
  viewport: { width: 1440, height: 1200 }, deviceScaleFactor: 2,
  reducedMotion: 'reduce', colorScheme: 'dark', locale: 'en-US',
  timezoneId: 'America/Los_Angeles',
});

test('capture the owned catalog import and sync flow', async ({ page, request, browser }) => {
  test.setTimeout(120_000);
  await resetDatabases(request);
  mkdirSync(output!, { recursive: true });
  const source = path.join(tmpBase(), 'TargetScheduler.sqlite');
  const db = new Database(source);
  try {
    applyRealSchema(db);
    db.prepare('ATTACH DATABASE ? AS fixture').run(fixtureDbPath());
    db.exec(`INSERT INTO project(Id,profileId,name,description,guid)
      SELECT Id,profileId,name,description,'project-'||Id FROM fixture.project;
      INSERT INTO target(Id,projectid,name,active,ra,dec,epochcode,guid)
      SELECT Id,projectId,name,active,ra,dec,0,'target-'||Id FROM fixture.target;
      INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,guid)
      SELECT Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,'image-'||Id FROM fixture.acquiredimage;`);
  } finally { db.close(); }
  const info = (await (await request.get('/api/info')).json()).data;
  const captures: object[] = [];
  const capture = async (name: string, element: Locator, state: string, endAt?: Locator) => {
    if (endAt) await endAt.evaluate(element => element.scrollIntoView({ block: 'center' }));
    else await element.scrollIntoViewIfNeeded();
    const crop = (await element.boundingBox())!;
    if (endAt) {
      const end = (await endAt.boundingBox())!;
      crop.height = end.y + end.height - crop.y;
    }
    const file = path.join(output!, name);
    if (endAt) await page.screenshot({ path: file, clip: crop, animations: 'disabled' });
    else await element.screenshot({ path: file, animations: 'disabled' });
    const png = readFileSync(file);
    const catalogs = (await (await request.get('/api/databases')).json()).data.map(
      (catalog: { id: string; name: string; database_path: string }) => ({
        id: catalog.id, name: catalog.name, databasePath: catalog.database_path,
      }),
    );
    captures.push({ name, catalogs, time: new Date().toISOString(), url: page.url(), state, crop,
      pixels: [png.readUInt32BE(16), png.readUInt32BE(20)],
      sha256: createHash('sha256').update(png).digest('hex') });
    writeFileSync(path.join(output!, 'import-flow-captures.json'), JSON.stringify({
      commit: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
      version: info.version, browser: browser.browserType().name(), browserVersion: browser.version(),
      viewport: page.viewportSize(), deviceScaleFactor: 2, theme: 'dark', locale: 'en-US',
      timezone: 'America/Los_Angeles', source, imageDirectory: fixtureImageDir(),
      fixture: 'global-setup.ts plus applyRealSchema; two projects, four images, stable GUIDs',
      captures,
    }, null, 2) + '\n');
  };
  await page.goto('/');
  const modal = page.locator('.tauri-settings');
  await expect(modal.getByRole('heading', { name: /Welcome to PSF Guard/ })).toBeVisible();
  await capture('import-first-run.png', modal, 'Empty registry. Settings > Databases auto-open; neither starting point selected. Release notes acknowledged by Playwright storage state. Crop ends after welcome choices.', modal.locator('.welcome-message'));
  await modal.getByRole('button', { name: /Import from Target Scheduler/ }).click();
  const form = modal.locator('.catalog-create-form');
  await form.getByLabel('Display name (optional):').fill('Observatory');
  await form.getByPlaceholder('Select or enter database path').fill(source);
  await form.getByPlaceholder('Type an absolute path and press Add').fill(fixtureImageDir());
  await form.getByRole('button', { name: 'Add', exact: true }).click();
  await capture('import-local-ts.png', form, 'Local TS file selected; Observatory name; source and images entered; managed storage; keep source connected on; import not started.');

  await form.getByLabel('Keep the source connected for Target Scheduler sync').uncheck();
  await form.getByRole('checkbox', { name: 'Use a custom database path (advanced)' }).check();
  await form.getByLabel('Destination database path:').fill(path.join(tmpBase(), 'restored.sqlite'));
  await capture('import-catalog-copy.png', form, 'Standalone copy selected; custom destination restored.sqlite; no sync or source writeback; not submitted.');
  await form.getByLabel('Keep the source connected for Target Scheduler sync').check();
  await form.getByRole('checkbox', { name: 'Use a custom database path (advanced)' }).uncheck();
  await form.getByRole('button', { name: 'Import from Target Scheduler', exact: true }).click();
  await expect(modal.getByText('Target Scheduler import complete. Automatic sync and grade writeback are enabled.')).toBeVisible();
  const settings = modal.getByRole('region', { name: 'Target Scheduler sync', exact: true });
  await expect(settings.getByRole('checkbox', { name: 'Sync when new images or grades arrive' })).toBeChecked();
  await capture('import-automatic-sync.png', settings, 'Import complete. Own catalog created. Defaults saved: startup on, change detection on, interval 5 minutes, automatic grade writeback on.');

  await settings.getByRole('checkbox', { name: 'Sync when the desktop app or server starts' }).uncheck();
  await settings.getByRole('checkbox', { name: 'Sync when new images or grades arrive' }).uncheck();
  await settings.getByRole('spinbutton').fill('0');
  await settings.getByRole('checkbox', { name: 'Automatically write grade changes back to Target Scheduler' }).uncheck();
  await settings.getByRole('button', { name: 'Save sync settings' }).click();
  await expect(settings.getByRole('button', { name: 'Save sync settings' })).toBeDisabled();
  const changed = new Database(source);
  try { changed.exec(`INSERT INTO project(Id,profileId,name,description,guid)
    VALUES(100,'default','Next night','New planning in Target Scheduler','project-next-night');
    INSERT INTO target(Id,projectid,name,active,ra,dec,epochcode,guid)
    VALUES(100,100,'Next night field',1,5.5,-5.4,0,'target-next-night');`); }
  finally { changed.close(); }
  await selectSettingsSection(modal, 'sync');
  const transfer = modal.locator('.scheduler-sync-workspace');
  await transfer.getByRole('button', { name: 'Preview changes' }).click();
  await expect(transfer.getByRole('button', { name: 'Apply this preview' })).toBeVisible();
  await capture('import-reviewed-transfer.png', transfer, 'Automatic sync and writeback saved off. Merge catalogs: Observatory — Target Scheduler to Observatory. Include thumbnails on. Preview of new project 100 and target 100; Apply not clicked.');
  await transfer.getByRole('button', { name: 'Apply this preview' }).click();
  await expect(transfer.getByRole('button', { name: 'Apply this preview' })).toBeHidden();

  await modal.getByRole('button', { name: 'Close settings', exact: true }).click();
  await resetDatabases(request);
  await page.goto('/');
  await modal.getByRole('button', { name: /New Database from Images/ }).click();
  await form.getByLabel('Display name (optional):').fill('Image archive');
  await form.getByPlaceholder('Type an absolute path and press Add').fill(fixtureImageDir());
  await form.getByRole('button', { name: 'Add', exact: true }).click();
  await capture('import-image-folders.png', form, 'Image folders selected; Image archive name; real FITS fixture directory added; managed storage; background quality analysis off; not submitted.');
  await form.getByRole('radio', { name: 'Use the PSF Guard Sync plugin' }).check();
  await form.getByLabel('Display name (optional):').fill('Remote observatory');
  await capture('import-sync-plugin.png', form, 'Sync plugin selected; Remote observatory name; shared fixture image folder retained when switching; no local TS file; managed storage; catalog not yet created.');
  await form.getByRole('button', { name: 'Create catalog for plugin' }).click();
  const pairing = modal.locator('.database-config').filter({ has: page.getByRole('button', { name: 'Generate pairing code' }) });
  const pairingRow = pairing.locator('.remote-upload-token-row').first();
  await capture('import-plugin-pairing.png', pairing, 'Remote observatory catalog created. Pairing controls open automatically; no code issued, no client paired, no uploads enabled. Crop ends after Generate pairing code.', pairingRow);
  await pairing.getByRole('button', { name: 'Generate pairing code' }).click();
  await expect(pairing.getByText(/Paste this code into the client/)).toBeVisible();
});
