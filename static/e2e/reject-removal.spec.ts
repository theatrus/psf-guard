import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import { applyRealSchema } from './fixtures/sync';
import { tmpBase } from './helpers';

// A database of its own with two small files, so removal never touches the
// suite's shared FITS fixtures.
test('old rejects are removed to the trash from Settings and restored', async ({ page, request }, testInfo) => {
  testInfo.setTimeout(60_000);
  const root = path.join(tmpBase(), `reject-removal-${randomUUID()}`);
  const light = path.join(root, 'images', 'M42', '2026-10-01', 'LIGHT');
  fs.mkdirSync(light, { recursive: true });
  fs.writeFileSync(path.join(light, 'M42_Ha_001.fits'), Buffer.alloc(2 * 1024 * 1024, 1));
  fs.writeFileSync(path.join(light, 'M42_Ha_002.fits'), Buffer.alloc(2 * 1024 * 1024, 2));
  const databasePath = path.join(root, 'rig.sqlite');
  const db = new Database(databasePath);
  try {
    applyRealSchema(db);
    db.exec(`
      INSERT INTO project (Id,profileId,name,state,priority,isMosaic,flatsHandling,guid) VALUES (1,'p','M42',1,1,0,0,'removal-project');
      INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES (1,'M42',1,5.588,-5.39,2,1,'removal-target');
      INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,guid) VALUES
        (1,1,1,1759300000,'Ha',2,'{"FileName":"M42_Ha_001.fits"}','Clouds','p','removal-bad'),
        (2,1,1,1759300300,'Ha',1,'{"FileName":"M42_Ha_002.fits"}',NULL,'p','removal-good');
    `);
  } finally {
    db.close();
  }
  const name = `Removal rig ${randomUUID().slice(0, 6)}`;
  const registered = await request.post('/api/databases', {
    data: { name, slug: `removal-${randomUUID()}`, db_path: databasePath, image_dirs: [path.join(root, 'images')] },
  });
  expect(registered.ok(), await registered.text()).toBeTruthy();
  const dbId = (await registered.json()).data.id;
  const count = (sql: string) => {
    const reader = new Database(databasePath, { readonly: true });
    try { return (reader.prepare(sql).get() as { n: number }).n; } finally { reader.close(); }
  };
  try {
    await page.goto('/');
    const alreadyOpen = await page.getByRole('tab', { name: 'Databases' }).waitFor({ state: 'visible', timeout: 2000 }).then(() => true, () => false);
    if (!alreadyOpen) await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByRole('tab', { name: 'Databases' }).click();
    const group = page.locator('.db-row', { hasText: name }).getByRole('group', { name: 'Remove rejects' });
    // Its first sight dates the reject now, so only "0 days" takes it.
    await group.getByLabel('Days rejected').fill('0');
    await group.getByRole('button', { name: 'Preview' }).click();
    await expect(group.getByTestId('reject-removal-plan')).toContainText('1 reject to remove (2.0 MB).');
    await group.getByRole('button', { name: 'Remove 1 reject' }).click();
    await expect(group.getByRole('status')).toContainText('Removed 1 reject; their files wait in the trash until');
    expect(fs.existsSync(path.join(light, 'M42_Ha_001.fits'))).toBe(false);
    expect(fs.existsSync(path.join(light, 'M42_Ha_002.fits'))).toBe(true);
    expect(count('SELECT COUNT(*) AS n FROM acquiredimage')).toBe(1);
    const batches = group.getByRole('list', { name: 'Removed rejects' });
    await expect(batches).toContainText('1 reject · 2.0 MB · in the trash until');
    await group.scrollIntoViewIfNeeded();
    await group.screenshot({ path: testInfo.outputPath('reject-removal.png') });

    await batches.getByRole('button', { name: /^Restore the rejects removed/ }).click();
    await expect(group.getByRole('status')).toContainText('Restored 1 reject.');
    expect(fs.existsSync(path.join(light, 'M42_Ha_001.fits'))).toBe(true);
    expect(count("SELECT COUNT(*) AS n FROM acquiredimage WHERE guid = 'removal-bad' AND gradingStatus = 2")).toBe(1);
    await expect(group.getByRole('list', { name: 'Removed rejects' })).toHaveCount(0);
  } finally {
    await request.delete(`/api/databases/${encodeURIComponent(dbId)}`);
    fs.rmSync(root, { recursive: true, force: true });
  }
});
