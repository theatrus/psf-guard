import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';
import { openSettings } from '../e2e/helpers';

test("a rig's exposure template takes new gain, twilight and Moon settings from Rig setup, and an unused one can be deleted", async ({ page, request }) => {
  const run = process.env.PSF_GUARD_DIRECTOR_E2E_TMP!;
  const dir = fs.mkdtempSync(path.join(run, 'rig-templates-'));
  const slug = `rig-templates-${randomUUID()}`;
  const file = path.join(dir, 'rig.sqlite');
  const database = new Database(file);
  applyRealSchema(database);
  // A template made in N.I.N.A. with the camera's defaults and no Moon avoidance.
  const seed = database.prepare(`INSERT INTO exposuretemplate (profileId, name, filtername, gain, offset, bin, readoutmode, twilightlevel,
    moonavoidanceenabled, moonavoidanceseparation, moonavoidancewidth, maximumhumidity, defaultexposure, moonrelaxscale,
    moonrelaxmaxaltitude, moonrelaxminaltitude, moondownenabled, ditherevery, minutesOffset, guid)
    VALUES ('profile', ?, ?, -1, -1, 1, -1, 0, 0, 60, 7, 0, 300, 0, 5, -15, 0, -1, 0, ?)`);
  seed.run('Ha 300', 'Ha', randomUUID());
  // One no plan uses, to delete.
  const spare = randomUUID();
  seed.run('OIII 300', 'OIII', spare);
  const post = async (route: string, data: unknown) => {
    const response = await request.post(`/api${route}`, { data, timeout: 5000 });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  try {
    await post('/databases', { name: 'Templates rig', slug, db_path: file, image_dirs: [dir] });
    const catalog = randomUUID();
    const reviewed = await post(`/director/v1/catalogs/${slug}/rig/preview`, { catalog_id: catalog });
    await post(`/director/v1/catalogs/${slug}/rig/apply`, { plan: { catalog_id: catalog }, preview_digest: reviewed.preview_digest });

    await page.goto('/#/sky');
    await openSettings(page);
    await page.getByRole('tab', { name: 'Overview', exact: true }).click();
    await page.getByRole('button', { name: 'Open Templates rig', exact: true }).click();
    await page.getByRole('tab', { name: 'Rig setup', exact: true }).click();
    await page.getByRole('tab', { name: 'Target Scheduler templates', exact: true }).click();
    const templates = page.getByRole('region', { name: 'Target Scheduler templates' });

    await templates.getByRole('spinbutton', { name: 'Gain of Ha 300' }).fill('139');
    // An unsaved edit survives a trip to another Rig setup tab.
    await page.getByRole('tab', { name: 'Optics', exact: true }).click();
    await page.getByRole('tab', { name: 'Target Scheduler templates', exact: true }).click();
    await expect(templates.getByRole('spinbutton', { name: 'Gain of Ha 300' })).toHaveValue('139');
    await templates.getByRole('button', { name: 'Moon and twilight settings for Ha 300' }).click();
    await templates.getByRole('combobox', { name: 'Twilight for Ha 300' }).selectOption('Astronomical');
    await templates.getByLabel('Enable Moon avoidance').check();
    await templates.getByLabel(/Separation at full Moon/).fill('110');
    await templates.getByRole('button', { name: 'Save Ha 300', exact: true }).click();
    await expect(templates.getByText('Ha 300 saved')).toBeVisible();
    await expect(templates.getByRole('button', { name: 'Save Ha 300', exact: true })).toHaveCount(0);

    const row = database.prepare(`SELECT filtername, gain, offset, twilightlevel, moonavoidanceenabled, moonavoidanceseparation
      FROM exposuretemplate WHERE name = 'Ha 300'`).get();
    expect(row).toEqual({ filtername: 'Ha', gain: 139, offset: -1, twilightlevel: 1, moonavoidanceenabled: 1, moonavoidanceseparation: 110 });

    // Delete the unused template; PSF Guard records it for the next sync.
    page.once('dialog', dialog => dialog.accept());
    await templates.getByRole('button', { name: 'Delete OIII 300', exact: true }).click();
    await expect(templates.getByText('OIII 300 deleted')).toBeVisible();
    await expect(templates.getByRole('textbox', { name: 'Name of OIII 300' })).toHaveCount(0);
    expect(database.prepare(`SELECT name FROM exposuretemplate`).all()).toEqual([{ name: 'Ha 300' }]);
    expect(database.prepare(`SELECT template_guid FROM psf_guard_removed_template`).all()).toEqual([{ template_guid: spare }]);

    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  } finally {
    await request.delete(`/api/databases/${slug}`, { timeout: 5000 });
    database.close();
    if (path.dirname(dir) === run && path.basename(dir).startsWith('rig-templates-')) fs.rmSync(dir, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
  }
});
