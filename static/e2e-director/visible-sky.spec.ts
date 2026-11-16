import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';
import { addRig } from './nightsFixtures';

/** A rig database with one target that has a frame, so the Sky has a map. */
function skyDatabase(name: string): string {
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'visible-'));
  const dbPath = path.join(root, 'catalog.sqlite');
  const db = new Database(dbPath);
  applyRealSchema(db);
  const now = Math.floor(Date.now() / 1000);
  db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(1,'current-profile',?,'',1,1,0,0,?)").run(`${name} project`, randomUUID());
  db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES(1,?,1,0.712313,41.2687,2,1,?)').run(`${name} target`, randomUUID());
  db.prepare("INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid) VALUES(1,'current-profile','Ha','Ha',100,30,1,0,?)").run(randomUUID());
  db.prepare("INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(1,'current-profile',300,40,1,1,1,1,1,?)").run(randomUUID());
  db.prepare("INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid) VALUES(1,1,1,?,'Ha',1,?,'current-profile',1,?)").run(now - 86_400, JSON.stringify({ FileName: 'not-here.fits', RA: 10.68, Dec: 41.27, ExposureDuration: 300 }), randomUUID());
  db.close();
  return dbPath;
}

test('Visible sky draws the line for a rig, a second line for a range, and explains a rig with no site', async ({ page, request }) => {
  // The first night for a rig measures the whole sky; a debug server is slow at it.
  test.setTimeout(240_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const placed = `visible-${randomUUID().slice(0, 8)}`;
  const unplaced = `visible-${randomUUID().slice(0, 8)}`;
  try {
    await addRig(request, 'Visible placed', placed, skyDatabase('Visible placed'), { latitude_degrees: 34.2, longitude_degrees: -118.3 });
    // A second rig with no location of its own and no Planning site.
    const unplacedDb = skyDatabase('Visible unplaced');
    const added = await request.post('/api/databases', { data: { name: 'Visible unplaced', slug: unplaced, db_path: unplacedDb, image_dirs: [path.dirname(unplacedDb)] } });
    expect(added.ok(), await added.text()).toBeTruthy();
    expect((await request.get('/api/director/v1/plans')).ok()).toBeTruthy();

    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto('/#/sky');
    await expect(page.locator('.sky-map')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('.sky-visible')).toHaveCount(0);
    const toggle = page.locator('.sky-controls').getByRole('button', { name: 'Visible sky', exact: true });
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-pressed', 'true');
    const panel = page.getByRole('group', { name: 'Visible sky' });
    await panel.getByLabel('Rig').selectOption({ label: 'Visible placed' });
    // One night: one solid line, marked on its visible side, and the legend.
    const every = page.locator('.sky-visible-line.sky-visible-every');
    await expect(every).toHaveAttribute('d', /^M[\d.,-]+L/, { timeout: 120_000 });
    await expect(page.locator('.sky-visible-marks.sky-visible-every')).toHaveAttribute('d', /^M/);
    await expect(page.locator('.sky-visible-any')).toHaveCount(0);
    await expect(panel.getByLabel('Minimum minutes')).toHaveValue('30');
    // Between dawn and noon at the site the night under way is last night.
    await expect(panel.getByRole('status')).toContainText(/Visible placed: in view 30 min or more · (tonight|last night) \(/);
    await expect(panel.getByRole('status')).toContainText('flat horizon at 25°');
    const oneNight = await every.getAttribute('d');
    // A longer minimum moves the line; the way back restores the rig's value.
    await panel.getByLabel('Minimum minutes').fill('240');
    await expect(panel.getByRole('status')).toContainText('in view 4 h or more', { timeout: 60_000 });
    expect(await every.getAttribute('d')).not.toBe(oneNight);
    await panel.getByRole('button', { name: "Use rig's 30 min" }).click();
    await expect(panel.getByLabel('Minimum minutes')).toHaveValue('30');
    await expect(panel.getByRole('status')).toContainText('in view 30 min or more', { timeout: 60_000 });
    // Two nights: a dashed line for at least one night joins the solid one.
    await panel.getByRole('radio', { name: 'Dates' }).click();
    const first = await panel.getByLabel('First night').inputValue();
    expect(first).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    const next = new Date(Date.parse(`${first}T00:00:00Z`) + 86_400_000).toISOString().slice(0, 10);
    await panel.getByLabel('Last night').fill(next);
    await expect(page.locator('.sky-visible-line.sky-visible-any')).toHaveAttribute('d', /^M/, { timeout: 180_000 });
    await expect(page.locator('.sky-visible')).toHaveAttribute('data-nights', 'range');
    await expect(panel.getByRole('status')).toContainText('Solid line: every night');
    // The lines are there in the globe too.
    await page.getByRole('radio', { name: 'Globe' }).click();
    await expect(every).toHaveAttribute('d', /^M[\d.,-]+L/);
    // The choice is kept for the session: a reload draws the same overlay.
    await page.reload();
    await expect(page.getByRole('group', { name: 'Visible sky' }).getByLabel('Rig')).toHaveValue(/.+/);
    await expect(page.locator('.sky-visible-line.sky-visible-any')).toHaveAttribute('d', /^M/, { timeout: 60_000 });
    // A rig with no site: the legend explains it and no line is drawn.
    await page.getByRole('group', { name: 'Visible sky' }).getByLabel('Rig').selectOption({ label: 'Visible unplaced' });
    await expect(page.getByRole('group', { name: 'Visible sky' }).getByRole('status')).toContainText('has no location. Choose a Planning site in Rig setup.');
    await expect(page.getByRole('button', { name: 'Open Rig setup' })).toBeVisible();
    await expect(page.locator('.sky-visible')).toHaveCount(0);
    // Off again: the panel and the lines go.
    await page.locator('.sky-controls').getByRole('button', { name: 'Visible sky', exact: true }).click();
    await expect(page.getByRole('group', { name: 'Visible sky' })).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally {
    for (const slug of [placed, unplaced]) await request.delete(`/api/databases/${slug}`);
  }
});
