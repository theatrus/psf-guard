import { expect, test, type APIRequestContext } from '@playwright/test';
import Database from 'better-sqlite3';
import { randomUUID } from 'node:crypto';
import { mkdtempSync } from 'node:fs';
import path from 'node:path';
import { applyRealSchema } from '../e2e/fixtures/sync';

/** A rig database with Target Scheduler projects of one N.I.N.A. profile. */
function rigDatabase(projects: Array<{ name: string; priority: number; target: string; ra_hours: number; dec: number; filters: Array<[string, number]>; moon?: boolean; switchEvery?: number }>): string {
  const root = mkdtempSync(path.join(process.env.PSF_GUARD_DIRECTOR_E2E_TMP!, 'nights-'));
  const dbPath = path.join(root, 'catalog.sqlite');
  const db = new Database(dbPath);
  applyRealSchema(db);
  let template = 0;
  let plan = 0;
  projects.forEach((project, index) => {
    const id = index + 1;
    db.prepare("INSERT INTO project (Id,profileId,name,description,state,priority,minimumtime,minimumaltitude,usecustomhorizon,horizonoffset,meridianwindow,filterswitchfrequency,ditherevery,enablegrader,isMosaic,flatsHandling,guid) VALUES(?,'profile-a',?,'',1,?,30,25,0,0,0,?,0,0,0,0,?)")
      .run(id, project.name, project.priority, project.switchEvery ?? 0, randomUUID());
    db.prepare('INSERT INTO target (Id,name,active,ra,dec,epochcode,rotation,roi,projectid,guid) VALUES(?,?,1,?,?,2,0,100,?,?)')
      .run(id, project.target, project.ra_hours, project.dec, id, randomUUID());
    for (const [filter, desired] of project.filters) {
      template += 1;
      plan += 1;
      db.prepare("INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,twilightlevel,moonavoidanceenabled,moonavoidanceseparation,moonavoidancewidth,defaultexposure,guid) VALUES(?,'profile-a',?,?,100,30,1,0,0,?,60,7,300,?)")
        .run(template, `${project.name} ${filter}`, filter, project.moon ? 1 : 0, randomUUID());
      db.prepare("INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid) VALUES(?,'profile-a',300,?,0,0,?,?,1,?)")
        .run(plan, desired, id, template, randomUUID());
    }
  });
  db.close();
  return dbPath;
}

async function addRig(request: APIRequestContext, name: string, slug: string, dbPath: string, site: { latitude_degrees: number; longitude_degrees: number }) {
  const json = async (response: Awaited<ReturnType<APIRequestContext['post']>>) => {
    expect(response.ok(), await response.text()).toBeTruthy();
    return (await response.json()).data;
  };
  await json(await request.post('/api/databases', { data: { name, slug, db_path: dbPath, image_dirs: [path.dirname(dbPath)] } }));
  const catalog = randomUUID();
  const reviewed = await json(await request.post(`/api/director/v1/catalogs/${slug}/rig/preview`, { data: { catalog_id: catalog } }));
  await json(await request.post(`/api/director/v1/catalogs/${slug}/rig/apply`, { data: { plan: { catalog_id: catalog }, preview_digest: reviewed.preview_digest } }));
  const profile = (await json(await request.get(`/api/director/v1/catalogs/${slug}/rig/profile`))).profile;
  await json(await request.put(`/api/director/v1/catalogs/${slug}/rig/profile`, { data: {
    expected_revision: profile.revision,
    optics: null,
    site: { value: { ...site, elevation_meters: 400 }, source: { kind: 'manual' } },
    horizon: null, sky_quality: null,
    limits: { value: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 90, meridian_exclusion: { before_ms: 0, after_ms: 0 } }, source: { kind: 'manual' } },
    peer_id: null,
  } }));
}

/** A screenshot in the test's output, and with PSF_GUARD_CAPTURE_DOCS=1
 *  also in docs/images for the documentation. */
async function shot(target: { screenshot: (options: { path: string; fullPage?: boolean; animations?: 'disabled' }) => Promise<unknown> }, name: string, outputPath: (name: string) => string, fullPage = true) {
  await target.screenshot({ path: outputPath(`${name}.png`), ...(fullPage ? { fullPage } : {}), animations: 'disabled' });
  if (process.env.PSF_GUARD_CAPTURE_DOCS === '1') {
    await target.screenshot({ path: path.resolve(process.cwd(), '..', 'docs', 'images', `${name}.png`), ...(fullPage ? { fullPage } : {}), animations: 'disabled' });
  }
}

