import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'crypto';
import * as path from 'path';
import { applyRealSchema } from './fixtures/sync';
import { fixtureDbPath, fixtureImageDir, resetDatabases, tmpBase, waitForCacheReady } from './helpers';

// Askar107PHQ's Heart Mosaic as N.I.N.A. laid it out (RA in hours), stored
// out of grid order and with names that say nothing about the cell.
const PANELS = [
  { id: 1, name: 'Heart d', ra: 2.445_519_401_920_46, dec: 60.871_626_084_609_4, cell: 'r2c2' },
  { id: 2, name: 'Heart a', ra: 2.645_499_681_489_58, dec: 61.832_231_223_272_4, cell: 'r1c1' },
  { id: 3, name: 'Heart c', ra: 2.642_430_289_441_8, dec: 60.871_626_084_609_4, cell: 'r2c1' },
  { id: 4, name: 'Heart b', ra: 2.442_450_009_872_69, dec: 61.832_231_223_272_4, cell: 'r1c2' },
];

let dbId: string;

test.beforeEach(async ({ request }, testInfo) => {
  testInfo.setTimeout(60_000);
  await resetDatabases(request);
  const databasePath = path.join(tmpBase(), `mosaic-${randomUUID()}.sqlite`);
  // The suite's real FITS rows, one per panel.
  const source = new Database(fixtureDbPath(), { readonly: true });
  const sourceImages = source.prepare(
    'SELECT Id, acquireddate, metadata FROM acquiredimage ORDER BY Id'
  ).all() as Array<{ Id: number; acquireddate: number; metadata: string }>;
  source.close();
  const db = new Database(databasePath);
  try {
    applyRealSchema(db);
    db.exec(`
      INSERT INTO project (Id,profileId,name,state,priority,isMosaic,flatsHandling,guid)
        VALUES (1,'default','Heart',1,5,1,0,'mosaic-heart-project');
      INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid)
        VALUES (1,'default','B 60','B',100,30,1,0,'mosaic-template');
    `);
    const target = db.prepare(
      `INSERT INTO target (Id,name,active,ra,dec,epochcode,rotation,projectid,guid) VALUES (?, ?, 1, ?, ?, 2, 0, 1, ?)`
    );
    const plan = db.prepare(
      `INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid)
       VALUES (?, 'default', 60, 20, 1, 0, ?, 1, 1, ?)`
    );
    for (const panel of PANELS) {
      target.run(panel.id, panel.name, panel.ra, panel.dec, `mosaic-target-${panel.id}`);
      plan.run(panel.id, panel.id, `mosaic-plan-${panel.id}`);
    }
    const insert = db.prepare(`
      INSERT INTO acquiredimage
        (Id,projectId,targetId,exposureId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,guid)
      VALUES (?, 1, ?, ?, ?, 'B', 0, ?, NULL, 'default', ?)
    `);
    sourceImages.slice(0, PANELS.length).forEach((row, index) => {
      const panel = PANELS[index];
      insert.run(row.Id, panel.id, panel.id, row.acquireddate, row.metadata, `mosaic-image-${row.Id}`);
    });
  } finally {
    db.close();
  }
  const registered = await request.post('/api/databases', {
    data: { name: 'Mosaic rig', slug: `mosaic-${randomUUID()}`, db_path: databasePath, image_dirs: [fixtureImageDir()] },
  });
  expect(registered.ok(), await registered.text()).toBeTruthy();
  dbId = (await registered.json()).data.id;
  await waitForCacheReady(request, dbId);
});

test("a mosaic's panels open as one view in Images, Sequence and Stacks, and a link reopens it", async ({ page }) => {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1`);
  const switcher = page.getByRole('combobox', { name: 'Rig and target' });
  await expect(switcher.locator('option', { hasText: 'Heart mosaic (4 panels)' })).toHaveCount(1, { timeout: 30_000 });
  await switcher.selectOption({ label: 'Heart mosaic (4 panels)' });
  await expect(page).toHaveURL(/mosaic=1/);

  const headings = page.locator('.mosaic-panel-heading');
  await expect(headings).toHaveCount(4, { timeout: 30_000 });
  await expect(headings).toHaveText(['r1c1 · Heart a', 'r1c2 · Heart b', 'r2c1 · Heart c', 'r2c2 · Heart d']);

  // A reload, as a shared link would, opens the same view.
  await page.reload();
  await expect(page.locator('.mosaic-panel-heading')).toHaveCount(4, { timeout: 30_000 });
  await expect(page.getByRole('combobox', { name: 'Rig and target' })).toHaveValue(`${dbId}:1:mosaic`);

  await page.goto(`/#/sequence?db=${encodeURIComponent(dbId)}&project=1&mosaic=1`);
  const frames = page.getByRole('list', { name: 'Frames in capture order' });
  await expect(frames.getByRole('listitem')).toHaveCount(4, { timeout: 30_000 });
  await expect(frames.locator('.mosaic-sequence-panel')).toHaveCount(4);

  await page.goto(`/#/stacks?db=${encodeURIComponent(dbId)}&project=1&mosaic=1`);
  await expect(page.locator('.mosaic-stacks-cell')).toHaveCount(4, { timeout: 30_000 });
  await expect(page.locator('.mosaic-stacks-cell figcaption strong')).toHaveText(['r1c1', 'r1c2', 'r2c1', 'r2c2']);
});
