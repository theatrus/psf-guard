import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'crypto';
import * as path from 'path';
import { applyRealSchema } from './fixtures/sync';
import { fixtureDbPath, fixtureImageDir, resetDatabases, tmpBase, waitForCacheReady } from './helpers';

const SOURCE_TARGET = 'M 44 alternate';
const DESTINATION_TARGET = 'M 44';
const SOURCE_PROJECT = 'Original import';
const DESTINATION_PROJECT = 'M 44 collection';
const EMPTY_PROJECT = 'Empty collection';
const EMPTY_TARGET = 'Empty target';

interface CatalogImage {
  Id: number;
  projectId: number;
  targetId: number;
  exposureId: number;
  gradingStatus: number;
  rejectreason: string | null;
  metadata: string;
  guid: string;
}

let databasePath: string;
let dbId: string;

function readCatalog<T>(read: (db: InstanceType<typeof Database>) => T): T {
  const db = new Database(databasePath, { readonly: true, timeout: 30_000 });
  try {
    return read(db);
  } finally {
    db.close();
  }
}

function images(): CatalogImage[] {
  return readCatalog((db) => db.prepare('SELECT * FROM acquiredimage ORDER BY Id').all() as CatalogImage[]);
}

function catalogSnapshot() {
  return readCatalog((db) => ({
    projects: db.prepare('SELECT * FROM project ORDER BY Id').all(),
    targets: db.prepare('SELECT * FROM target ORDER BY Id').all(),
    plans: db.prepare('SELECT * FROM exposureplan ORDER BY Id').all(),
    images: db.prepare('SELECT * FROM acquiredimage ORDER BY Id').all(),
  }));
}

function expectImageEvidenceUnchanged(before: CatalogImage[], after: CatalogImage[]) {
  const evidence = (rows: CatalogImage[]) => rows.map((row) => ({
    id: row.Id,
    guid: row.guid,
    grade: row.gradingStatus,
    rejectreason: row.rejectreason,
    metadata: row.metadata,
  }));
  expect(evidence(after)).toEqual(evidence(before));
}

async function openMove(page: Page, imageIds: number[]): Promise<Locator> {
  await page.goto(`/#/grid?db=${encodeURIComponent(dbId)}&project=1&target=1`);
  await expect(page.locator('.image-card')).toHaveCount(3, { timeout: 30_000 });
  await page.locator(`[data-card-image-id="${imageIds[0]}"]`).click();
  for (const id of imageIds.slice(1)) {
    await page.locator(`[data-card-image-id="${id}"]`).click({ modifiers: ['ControlOrMeta'] });
  }
  await page.getByRole('button', { name: 'Move exposures', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Move exposures', exact: true });
  await expect(dialog).toBeVisible();
  return dialog;
}

async function previewMove(dialog: Locator) {
  await expect(dialog.getByRole('button', { name: 'Apply move', exact: true })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Preview move', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Apply move', exact: true })).toBeEnabled();
}

async function captureDialog(page: Page, dialog: Locator, testInfo: TestInfo, name: string) {
  const screenshot = testInfo.outputPath(`${name}.png`);
  await page.screenshot({ path: screenshot, fullPage: true });
  await testInfo.attach(name, { path: screenshot, contentType: 'image/png' });
  const layout = await dialog.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    return {
      left: bounds.left,
      right: bounds.right,
      top: bounds.top,
      bottom: bounds.bottom,
      width: window.innerWidth,
      height: window.innerHeight,
      scrollWidth: element.scrollWidth,
      clientWidth: element.clientWidth,
    };
  });
  expect(layout.left).toBeGreaterThanOrEqual(0);
  expect(layout.right).toBeLessThanOrEqual(layout.width + 1);
  expect(layout.top).toBeGreaterThanOrEqual(0);
  expect(layout.bottom).toBeLessThanOrEqual(layout.height + 1);
  expect(layout.scrollWidth).toBeLessThanOrEqual(layout.clientWidth + 1);
  const note = dialog.locator('.organization-note');
  await note.scrollIntoViewIfNeeded();
  const noteBounds = await note.boundingBox();
  const footerBounds = await dialog.locator('.dialog-footer').boundingBox();
  expect(noteBounds).not.toBeNull();
  expect(footerBounds).not.toBeNull();
  expect(noteBounds!.y + noteBounds!.height).toBeLessThanOrEqual(footerBounds!.y + 1);
}