test('Nights shows every rig across the coming nights, a night\'s schedule, and marks', async ({ page, request }, testInfo) => {
  test.setTimeout(120_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const slugs = [`nights-north-${randomUUID().slice(0, 6)}`, `nights-south-${randomUUID().slice(0, 6)}`];
  try {
    await addRig(request, 'North rig', slugs[0], rigDatabase([
      { name: 'Heart Nebula', priority: 2, target: 'IC 1805', ra_hours: 2.5467, dec: 61.45, filters: [['Ha', 120], ['OIII', 120]], switchEvery: 3 },
      { name: 'Andromeda', priority: 1, target: 'M 31', ra_hours: 0.7123, dec: 41.27, filters: [['L', 200]], moon: true },
      { name: 'Soul Nebula', priority: 0, target: 'IC 1848', ra_hours: 2.8533, dec: 60.4, filters: [['Ha', 80]] },
    ]), { latitude_degrees: 34.2, longitude_degrees: -118.3 });
    await addRig(request, 'South rig', slugs[1], rigDatabase([
      { name: 'Orion', priority: 1, target: 'M 42', ra_hours: 5.588, dec: -5.39, filters: [['Ha', 150], ['SII', 150]], switchEvery: 2 },
    ]), { latitude_degrees: -31.3, longitude_degrees: 149.1 });

    await page.setViewportSize({ width: 1440, height: 1100 });
    await page.goto('/#/nights?from=2026-11-12');
    const grid = page.getByTestId('nights-grid');
    await expect(grid).toBeVisible({ timeout: 30_000 });
    await expect(grid.getByText('North rig', { exact: true })).toBeVisible();
    await expect(grid.getByText('South rig', { exact: true })).toBeVisible();
    await expect(grid.getByText('Target Scheduler').first()).toBeVisible();
    const north = grid.getByRole('row').filter({ hasText: 'North rig' });
    const cells = north.getByRole('button');
    await expect(cells).toHaveCount(7);
    await expect(cells.first()).toContainText('planned');
    await shot(page, 'nights-grid', name => testInfo.outputPath(name));

    // A night's schedule, with the planner's reasons for a block.
    await cells.nth(1).click();
    const detail = page.getByRole('region', { name: 'Night details' });
    await expect(detail.getByRole('heading', { name: 'Target Scheduler schedule' })).toBeVisible();
    const timeline = detail.getByRole('group', { name: 'Schedule' });
    const blocks = timeline.locator('.nights-block');
    await expect(blocks.first()).toBeVisible({ timeout: 30_000 });
    await blocks.first().click();
    const reasons = detail.getByRole('region', { name: 'Why this target' });
    await expect(reasons).toBeVisible();
    await expect(reasons.getByText('Chosen')).toBeVisible();
    await shot(page, 'nights-schedule', name => testInfo.outputPath(name));

    // Mark the night clear only from 22:00 to 02:00 for this rig.
    await detail.getByLabel('Limited').check();
    await detail.getByLabel('From').fill('22:00');
    await detail.getByLabel('To').fill('02:00');
    await detail.getByLabel('Note').fill('high cirrus');
    await detail.getByRole('button', { name: 'Save mark' }).click();
    await expect(cells.nth(1)).toContainText('◐', { timeout: 30_000 });
    await expect(detail.getByText('Rig: Limited')).toBeVisible();
    await expect(detail.getByText('· high cirrus')).toBeVisible();
    const gap = timeline.locator('.nights-gap[data-reason="unavailable"]');
    await expect(gap.first()).toBeAttached({ timeout: 30_000 });
    await shot(page, 'nights-marked', name => testInfo.outputPath(name));

    // Director's view of the same rig: nothing is activated on it.
    await detail.getByRole('button', { name: 'Compare with Director' }).click();
    await expect(detail.getByRole('heading', { name: 'Director schedule' })).toBeVisible();
    await expect(detail.getByText(/No Director schedule/)).toBeVisible({ timeout: 30_000 });

    // On a phone the grid scrolls in its own box; the page does not.
    await page.setViewportSize({ width: 390, height: 900 });
    await expect(grid).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    await shot(page, 'nights-phone', name => testInfo.outputPath(name));

    // The Live table says what each rig plans tonight and links to the night.
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto('/#/sky?live=1');
    const tonight = page.getByTestId('director-dashboard').getByRole('link', { name: /clear/ }).first();
    await expect(tonight).toBeVisible({ timeout: 30_000 });
    await shot(page.getByTestId('director-dashboard'), 'nights-live-tonight', name => testInfo.outputPath(name), false);
    await tonight.click();
    await expect(page.getByRole('region', { name: 'Night details' })).toBeVisible();
    expect(errors).toEqual([]);
  } finally {
    for (const slug of slugs) await request.delete(`/api/databases/${slug}`);
  }
});
