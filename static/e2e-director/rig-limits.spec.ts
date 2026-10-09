import { expect, test } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

test("a rig's Target Scheduler limits reach every project in its database on Apply", async ({ page, request }, testInfo) => {
  const run = process.env.PSF_GUARD_DIRECTOR_E2E_TMP!;
  const dir = fs.mkdtempSync(path.join(run, 'rig-limits-'));
  const slug = `rig-limits-${randomUUID()}`;
  const file = path.join(dir, 'rig.sqlite');
  const database = new Database(file);
  applyRealSchema(database);
  // Two projects made in N.I.N.A.: one running, one closed long ago.
  const insert = database.prepare(`INSERT INTO project (profileId, name, description, state, priority, minimumtime, minimumaltitude,
    usecustomhorizon, horizonoffset, meridianwindow, filterswitchfrequency, ditherevery, enablegrader, guid)
    VALUES ('profile', ?, '', ?, 1, 30, 0, 0, 5, 0, 0, 0, 0, ?)`);
  insert.run('Hand made', 1, randomUUID());
  insert.run('Old and closed', 3, randomUUID());
  const post = async (route: string, data: unknown) => {
    const response = await request.post(`/api${route}`, { data, timeout: 5000 });
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json()).data;
  };
  try {
    await post('/databases', { name: 'Limits rig', slug, db_path: file, image_dirs: [dir] });
    const catalog = randomUUID();
    const reviewed = await post(`/director/v1/catalogs/${slug}/rig/preview`, { catalog_id: catalog });
    await post(`/director/v1/catalogs/${slug}/rig/apply`, { plan: { catalog_id: catalog }, preview_digest: reviewed.preview_digest });

    await page.goto('/#/sky');
    await page.getByTitle('Settings', { exact: true }).click();
    await page.getByRole('tab', { name: 'Rigs', exact: true }).click();
    await page.getByRole('button', { name: 'Setup Limits rig', exact: true }).click();
    await page.getByRole('tab', { name: 'Limits and delivery', exact: true }).click();
    const fields = page.getByRole('group', { name: 'Rig Target Scheduler limits' });
    const projects = page.getByRole('region', { name: 'Target Scheduler projects' });
    // Nothing set anywhere: every project keeps its own values.
    await expect(projects.getByTestId('rig-scheduling-summary')).toHaveText('All 2 projects in Limits rig have these limits.');

    await fields.getByRole('spinbutton', { name: 'Minimum altitude' }).fill('25');
    await fields.getByRole('spinbutton', { name: 'Meridian window' }).fill('20');
    // Unsaved, the list still shows the saved limits.
    await expect(projects.getByTestId('rig-scheduling-summary')).toContainText('The list shows the saved limits.');
    await page.getByRole('button', { name: 'Save rig profile', exact: true }).click();
    await expect(projects.getByTestId('rig-scheduling-summary')).toHaveText('2 of 2 projects in Limits rig differ.');
    await expect(projects.getByText('minimum altitude 0° → 25°, meridian window off → 20 min')).toHaveCount(2);
    await expect(projects.getByText('Closed', { exact: true })).toBeVisible();

    await projects.getByRole('button', { name: 'Apply to 2 projects', exact: true }).click();
    await expect(projects.getByText('Wrote the limits into 2 projects.')).toBeVisible();
    await expect(projects.getByTestId('rig-scheduling-summary')).toHaveText('All 2 projects in Limits rig have these limits.');
    const rows = database.prepare('SELECT name, minimumaltitude, meridianwindow, horizonoffset FROM project ORDER BY name').all();
    // The horizon offset nobody set stays as N.I.N.A. had it.
    expect(rows).toEqual([
      { name: 'Hand made', minimumaltitude: 25, meridianwindow: 20, horizonoffset: 5 },
      { name: 'Old and closed', minimumaltitude: 25, meridianwindow: 20, horizonoffset: 5 },
    ]);
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  } finally {
    await request.delete(`/api/databases/${slug}`, { timeout: 5000 });
    database.close();
    if (path.dirname(dir) === run && path.basename(dir).startsWith('rig-limits-')) fs.rmSync(dir, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
  }
});