test.beforeEach(async ({ request }, testInfo) => {
  testInfo.setTimeout(60_000);
  await resetDatabases(request);
  databasePath = path.join(tmpBase(), `organization-${randomUUID()}.sqlite`);

  // Keep the full Scheduler schema while reusing the suite's real FITS paths.
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
        VALUES (1,'default','${SOURCE_PROJECT}',1,5,0,0,'organization-source-project'),
               (2,'default','${DESTINATION_PROJECT}',1,5,0,0,'organization-destination-project'),
               (3,'default','${EMPTY_PROJECT}',1,5,0,0,'organization-empty-project');
      INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid)
        VALUES (1,'${SOURCE_TARGET}',1,8.6738,19.66015,0,1,'organization-source-target'),
               (2,'${DESTINATION_TARGET}',1,8.6738,19.66015,0,2,'organization-destination-target'),
               (3,'${EMPTY_TARGET}',1,8.6738,19.66015,0,3,'organization-empty-target');
      INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid)
        VALUES (1,'default','B 60','B',100,30,1,0,'organization-template');
      INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid)
        VALUES (1,'default',60,20,3,1,1,1,1,'organization-source-plan'),
               (2,'default',60,20,1,1,2,1,1,'organization-destination-plan');
    `);
    const insert = db.prepare(`
      INSERT INTO acquiredimage
        (Id,projectId,targetId,exposureId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,guid)
      VALUES (?, ?, ?, ?, ?, 'B', ?, ?, ?, 'default', ?)
    `);
    for (const row of sourceImages) {
      const parentId = row.Id === 4 ? 2 : 1;
      const grade = row.Id === 3 ? 2 : row.Id === 1 ? 0 : 1;
      insert.run(
        row.Id, parentId, parentId, parentId, row.acquireddate, grade,
        row.metadata, grade === 2 ? 'Tracking error' : null, `organization-image-${row.Id}`
      );
    }
  } finally {
    db.close();
  }
  const registered = await request.post('/api/databases', {
    data: {
      name: 'Organization test rig',
      slug: `organization-${randomUUID()}`,
      db_path: databasePath,
      image_dirs: [fixtureImageDir()],
    },
  });
  expect(registered.ok(), await registered.text()).toBeTruthy();
  dbId = (await registered.json()).data.id;
  await waitForCacheReady(request, dbId);
});

test('canceling a target merge preview leaves the catalog unchanged', async ({ page }) => {
  const before = catalogSnapshot();
  await page.goto('/');
  await page.getByRole('button', { name: `Merge target ${SOURCE_TARGET}`, exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Merge targets', exact: true });
  await dialog.getByRole('combobox', { name: 'Destination project', exact: true }).selectOption({ label: DESTINATION_PROJECT });
  await dialog.getByRole('combobox', { name: 'Destination target', exact: true }).selectOption({ label: DESTINATION_TARGET });
  await expect(dialog.getByRole('button', { name: 'Apply merge', exact: true })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Preview merge', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Apply merge', exact: true })).toBeEnabled();
  expect(catalogSnapshot()).toEqual(before);
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(catalogSnapshot()).toEqual(before);
});

test('Overview merges a target into another project after preview', async ({ page }, testInfo) => {
  const before = images();
  await page.goto('/');
  await page.getByRole('button', { name: `Merge target ${SOURCE_TARGET}`, exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Merge targets', exact: true });
  await dialog.getByRole('combobox', { name: 'Destination project', exact: true }).selectOption({ label: DESTINATION_PROJECT });
  await dialog.getByRole('combobox', { name: 'Destination target', exact: true }).selectOption({ label: DESTINATION_TARGET });
  await dialog.getByRole('button', { name: 'Preview merge', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Apply merge', exact: true })).toBeEnabled();
  await captureDialog(page, dialog, testInfo, 'merge-target-preview');
  await dialog.getByRole('button', { name: 'Apply merge', exact: true }).click();
  await expect(dialog).toBeHidden();
  const after = images();
  expect(after.map((row) => [row.projectId, row.targetId])).toEqual([[2, 2], [2, 2], [2, 2], [2, 2]]);
  expectImageEvidenceUnchanged(before, after);
  expect(readCatalog((db) => db.prepare('SELECT Id FROM target WHERE Id = 1').get())).toBeUndefined();
  expect(readCatalog((db) => db.prepare('SELECT targetid, guid FROM exposureplan WHERE Id = 1').get()))
    .toEqual({ targetid: 2, guid: 'organization-source-plan' });
});

test('selected exposures move to an existing target and refresh the destination grid', async ({ page }) => {
  const before = images();
  const dialog = await openMove(page, [2, 3]);
  await dialog.getByRole('combobox', { name: 'Destination project', exact: true }).selectOption({ label: DESTINATION_PROJECT });
  await dialog.getByRole('combobox', { name: 'Destination target', exact: true }).selectOption({ label: DESTINATION_TARGET });
  await previewMove(dialog);
  await dialog.getByRole('button', { name: 'Apply move', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page).toHaveURL(new RegExp(`db=${dbId}.*project=2.*target=2`));
  await expect(page.locator('.image-card')).toHaveCount(3);
  const after = images();
  expect(after.map((row) => [row.projectId, row.targetId])).toEqual([[1, 1], [2, 2], [2, 2], [2, 2]]);
  expectImageEvidenceUnchanged(before, after);
  expect(readCatalog((db) => db.prepare('SELECT acquired, accepted FROM exposureplan WHERE Id = 1').get()))
    .toEqual({ acquired: 1, accepted: 0 });
  expect(readCatalog((db) => db.prepare('SELECT acquired, accepted FROM exposureplan WHERE Id = 2').get()))
    .toEqual({ acquired: 1, accepted: 1 });
  const movedPlan = after[1].exposureId;
  expect(movedPlan).not.toBe(1);
  expect(after[2].exposureId).toBe(movedPlan);
  expect(readCatalog((db) => db.prepare('SELECT targetid, desired, acquired, accepted, enabled FROM exposureplan WHERE Id = ?').get(movedPlan)))
    .toEqual({ targetid: 2, desired: 0, acquired: 2, accepted: 1, enabled: 0 });
});

test('selected exposures split into a new target in the current project', async ({ page }) => {
  const before = images();
  const dialog = await openMove(page, [1]);
  await dialog.getByRole('combobox', { name: 'Destination target', exact: true }).selectOption('new');
  await dialog.getByLabel('New target name', { exact: true }).fill('M 44 east panel');
  await previewMove(dialog);
  await dialog.getByRole('button', { name: 'Apply move', exact: true }).click();
  await expect(dialog).toBeHidden();
  const target = readCatalog((db) => db.prepare('SELECT Id, projectid, active FROM target WHERE name = ?').get('M 44 east panel')) as { Id: number; projectid: number; active: number };
  expect(target).toMatchObject({ projectid: 1, active: 0 });
  await expect(page).toHaveURL(new RegExp(`db=${dbId}.*project=1.*target=${target.Id}`));
  await expect(page.locator('.image-card')).toHaveCount(1);
  const after = images();
  expect(after[0]).toMatchObject({ projectId: 1, targetId: target.Id });
  expect(after[1]).toMatchObject({ projectId: 1, targetId: 1 });
  expect(after[2]).toMatchObject({ projectId: 1, targetId: 1 });
  expectImageEvidenceUnchanged(before, after);
});

test('selected exposures can populate an existing empty project and target', async ({ page }) => {
  const before = images();
  const dialog = await openMove(page, [1]);
  await dialog.getByRole('combobox', { name: 'Destination project', exact: true }).selectOption({ label: EMPTY_PROJECT });
  await dialog.getByRole('combobox', { name: 'Destination target', exact: true }).selectOption({ label: EMPTY_TARGET });
  await previewMove(dialog);
  await dialog.getByRole('button', { name: 'Apply move', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page).toHaveURL(new RegExp(`db=${dbId}.*project=3.*target=3`));
  await expect(page.locator('.image-card')).toHaveCount(1);
  const after = images();
  expect(after.map((row) => [row.projectId, row.targetId])).toEqual([[3, 3], [1, 1], [1, 1], [2, 2]]);
  expectImageEvidenceUnchanged(before, after);
  expect(readCatalog((db) => db.prepare('SELECT guid FROM target WHERE Id = 3').get()))
    .toEqual({ guid: 'organization-empty-target' });
  expect(readCatalog((db) => db.prepare('SELECT targetid, desired, acquired, accepted, enabled FROM exposureplan WHERE Id = ?').get(after[0].exposureId)))
    .toEqual({ targetid: 3, desired: 0, acquired: 1, accepted: 0, enabled: 0 });
});

test('selected exposures split into a new project and target on a phone', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const before = images();
  const dialog = await openMove(page, [3]);
  await dialog.getByRole('combobox', { name: 'Destination project', exact: true }).selectOption('new');
  await dialog.getByLabel('New project name', { exact: true }).fill('M 44 mosaic follow-up');
  await dialog.getByLabel('New target name', { exact: true }).fill('M 44 western field');
  await previewMove(dialog);
  await captureDialog(page, dialog, testInfo, 'move-exposures-phone-preview');
  await dialog.getByRole('button', { name: 'Apply move', exact: true }).click();
  await expect(dialog).toBeHidden();
  const project = readCatalog((db) => db.prepare('SELECT Id, state, profileId FROM project WHERE name = ?').get('M 44 mosaic follow-up')) as { Id: number; state: number; profileId: string };
  const target = readCatalog((db) => db.prepare('SELECT Id, projectid, active FROM target WHERE name = ?').get('M 44 western field')) as { Id: number; projectid: number; active: number };
  expect(project).toMatchObject({ state: 2, profileId: 'default' });
  expect(target).toMatchObject({ projectid: project.Id, active: 0 });
  await expect(page).toHaveURL(new RegExp(`db=${dbId}.*project=${project.Id}.*target=${target.Id}`));
  await expect(page.locator('.image-card')).toHaveCount(1);
  const after = images();
  expect(after[2]).toMatchObject({ projectId: project.Id, targetId: target.Id });
  expect(after[0]).toMatchObject({ projectId: 1, targetId: 1 });
  expect(after[1]).toMatchObject({ projectId: 1, targetId: 1 });
  expectImageEvidenceUnchanged(before, after);
});
