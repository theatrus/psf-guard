import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

test('the Live chip opens the Sky with each reporting rig drawn where it points', async ({ page, request }, testInfo) => {
  test.setTimeout(60_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const slugs: string[] = [];
  try {
    const now = Math.floor(Date.now() / 1000);
    for (const name of ['Live north', 'Live south']) {
      const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'live-'));
      const dbPath = path.join(root, 'catalog.sqlite');
      const db = new Database(dbPath);
      applyRealSchema(db);
      db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(1,'current-profile',?,'',1,1,0,0,?)").run(`${name} project`, randomUUID());
      // Andromeda for the north rig, Orion for the south one (RA in hours, as Target Scheduler stores it).
      const [ra, dec] = name === 'Live north' ? [0.712313, 41.2687] : [5.588, -5.39];
      db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES(1,?,1,?,?,2,1,?)').run(`${name} target`, ra, dec, randomUUID());
      db.prepare("INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid) VALUES(1,'current-profile','Ha','Ha',100,30,1,0,?)").run(randomUUID());
      db.prepare("INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(1,'current-profile',300,40,1,1,1,1,1,?)").run(randomUUID());
      db.prepare("INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid) VALUES(1,1,1,?,'Ha',1,?,'current-profile',1,?)").run(now - 86_400, JSON.stringify({ FileName: 'not-here.fits', RA: ra * 15, Dec: dec, ExposureDuration: 300 }), randomUUID());
      db.close();
      const slug = `live-${randomUUID().slice(0, 8)}`;
      const added = await request.post('/api/databases', { data: { name, slug, db_path: dbPath, image_dirs: [root] } });
      expect(added.ok(), await added.text()).toBeTruthy();
      slugs.push(slug);
    }
    // Listing plans takes both databases in as rigs.
    expect((await request.get('/api/director/v1/plans')).ok()).toBeTruthy();
    const instance = (await (await request.get('/api/director/v1/status')).json()).data.instance_id as string;
    const report = async (slug: string, status: Record<string, unknown>) => {
      const mapping = (await (await request.get(`/api/director/v1/catalogs/${slug}/mappings`)).json()).data;
      const posted = await request.post(`/api/director/v1/rigs/${mapping.rig.id}/status`, { data: { coordinator_instance_id: instance, catalog_id: mapping.catalog_identity.id, session_id: `s-${slug}`, reported_at_ms: Date.now(), status } });
      expect(posted.ok(), await posted.text()).toBeTruthy();
    };
    // The north rig names its target; the south one reports the mount's own pointing.
    await report(slugs[0], { phase: 'exposing', target_name: 'Live north target' });
    await report(slugs[1], { phase: 'slewing', target_name: 'Somewhere new', pointing: { ra_degrees: 83.82, dec_degrees: -5.39 } });

    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto('/#/?db=parked-catalog');
    const chip = page.getByRole('button', { name: 'Live rigs: 2 rigs · 1 exposing' });
    await expect(chip).toBeVisible();
    await chip.click();
    await expect(page).toHaveURL(/#\/sky\?db=parked-catalog&live=1$/);
    const panel = page.getByRole('complementary', { name: 'Live rigs on the sky' });
    await expect(panel.getByText('Live north', { exact: true })).toBeVisible();
    await expect(panel.getByText('exposing, Live north target')).toBeVisible();
    await expect(panel.getByRole('button', { name: 'Show Live south on the map' })).toContainText('Where it points');
    await expect(panel.getByRole('button', { name: 'Show Live north on the map' })).toContainText('At Live north target (its target)');
    // Both are on the map, at their places.
    await expect(page.locator('[data-rig="Live north"]')).toHaveClass(/is-exposing/);
    await expect(page.locator('[data-rig="Live north"]')).toHaveAttribute('data-source', 'target');
    await expect(page.locator('[data-rig="Live south"]')).toHaveAttribute('data-source', 'pointing');
    await expect(page.locator('[data-rig="Live south"]')).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('sky-live.png') });
    // Choosing a rig turns the map to it and comes in close.
    await panel.getByRole('button', { name: 'Show Live north on the map' }).click();
    await expect(page.locator('.sky-map')).toHaveAttribute('data-zoom', '4.00');
    await expect(page.locator('.sky-map')).toHaveAttribute('data-center', /^10\.7,41\.3$/);
    // The full Live table sits under the map; the Live chip in the controls closes it all.
    await expect(page.getByRole('region', { name: 'Live table' }).getByTestId('director-dashboard')).toBeVisible();
    // Leaving the Sky leaves Live behind; the Live chip in the controls closes it.
    await page.locator('.sky-controls').getByRole('button', { name: 'Live', exact: true }).click();
    await expect(page.locator('[data-rig]')).toHaveCount(0);
    await expect(panel).toHaveCount(0);
    await page.getByRole('button', { name: /^Live rigs/ }).click();
    await expect(panel).toBeVisible();
    await page.getByRole('button', { name: 'Library', exact: true }).click();
    expect(page.url()).not.toContain('live=1');
    expect(errors).toEqual([]);
  } finally {
    for (const slug of slugs) await request.delete(`/api/databases/${slug}`);
  }
});
