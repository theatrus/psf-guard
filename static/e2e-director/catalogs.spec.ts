import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

test('every database project is a plan, shared GUIDs make one plan across rigs, and the workspace opens from Overview', async ({ page, request }, testInfo) => {
  test.setTimeout(60_000);
  const slugs: string[] = [];
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const shared = randomUUID();
  try {
    for (const [index, name] of ['C925 data', 'Redcat data'].entries()) {
      const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'catalog-'));
      const dbPath = path.join(root, 'catalog.sqlite');
      const db = new Database(dbPath);
      applyRealSchema(db);
      const insert = db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(?,?,?,'',1,1,0,0,?)");
      // The same GUID in both databases, the way Sync copies a project.
      insert.run(1, 'current-profile', 'Andromeda exposures', shared);
      if (index === 0) {
        insert.run(2, 'previous-profile', 'Andromeda older setup', randomUUID());
        insert.run(3, 'current-profile', 'Legacy project without GUID', null);
      }
      db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES(1,?,1,0.712313,41.2687,2,1,?)').run('Andromeda target', randomUUID());
      db.prepare('INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid) VALUES(1,?,?,?,100,30,1,0,?)').run('current-profile', 'Ha', 'Ha', randomUUID());
      db.prepare('INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(1,?,?,40,1,1,1,1,1,?)').run('current-profile', index === 0 ? 300 : 60, randomUUID());
      db.prepare('INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid) VALUES(1,1,1,1750000000,?,1,?,?,1,?)').run('Ha', JSON.stringify({ FileName: 'not-copied-yet.fits' }), 'current-profile', randomUUID());
      db.close();
      const slug = `director-${randomUUID().slice(0, 8)}`;
      const added = await request.post('/api/databases', { data: { name, slug, db_path: dbPath, image_dirs: [root] } });
      expect(added.ok(), await added.text()).toBeTruthy();
      slugs.push(slug);
    }

    // Opening the page adopts both databases and lists their projects as plans.
    await page.goto('/#/director?db=parked-catalog');
    const andromeda = page.locator('.director-plan', { hasText: 'Andromeda exposures' });
    await expect(andromeda).toHaveCount(1);
    await expect(andromeda.getByText('C925 data: Andromeda exposures')).toBeVisible();
    await expect(andromeda.getByText('Redcat data: Andromeda exposures')).toBeVisible();
    await expect(page.locator('.director-plan', { hasText: 'Andromeda older setup' })).toHaveCount(1);
    await expect(page.getByText('Legacy project without GUID')).toHaveCount(0);
    await expect(page.getByText('C925 data', { exact: true })).toBeVisible();
    await expect(page.getByText('Redcat data', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'New rig' })).toHaveCount(0);
    const mappings = await Promise.all(slugs.map(async slug => (await (await request.get(`/api/director/v1/catalogs/${slug}/mappings`)).json()).data));
    expect(mappings[0].items).toHaveLength(2);
    expect(mappings[1].items).toHaveLength(1);
    expect(mappings[0].items.find((item: { source_project_guid: string }) => item.source_project_guid === shared).project_id).toBe(mappings[1].items[0].project_id);
    expect(mappings[0].rig.id).not.toBe(mappings[1].rig.id);

    // Setup expands the rig profile in place.
    await page.getByRole('button', { name: 'Setup C925 data' }).click();
    await expect(page.getByRole('region', { name: 'Rig profile' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Save rig profile' })).toBeVisible();
    await page.getByRole('button', { name: 'Setup C925 data' }).click();

    // The workspace shows both databases and each database's own editor.
    await andromeda.getByRole('link', { name: 'Open Andromeda exposures' }).click();
    await expect(page.getByRole('heading', { name: 'Andromeda exposures' })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Linked databases' }).getByText('C925 data')).toBeVisible();
    await expect(page.getByRole('region', { name: 'Linked databases' }).getByText('Redcat data')).toBeVisible();
    await page.getByRole('button', { name: /Targets and exposures/ }).first().click();
    await expect(page.getByLabel('RA (decimal hours)')).toHaveValue('0.712313');
    await expect(page.getByLabel('Ha desired count')).toHaveValue('40');
    await expect(page.getByRole('region', { name: 'Acquisition plan' })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Activation' })).toBeVisible();
    await page.setViewportSize({ width: 375, height: 812 });
    await page.screenshot({ path: testInfo.outputPath('project-workspace-mobile.png'), fullPage: true });
    expect(await page.locator('.director-page').evaluate(element => element.scrollWidth <= element.clientWidth)).toBeTruthy();
    await page.setViewportSize({ width: 1440, height: 1000 });

    // Overview's Rig planning lands in the same workspace.
    await page.goto(`/#/?db=${slugs[0]}&project=1&dbfilter=${slugs[0]}`);
    const card = page.locator(`[data-project-key="${slugs[0]}:1"]`);
    await card.getByRole('button', { name: 'Plan & coordinates' }).click();
    await page.getByRole('button', { name: 'Rig planning' }).click();
    await expect(page.getByRole('heading', { name: 'Andromeda exposures' })).toBeVisible();
    expect(new URL(page.url().split('#')[1], 'http://test').searchParams.get('directorProject')).toBe(mappings[1].items[0].project_id);
    await page.getByRole('link', { name: 'Plans' }).click();
    await expect(page.getByRole('heading', { name: 'Plans' })).toBeVisible();
    expect(errors).toEqual([]);
  } finally {
    for (const slug of slugs) await request.delete(`/api/databases/${slug}`);
  }
});
