import { expect, test, type Page } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

/** A panel's middle on the screen, from its outline on the sky. */
async function middleOf(page: Page, id: string): Promise<[number, number]> {
  const box = await page.locator(`[data-panel-id='${id}'] polygon`).boundingBox();
  if (!box) throw new Error(`panel ${id} is not drawn`);
  return [box.x + box.width / 2, box.y + box.height / 2];
}

test('a shot target stays put while panels are added round it, and activates as targets of one mosaic', async ({ page, request }) => {
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 1440, height: 1000 });
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'mosaic-'));
  const dbPath = path.join(root, 'catalog.sqlite');
  const db = new Database(dbPath);
  applyRealSchema(db);
  db.prepare('INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,guid) VALUES(1,?,?,?,100,30,1,0,?)').run('p', 'Ha', 'Ha', randomUUID());
  db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid) VALUES(1,'p','Heart Nebula','',1,1,0,0,?)").run(randomUUID());
  // One target with twelve frames shot: the framed target.
  db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,rotation,projectid,guid) VALUES(1,?,1,2.5466667,61.45,2,0,1,?)').run('Heart Nebula', randomUUID());
  db.prepare('INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(1,?,300,60,12,10,1,1,1,?)').run('p', randomUUID());
  db.prepare('INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid) VALUES(1,1,1,1750000000,?,1,?,?,1,?)').run('Ha', JSON.stringify({ FileName: 'heart.fits' }), 'p', randomUUID());
  db.close();
  const slug = `mosaic-${randomUUID().slice(0, 8)}`;
  expect((await request.post('/api/databases', { data: { name: 'Askar data', slug, db_path: dbPath, image_dirs: [root] } })).ok()).toBeTruthy();
  try {
    await page.goto('/#/?db=parked');
    await expect(page.locator('[data-project-key]').filter({ hasText: 'Heart Nebula' })).toHaveCount(1);
    // A 1.8° by 1.2° field.
    const view = (await (await request.get(`/api/director/v1/catalogs/${slug}/rig/profile`)).json()).data;
    const optics = { sensor_width_px: 6248, sensor_height_px: 4176, pixel_size_um: 3.76, focal_length_mm: 749, aperture_mm: 107, rotation: { mode: 'manual', angle_degrees: 0 } };
    expect((await request.put(`/api/director/v1/catalogs/${slug}/rig/profile`, { data: { expected_revision: view.profile.revision, optics: { value: optics, source: { kind: 'manual' } }, site: null, horizon: null, sky_quality: null, limits: { value: view.profile.limits.value, source: { kind: 'manual' } }, peer_id: null } })).ok()).toBeTruthy();
    await page.reload();
    await page.locator('[data-project-key]').filter({ hasText: 'Heart Nebula' }).getByRole('button', { name: /^Open the .* plan$/ }).first().click();
    await expect(page.getByTestId('framing-panel-source')).toContainText('Size from Askar data');

    // The shot target is locked by its frames, and holds the center and angle.
    const panel = (id: string) => page.locator(`[data-panel-id='${id}']`);
    await expect(panel('r1c1')).toHaveAttribute('data-panel-state', 'locked');
    await expect(page.getByRole('list', { name: 'Panels' }).getByRole('button', { name: /r1c1/ })).toContainText('12 frames');
    await expect(page.getByLabel('Right ascension degrees')).toBeDisabled();
    await expect(page.getByLabel('Position angle degrees')).toBeDisabled();

    // A column east, then a row north across both.
    await page.getByRole('button', { name: "Add a column on the mosaic's east side" }).click();
    await page.getByRole('button', { name: "Add a row on the mosaic's north side" }).click();
    await expect(page.locator('.framing-panel')).toHaveCount(4);
    await expect(page.getByTestId('framing-issues')).toHaveCount(0);

    // Drag the east panel well out: too thin an overlap, so it goes back.
    await page.getByRole('button', { name: 'Drag moves one panel' }).click();
    const [ax] = await middleOf(page, 'r1c1');
    const [bx, by] = await middleOf(page, 'p2');
    const width = (Math.abs(bx - ax)) / 0.8;
    await page.mouse.move(bx, by);
    await page.mouse.down();
    await page.mouse.move(bx - width * 0.06, by, { steps: 4 });
    await page.mouse.move(bx - width * 0.12, by, { steps: 4 });
    await expect(panel('p2')).toHaveAttribute('data-panel-state', 'invalid');
    await page.mouse.up();
    await expect(page.getByText(/Not placed: r1c1 and p2 overlap by only/)).toBeVisible();
    await expect(panel('p2')).not.toHaveAttribute('data-panel-state', 'invalid');
    // The note offers to place it anyway, without Alt.
    await expect(page.getByTestId('framing-mosaic-notice').getByRole('button', { name: 'Place anyway' })).toBeVisible();
    // Escape in the middle of a drag puts the panel back.
    await page.mouse.move(bx, by);
    await page.mouse.down();
    await page.mouse.move(bx - width * 0.06, by, { steps: 4 });
    await page.keyboard.press('Escape');
    await page.mouse.up();
    await expect(page.getByTestId('framing-mosaic-notice')).toContainText('Put p2 back.');
    expect(Math.abs((await middleOf(page, 'p2'))[0] - bx)).toBeLessThan(2);

    // Turn the north-east panel ten degrees on its own. The rig has no
    // rotator; planning sets the angle all the same.
    await page.getByRole('list', { name: 'Panels' }).getByRole('button', { name: /p4/ }).click();
    await expect(page.getByTestId('framing-turn-handle')).toBeVisible();
    await page.getByTestId('framing-stage').focus();
    await page.keyboard.press('Shift+BracketRight');
    await expect(page.getByLabel('p4 turn degrees')).toHaveValue('10');
    await expect(page.getByTestId('framing-chosen-angle')).toContainText('turned +10.0° from the framing');

    // Save, then activate: three new targets beside the untouched one.
    const saveBar = page.getByRole('region', { name: 'Unsaved changes' });
    await saveBar.getByRole('button', { name: 'Save changes' }).click();
    await expect(saveBar).toContainText('All changes saved');
    await page.getByRole('region', { name: 'Activation due' }).getByRole('button', { name: /^Activate/ }).click();
    const dialog = page.getByRole('dialog', { name: 'Activate on the rigs' });
    await dialog.getByRole('button', { name: 'Preview' }).click();
    await expect(dialog.getByLabel('Askar data activation')).toContainText('Heart Nebula p2');
    // Each panel says the angle it is shot at, and nothing about how the rig gets there.
    await expect(dialog.getByLabel('Askar data activation')).toContainText('turned +10.0°');
    await expect(dialog.getByLabel('Askar data activation')).not.toContainText('by hand');
    await dialog.getByRole('button', { name: 'Apply' }).click();
    await expect(dialog.getByText(/^Applied/).first()).toBeVisible();

    const after = new Database(dbPath, { readonly: true });
    try {
      const targets = after.prepare('SELECT name, ra, dec FROM target ORDER BY Id').all() as Array<{ name: string; ra: number; dec: number }>;
      expect(targets.map(target => target.name)).toEqual(['Heart Nebula', 'Heart Nebula p2', 'Heart Nebula p3', 'Heart Nebula p4']);
      expect(targets[0]).toMatchObject({ ra: 2.5466667, dec: 61.45 });
      // The turned panel is shot at the framing angle plus its turn and north's lean at +61°.
      const turned = (after.prepare("SELECT rotation FROM target WHERE name = 'Heart Nebula p4'").get() as { rotation: number }).rotation;
      expect(Math.abs(turned - 10)).toBeLessThan(3);
      expect(turned).not.toBe(10);
      expect((after.prepare('SELECT isMosaic FROM project').get() as { isMosaic: number }).isMosaic).toBe(1);
      expect((after.prepare('SELECT acquired FROM exposureplan WHERE targetid = 1').get() as { acquired: number }).acquired).toBe(12);
    } finally {
      after.close();
    }
    // The Library reads the four as one mosaic from Director's record.
    const mosaic = (await (await request.get(`/api/db/${slug}/projects/1/mosaic`)).json()).data.mosaic;
    expect(mosaic.source).toBe('director');
    expect(mosaic.panels.map((entry: { panel_id: string }) => entry.panel_id).sort()).toEqual(['p2', 'p3', 'p4', 'r1c1']);
    expect([mosaic.rows, mosaic.columns]).toEqual([2, 2]);
  } finally {
    await request.delete(`/api/databases/${slug}`);
  }
});
