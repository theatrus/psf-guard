import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

test('database rigs contribute to one project and reuse its source editor', async ({ page, request }, testInfo) => {
  test.setTimeout(60_000);
  const slugs: string[] = [];
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    for (const [index, name] of ['C925 data', 'Redcat data'].entries()) {
      const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'catalog-'));
      const dbPath = path.join(root, 'catalog.sqlite');
      const db = new Database(dbPath);
      applyRealSchema(db);
      const insert = db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(?,?,?,'',1,1,0,0,?)");
      insert.run(1, 'current-profile', 'Andromeda exposures', randomUUID());
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

    let projectId = '';
    for (const [index, slug] of slugs.entries()) {
      await page.goto(`/#/director?directorView=catalogs&directorCatalog=${slug}&db=parked-catalog`);
      await page.getByRole('button', { name: 'Preview rig setup' }).click();
      await page.getByRole('button', { name: 'Enable planning' }).click();
      await expect(page.getByRole('checkbox', { name: 'Select Andromeda exposures (1)' })).toBeEnabled();
      await expect(page.getByRole('button', { name: /New rig/ })).toHaveCount(0);
      if (index === 0) {
        await expect(page.getByRole('checkbox', { name: 'Select Legacy project without GUID (3)' })).toBeDisabled();
        await page.getByRole('button', { name: 'New project for Andromeda exposures' }).click();
        await page.getByLabel('New project name').fill('Andromeda multi-rig campaign');
        await page.getByRole('button', { name: 'Create', exact: true }).click();
        await expect(page.getByLabel('New project name')).toHaveCount(0);
        projectId = await page.getByLabel('Project for Andromeda exposures (1)').inputValue();
        await page.getByLabel('Project for Andromeda older setup (2)').selectOption(projectId);
        await page.getByRole('checkbox', { name: 'Select Andromeda older setup (2)' }).check();
      } else await page.getByLabel('Project for Andromeda exposures (1)').selectOption(projectId);
      await page.getByRole('checkbox', { name: 'Select Andromeda exposures (1)' }).check();
      await page.getByRole('button', { name: 'Preview mappings' }).click();
      await expect(page.getByRole('heading', { name: `Review ${index === 0 ? 2 : 1} mappings` })).toBeVisible();
      if (index === 0) {
        const changed = await request.patch(`/api/director/v1/projects/${projectId}`, { data: { expected_revision: 1, name: 'Andromeda campaign revised' } });
        expect(changed.ok()).toBeTruthy();
        await page.getByRole('button', { name: 'Apply mappings' }).click();
        await expect(page.getByRole('alert')).toContainText('conflicts');
        await page.getByRole('button', { name: 'Preview mappings' }).click();
        await expect(page.getByRole('region', { name: 'Mapping review' }).getByText('Andromeda campaign revised')).toHaveCount(2);
      }
      await page.getByRole('button', { name: 'Apply mappings' }).click();
      await expect(page.getByText('Mappings saved.')).toBeVisible();
      await expect(page.getByText('Linked', { exact: true })).toHaveCount(index === 0 ? 2 : 1);
      await page.locator('.tauri-settings .close-button').click();
    }

    const first = (await (await request.get(`/api/director/v1/catalogs/${slugs[0]}/mappings`)).json()).data;
    const second = (await (await request.get(`/api/director/v1/catalogs/${slugs[1]}/mappings`)).json()).data;
    expect(first.items).toHaveLength(2);
    expect(first.items.every((item: { rig_id: string; project_id: string }) => item.rig_id === first.rig.id && item.project_id === projectId)).toBeTruthy();
    expect(second.items[0].project_id).toBe(projectId);
    expect(second.items[0].rig_id).toBe(second.rig.id);
    expect(first.rig.id).not.toBe(second.rig.id);
    await page.goto('/#/director?directorView=rigs');
    await expect(page.getByText('C925 data', { exact: true })).toBeVisible();
    await expect(page.getByText('Redcat data', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'New rig' })).toHaveCount(0);
    await page.getByRole('button', { name: 'Configure C925 data' }).click();
    await expect(page.getByText('Linked', { exact: true })).toHaveCount(2);
    await page.setViewportSize({ width: 375, height: 812 });
    await page.getByRole('region', { name: 'Project planning links' }).getByRole('heading', { name: 'Project planning links' }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath('database-planning-links-mobile.png'), fullPage: true });
    expect(await page.locator('.director-database-links').evaluate(element => element.scrollWidth <= element.clientWidth)).toBeTruthy();
    await page.locator('.tauri-settings .close-button').click();
    await page.goto(`/#/?db=${slugs[0]}&project=1&dbfilter=${slugs[0]}`);
    const card = page.locator(`[data-project-key="${slugs[0]}:1"]`);
    await card.getByRole('button', { name: 'Plan & coordinates' }).click();
    await expect(page.getByLabel('RA (decimal hours)')).toHaveValue('0.712313');
    await page.getByRole('button', { name: 'Rig planning' }).click();
    await expect(page.getByRole('region', { name: 'Project acquisition planning' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Andromeda campaign revised' })).toBeVisible();
    await expect(page.getByLabel('RA (decimal hours)')).toHaveValue('0.712313');
    await expect(page.getByRole('button', { name: 'Catalogs', exact: true })).toHaveCount(0);
    await expect(page.getByLabel('Ha desired count')).toHaveValue('40');
    await expect(page.getByLabel('Ha exposure seconds')).toHaveValue('300');
    await page.getByRole('heading', { name: 'Andromeda campaign revised' }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath('project-planning-mobile.png'), fullPage: true });
    expect(await page.locator('.director-page').evaluate(element => element.scrollWidth <= element.clientWidth)).toBeTruthy();
    await page.reload();
    await expect(page.getByLabel('RA (decimal hours)')).toHaveValue('0.712313');
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.screenshot({ path: testInfo.outputPath('project-planning-desktop.png'), fullPage: true });
    await page.getByRole('region', { name: 'Project acquisition planning' }).getByRole('link', { name: 'Overview' }).click();
    await expect(card).toHaveAttribute('data-current-project', 'true');
    expect(new URL(page.url().split('#')[1], 'http://test').searchParams.get('dbfilter')).toBe(slugs[0]);
    expect(errors).toEqual([]);
  } finally {
    for (const slug of slugs) {
      const removed = await request.delete(`/api/databases/${slug}`);
      expect(removed.ok(), await removed.text()).toBeTruthy();
    }
  }
});
