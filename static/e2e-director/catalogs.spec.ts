import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

test('catalog mappings require review and survive reload across two rig profiles', async ({ page, request }, testInfo) => {
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'catalog-'));
  const dbPath = path.join(root, 'catalog.sqlite');
  const db = new Database(dbPath);
  applyRealSchema(db);
  const insert = db.prepare('INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(?,?,?,\'\',1,5,0,0,?)');
  insert.run(1, 'remote-c925-profile', 'Andromeda long exposures', randomUUID());
  insert.run(2, 'remote-redcat-profile', 'Andromeda short exposures', randomUUID());
  insert.run(3, 'remote-c925-profile', 'Legacy project without GUID', null);
  db.close();
  const slug = `director-${randomUUID().slice(0, 8)}`;
  const added = await request.post('/api/databases', { data: { name: 'Observatory catalogs', slug, db_path: dbPath, image_dirs: [root] } });
  expect(added.ok(), await added.text()).toBeTruthy();
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await page.goto(`/#/director?directorView=catalogs&directorCatalog=${slug}&db=parked-catalog`);
    const long = 'Andromeda long exposures';
    const short = 'Andromeda short exposures';
    await expect(page.getByRole('checkbox', { name: `Select ${long} (1)` })).toBeEnabled();
    await expect(page.getByRole('checkbox', { name: 'Select Legacy project without GUID (3)' })).toBeDisabled();
    await page.getByRole('button', { name: `New global project for ${long}` }).click();
    await page.getByLabel('New global project name').fill('Andromeda multi-rig campaign');
    await page.getByRole('button', { name: 'Create', exact: true }).click();
    await expect(page.getByLabel('New global project name')).toHaveCount(0);
    const projectId = await page.getByLabel(`Global project for ${long} (1)`).inputValue();
    await page.getByLabel(`Global project for ${short} (2)`).selectOption(projectId);
    for (const [source, name] of [[long, 'C925'], [short, 'Redcat 61']]) {
      await page.getByRole('button', { name: `New rig for ${source}` }).click();
      await page.getByLabel('New rig name').fill(name);
      await page.getByRole('button', { name: 'Create', exact: true }).click();
      await expect(page.getByLabel('New rig name')).toHaveCount(0);
    }
    await page.getByRole('checkbox', { name: `Select ${long} (1)` }).check();
    await page.getByRole('checkbox', { name: `Select ${short} (2)` }).check();
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.screenshot({ path: testInfo.outputPath('director-catalog-choices.png'), fullPage: true });
    const rigId = await page.getByLabel(`Rig for ${long} (1)`).inputValue();
    await page.getByRole('button', { name: 'Preview mappings' }).click();
    await expect(page.getByRole('heading', { name: 'Review 2 mappings' })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('director-catalog-review.png'), fullPage: true });
    const changed = await request.patch(`/api/director/v1/rigs/${rigId}`, { data: { expected_revision: 1, name: 'C925 revised' } });
    expect(changed.ok()).toBeTruthy();
    await page.getByRole('button', { name: 'Apply mappings' }).click();
    await expect(page.getByRole('alert')).toContainText('conflicts');
    const inspect = new Database(dbPath, { readonly: true });
    expect(inspect.prepare("SELECT COUNT(*) n FROM sqlite_master WHERE name='psf_guard_catalog_identity'").get()).toEqual({ n: 0 });
    inspect.close();
    await page.getByRole('button', { name: 'Preview mappings' }).click();
    await expect(page.getByRole('region', { name: 'Mapping review' }).getByText('C925 revised')).toBeVisible();
    await page.getByRole('button', { name: 'Apply mappings' }).click();
    await expect(page.getByText('Mappings saved.')).toBeVisible();
    await expect(page.getByText('Linked', { exact: true })).toHaveCount(2);
    await page.reload();
    await expect(page.getByText('Linked', { exact: true })).toHaveCount(2);
    await expect(page.getByRole('checkbox', { name: `Select ${long} (1)` })).toBeDisabled();
    const saved = await (await request.get(`/api/director/v1/catalogs/${slug}/mappings`)).json();
    expect(saved.data.items).toHaveLength(2);
    expect(new Set(saved.data.items.map((item: { rig_id: string }) => item.rig_id)).size).toBe(2);
    expect(new Set(saved.data.items.map((item: { project_id: string }) => item.project_id)).size).toBe(1);
    await page.setViewportSize({ width: 375, height: 812 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy();
    expect(await page.locator('.director-page').evaluate(element => element.scrollWidth <= element.clientWidth)).toBeTruthy();
    await page.screenshot({ path: testInfo.outputPath('director-catalog-mobile.png'), fullPage: true });
    expect(errors).toEqual([]);
  } finally {
    const removed = await request.delete(`/api/databases/${slug}`);
    expect(removed.ok(), await removed.text()).toBeTruthy();
  }
});
